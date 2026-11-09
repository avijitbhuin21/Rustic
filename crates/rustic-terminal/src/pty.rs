use crate::emulator::TerminalEmulator;
use anyhow::Result;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub type SessionId = u64;

/// Boxed handle to a spawned shell process. Held by the session-monitor thread
/// so it can poll `try_wait()` to detect shell exit independently of the
/// output-reader's EOF — essential on Windows ConPTY, where the reader never
/// sees EOF until the master PseudoConsole is closed (which we only do *after*
/// learning the shell exited). See `spawn_session_monitor`.
pub type BoxedChild = Box<dyn Child + Send + Sync>;

/// Output buffer cap. Two consumers read this ring:
///   1. The agent `read_terminal_output` tool — only ever wants the recent tail
///      and passes its own (smaller) `max_bytes`, so a larger ring never floods
///      the model.
///   2. The frontend scrollback *replay* (`read_terminal_buffer`) when an xterm
///      instance is (re)created — e.g. an agent-spawned terminal opened after it
///      produced output, a web reconnect, or any path that rebuilds the instance.
///      At 128 KB a long session (a Claude Code run, a verbose build log) only
///      replayed its last sliver, so reopening such a terminal looked like it had
///      "lost" its history. 2 MB keeps a far deeper tail — thousands of lines —
///      while staying bounded per terminal so long-running processes can't blow
///      memory.
pub const OUTPUT_BUFFER_MAX_BYTES: usize = 2 * 1024 * 1024;

pub struct PtySession {
    pub id: SessionId,
    pub label: String,
    pub cwd: PathBuf,
    pub is_agent: bool,
    pub created_at_ms: u64,
    /// OS process id of the spawned shell, captured at spawn time before the
    /// `Child` handle is dropped. Used for display in the UI tab label and the
    /// `@` mention picker so users can reference a specific terminal.
    pub pid: Option<u32>,
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    // reader is taken out via take_reader() for the output streaming thread
    reader: Option<Box<dyn Read + Send>>,
    // child handle is taken out via take_child() for the monitor thread; held
    // here only until create_session hands it off.
    child: Option<BoxedChild>,
    /// Shared rolling byte buffer of recent output. Appended by the output-reader
    /// thread; read (and tail-truncated) by the agent `read_terminal_output` tool.
    pub output_buffer: Arc<Mutex<VecDeque<u8>>>,
    /// Headless VT emulator fed the same bytes as `output_buffer`. Lets the
    /// agent read the *rendered* screen (no escape codes) on demand. Shared so
    /// the output-reader thread can feed it while the agent tool renders it.
    pub emulator: Arc<Mutex<TerminalEmulator>>,
    /// Most recent command sent to this terminal by the agent (for UI display).
    pub last_command: Arc<Mutex<Option<String>>>,
    /// Task ID that owns this session, if agent-spawned. Used by the output
    /// reader to route pty-exit notifications back to the owning task.
    pub task_id: Arc<Mutex<Option<String>>>,
    /// When `Some`, an agent-issued command is (believed to be) still running
    /// in this shell; the value is the instant the command was sent. Set by
    /// the agent broker's `send_command`, cleared by the session monitor when
    /// it detects the shell returned to its prompt.
    pub command_in_flight: Arc<Mutex<Option<Instant>>>,
}

impl PtySession {
    pub fn new(
        cwd: PathBuf,
        label: String,
        is_agent: bool,
        shell_program: Option<String>,
        initial_size: Option<(u16, u16)>,
        extra_env: &[(String, String)],
    ) -> Result<Self> {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);

        // If the frontend already knows the terminal panel's size at spawn
        // time, honor it — otherwise default to a generous 120×30 (instead of
        // the classic 80×24) so TUI tools that read window size at startup
        // and never re-detect (like claude's welcome banner) don't lay out
        // for a cramped terminal that then gets resized seconds later.
        let (cols, rows) = initial_size.unwrap_or((120, 30));
        let pty_system = native_pty_system();
        let pair = pty_system.openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        // Build shell command — use specified shell or system default
        let mut cmd = match shell_program {
            Some(ref prog) => CommandBuilder::new(prog),
            None => CommandBuilder::new_default_prog(),
        };
        cmd.cwd(&cwd);

        // Advertise terminal capabilities to child processes. Without these,
        // TUI tools (claude, vim, htop, etc.) detect a "minimal" terminal and
        // fall back to a defensive boxy renderer with tight line-wrapping.
        // VS Code's terminal sets the same set; matching it gives us the same
        // rich rendering — proper unicode, 24-bit color, inline layouts.
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("TERM_PROGRAM", "rustic");
        cmd.env("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION"));

        // Force Claude Code to emit DEC private mode 2026 (synchronized output)
        // around its repaints. Claude gates 2026 behind a hardcoded terminal
        // allowlist (iTerm/WezTerm/vscode/ghostty/alacritty/kitty/Windows
        // Terminal/…) keyed on TERM_PROGRAM/TERM — "rustic" isn't on it, so
        // without this every redraw is unsynchronized and tears, and the torn
        // frames get committed into scrollback (the "jumbled scroll-up" bug).
        // Our xterm.js 6 frontend honors 2026, so forcing it makes repaints
        // atomic and keeps scrollback clean. Claude-specific; ignored by other
        // programs.
        cmd.env("CLAUDE_CODE_FORCE_SYNC_OUTPUT", "1");

        // Force Claude Code's CLASSIC renderer instead of its "fullscreen"
        // alt-screen renderer. In fullscreen mode Claude expects to own the
        // screen and manage its own virtualized scroll, so it commits NOTHING
        // to the normal-buffer scrollback — but the alt-screen switch itself
        // never engages under our PTY, leaving the user with no scrollback AND
        // no in-app scroll. The classic renderer commits finished conversation
        // lines to the normal buffer, which flow into xterm's scrollback so
        // scroll-up works like any other terminal (this is how Claude behaves
        // in VS Code's terminal). Measured: classic accumulates real
        // scrollback, fullscreen leaves ~0. This var is checked before the
        // user's `tui` setting, so it reliably overrides `tui: fullscreen`.
        cmd.env("CLAUDE_CODE_DISABLE_ALTERNATE_SCREEN", "1");

        // Caller-supplied environment, applied last so it wins over the
        // defaults above. Used to hand external CLI agents their correlation
        // token: the child shell exports it to the agent, which exports it to
        // the hook processes it spawns, which is what lets a hook callback name
        // the tab it came from.
        for (key, value) in extra_env {
            cmd.env(key, value);
        }

        // Spawn child process. We KEEP the `Child` handle (handed off to the
        // monitor thread via take_child) so we can detect shell exit through
        // `try_wait()`. On Windows ConPTY the output reader never receives EOF
        // on shell exit until the master PseudoConsole is closed, so polling
        // the process handle is the only reliable exit signal.
        let child = pair.slave.spawn_command(cmd)?;
        let pid = child.process_id();
        // Drop the slave side — we communicate through the master
        drop(pair.slave);

        let writer = pair.master.take_writer()?;
        let reader = pair.master.try_clone_reader()?;

        let created_at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        Ok(Self {
            id,
            label,
            cwd,
            is_agent,
            created_at_ms,
            pid,
            master: pair.master,
            writer,
            reader: Some(reader),
            child: Some(child),
            output_buffer: Arc::new(Mutex::new(VecDeque::with_capacity(16 * 1024))),
            emulator: Arc::new(Mutex::new(TerminalEmulator::new(cols, rows))),
            last_command: Arc::new(Mutex::new(None)),
            task_id: Arc::new(Mutex::new(None)),
            command_in_flight: Arc::new(Mutex::new(None)),
        })
    }

    /// Take the reader out for spawning a background output thread.
    /// Can only be called once.
    pub fn take_reader(&mut self) -> Option<Box<dyn Read + Send>> {
        self.reader.take()
    }

    /// Take the child handle out for spawning the session-monitor thread.
    /// Can only be called once.
    pub fn take_child(&mut self) -> Option<BoxedChild> {
        self.child.take()
    }

    /// The pty's foreground process group (`tcgetpgrp` on the master); `None`
    /// where unsupported (Windows) or on error.
    pub fn foreground_pgid(&self) -> Option<i32> {
        #[cfg(unix)]
        {
            self.master.process_group_leader().filter(|p| *p > 0)
        }
        #[cfg(not(unix))]
        {
            None
        }
    }

    pub fn write(&mut self, data: &[u8]) -> Result<()> {
        self.writer.write_all(data)?;
        self.writer.flush()?;
        Ok(())
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        // Keep the headless emulator grid in lock-step with the PTY so the
        // agent's rendered-screen view reflows at the right width.
        if let Ok(mut emu) = self.emulator.lock() {
            emu.resize(cols, rows);
        }
        Ok(())
    }
}

/// Append bytes to a shared output buffer, evicting from the front if the
/// buffer exceeds `OUTPUT_BUFFER_MAX_BYTES`. Safe to call from the reader thread.
pub fn append_output(buffer: &Arc<Mutex<VecDeque<u8>>>, data: &[u8]) {
    if let Ok(mut buf) = buffer.lock() {
        buf.extend(data.iter().copied());
        while buf.len() > OUTPUT_BUFFER_MAX_BYTES {
            // Drop the oldest chunk (8KB at a time for efficiency).
            let drop_n = (buf.len() - OUTPUT_BUFFER_MAX_BYTES)
                .max(8 * 1024)
                .min(buf.len());
            buf.drain(..drop_n);
        }
    }
}

/// Does the process `pid` currently have any child process? Used by the
/// session-monitor's idle-close heuristic to tell "shell sitting at its prompt"
/// (no children → safe to reclaim after the idle grace period) apart from
/// "shell running a command" (a child exists, even if it's producing no output
/// — e.g. a quiet `cargo build`), so we never kill a terminal that's actually
/// working.
///
/// Returns `Some(true)`/`Some(false)` when we could enumerate processes, and
/// `None` when we couldn't determine it (or the platform isn't supported) — in
/// which case the caller treats it as "busy" and declines to auto-close.
#[cfg(windows)]
pub fn process_has_children(pid: u32) -> Option<bool> {
    let procs = windows_process_table()?;
    let root_is_launcher = match windows_image_path(pid) {
        Some(path) => is_git_bash_launcher_path(&path),
        None => procs
            .iter()
            .find(|p| p.0 == pid)
            .map(|p| is_posix_shell_exe(&p.2))
            .unwrap_or(false),
    };
    Some(resolve_shell_busy(pid, &procs, root_is_launcher))
}

#[cfg(target_os = "linux")]
pub fn process_has_children(pid: u32) -> Option<bool> {
    child_pids(pid).map(|c| !c.is_empty())
}

/// Every process whose parent is `pid` (scans `/proc/<n>/stat`).
#[cfg(target_os = "linux")]
fn child_pids(pid: u32) -> Option<Vec<u32>> {
    // Scan /proc/<pid>/stat for any process whose PPID is `pid`. Without this
    // the server build (Linux) fell into the "undetermined" branch, so agent
    // terminals never reported command-completion and never auto-closed.
    let entries = std::fs::read_dir("/proc").ok()?;
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.is_empty() || !name.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else {
            continue;
        };
        // Layout is `pid (comm) state ppid ...` and `comm` may itself contain
        // spaces or parentheses, so anchor the parse after the final ')'.
        let Some((_, rest)) = stat.rsplit_once(')') else {
            continue;
        };
        let mut fields = rest.split_whitespace();
        let _state = fields.next();
        if fields.next().and_then(|v| v.parse::<u32>().ok()) == Some(pid) {
            if let Ok(child) = name.parse::<u32>() {
                out.push(child);
            }
        }
    }
    Some(out)
}

#[cfg(target_os = "macos")]
pub fn process_has_children(pid: u32) -> Option<bool> {
    child_pids(pid).map(|c| !c.is_empty())
}

/// Child pids of `pid` via libproc (part of libSystem, always linked).
#[cfg(target_os = "macos")]
fn child_pids(pid: u32) -> Option<Vec<u32>> {
    // Returns the number of child pids written into the buffer, or -1 on
    // error. A NULL-buffer size probe is only an estimate, so pass a real buffer.
    extern "C" {
        fn proc_listchildpids(ppid: i32, buffer: *mut std::ffi::c_void, buffersize: i32) -> i32;
    }
    let ppid = i32::try_from(pid).ok()?;
    let mut pids = [0i32; 64];
    // SAFETY: the buffer is a stack array and we pass its exact byte length.
    let n = unsafe {
        proc_listchildpids(
            ppid,
            pids.as_mut_ptr().cast(),
            std::mem::size_of_val(&pids) as i32,
        )
    };
    if n < 0 {
        return None;
    }
    Some(
        pids.iter()
            .take(n as usize)
            .filter(|&&p| p > 0)
            .map(|&p| p as u32)
            .collect(),
    )
}

/// Is the shell `shell_pid` busy with a FOREGROUND command (the agent's
/// "is my command done?" signal)? `foreground_pgid` is the pty's foreground
/// process group (`tcgetpgrp`). Busy when another group owns the terminal, or —
/// for shells without job control — when a child still shares the shell's own
/// group. Background `cmd &` jobs run in their own group, so a shell sitting at
/// its prompt with live background jobs reads as NOT busy (it accepts input).
/// Falls back to "has any child" when the foreground group is unknown.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub fn shell_command_busy(shell_pid: u32, foreground_pgid: Option<i32>) -> Option<bool> {
    extern "C" {
        fn getpgid(pid: i32) -> i32;
    }
    let children = child_pids(shell_pid)?;
    let Some(fg) = foreground_pgid.filter(|g| *g > 0) else {
        return Some(!children.is_empty());
    };
    let shell = i32::try_from(shell_pid).ok()?;
    // SAFETY: getpgid only reads process metadata; -1 on error is handled.
    let shell_pgid = unsafe { getpgid(shell) };
    let shell_pgid = if shell_pgid > 0 { shell_pgid } else { shell };
    if fg != shell_pgid {
        return Some(true);
    }
    let same_group = children.iter().any(|&c| {
        // SAFETY: as above.
        i32::try_from(c)
            .map(|c| unsafe { getpgid(c) } == shell_pgid)
            .unwrap_or(false)
    });
    Some(same_group)
}

/// Platforms without process groups (Windows): busy when the shell has a
/// child. Git for Windows' `<Git>\bin\bash.exe` is a native launcher that keeps
/// the real MSYS shell (`<Git>\usr\bin\bash.exe`) as its child for the whole
/// session, so measuring the launcher reported every finished command as still
/// running — look one level down through it.
#[cfg(windows)]
pub fn shell_command_busy(shell_pid: u32, _foreground_pgid: Option<i32>) -> Option<bool> {
    process_has_children(shell_pid)
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
pub fn shell_command_busy(shell_pid: u32, _foreground_pgid: Option<i32>) -> Option<bool> {
    process_has_children(shell_pid)
}

/// Busy-ness of a shell from a `(pid, ppid, exe_name)` process table, stepping
/// through a launcher whose only job is to host the real shell.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
fn resolve_shell_busy(root: u32, procs: &[(u32, u32, String)], root_is_launcher: bool) -> bool {
    let kids = |pid: u32| -> Vec<&(u32, u32, String)> {
        procs
            .iter()
            .filter(|p| p.1 == pid && p.0 != pid && !is_console_host_exe(&p.2))
            .collect()
    };
    let mut shell = root;
    if root_is_launcher {
        let k = kids(root);
        if k.len() == 1 && is_posix_shell_exe(&k[0].2) {
            shell = k[0].0;
        }
    }
    !kids(shell).is_empty()
}

#[cfg_attr(not(any(windows, test)), allow(dead_code))]
fn is_posix_shell_exe(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n == "bash.exe" || n == "sh.exe"
}

#[cfg_attr(not(any(windows, test)), allow(dead_code))]
fn is_console_host_exe(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n == "conhost.exe" || n == "openconsole.exe"
}

/// True for Git for Windows' `<Git>\bin\{bash,sh}.exe` launcher (not the real
/// `<Git>\usr\bin\bash.exe` shell).
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
fn is_git_bash_launcher_path(path: &str) -> bool {
    let p = path.replace('/', "\\").to_ascii_lowercase();
    (p.ends_with("\\bin\\bash.exe") || p.ends_with("\\bin\\sh.exe")) && !p.contains("\\usr\\bin\\")
}

/// Snapshot of every process as `(pid, parent_pid, exe_name)`.
#[cfg(windows)]
fn windows_process_table() -> Option<Vec<(u32, u32, String)>> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    // SAFETY: same Toolhelp walk as `process_has_children`.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return None;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut out = Vec::new();
        if Process32FirstW(snap, &mut entry) != 0 {
            loop {
                let len = entry
                    .szExeFile
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(entry.szExeFile.len());
                let name = String::from_utf16_lossy(&entry.szExeFile[..len]);
                out.push((entry.th32ProcessID, entry.th32ParentProcessID, name));
                if Process32NextW(snap, &mut entry) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snap);
        Some(out)
    }
}

/// Full executable path of `pid`, or `None` when it can't be queried.
#[cfg(windows)]
fn windows_image_path(pid: u32) -> Option<String> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    // SAFETY: the handle is checked for null and always closed; the buffer is
    // a stack array whose capacity is passed in `len`.
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len);
        CloseHandle(h);
        if ok == 0 {
            return None;
        }
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

#[cfg(test)]
mod shell_busy_tests {
    use super::*;

    fn t(rows: &[(u32, u32, &str)]) -> Vec<(u32, u32, String)> {
        rows.iter()
            .map(|(a, b, c)| (*a, *b, c.to_string()))
            .collect()
    }

    #[test]
    fn git_bash_launcher_at_prompt_is_idle() {
        // launcher(10) → real bash(11), nothing else: the prompt is waiting.
        let procs = t(&[
            (10, 1, "bash.exe"),
            (11, 10, "bash.exe"),
            (12, 10, "conhost.exe"),
        ]);
        assert!(!resolve_shell_busy(10, &procs, true));
    }

    #[test]
    fn git_bash_launcher_running_command_is_busy() {
        let procs = t(&[
            (10, 1, "bash.exe"),
            (11, 10, "bash.exe"),
            (13, 11, "python.exe"),
        ]);
        assert!(resolve_shell_busy(10, &procs, true));
    }

    #[test]
    fn nested_bash_under_real_shell_still_counts_as_busy() {
        // `bash script.sh` from the real shell: only the launcher is skipped.
        let procs = t(&[
            (10, 1, "bash.exe"),
            (11, 10, "bash.exe"),
            (14, 11, "bash.exe"),
        ]);
        assert!(resolve_shell_busy(10, &procs, true));
    }

    #[test]
    fn non_launcher_shell_with_child_is_busy() {
        let procs = t(&[(20, 1, "pwsh.exe"), (21, 20, "node.exe")]);
        assert!(resolve_shell_busy(20, &procs, false));
        let procs = t(&[(20, 1, "bash.exe"), (21, 20, "bash.exe")]);
        assert!(
            resolve_shell_busy(20, &procs, false),
            "real shell running `bash x.sh`"
        );
    }

    #[test]
    fn launcher_path_detection() {
        assert!(is_git_bash_launcher_path(
            r"C:\Program Files\Git\bin\bash.exe"
        ));
        assert!(is_git_bash_launcher_path(
            r"C:\Users\a\scoop\apps\git\current\bin\sh.exe"
        ));
        assert!(!is_git_bash_launcher_path(
            r"C:\Program Files\Git\usr\bin\bash.exe"
        ));
        assert!(!is_git_bash_launcher_path(
            r"C:\Program Files\PowerShell\7\pwsh.exe"
        ));
    }
}

#[cfg(all(not(windows), not(target_os = "linux"), not(target_os = "macos")))]
pub fn process_has_children(_pid: u32) -> Option<bool> {
    // Not implemented on this platform: returning None makes the idle-close
    // heuristic treat the shell as busy, so it never auto-closes. The
    // shell-exit detection (try_wait) still works everywhere.
    None
}

/// Read the tail of a buffer as a lossy UTF-8 string, up to `max_bytes`.
pub fn read_tail(buffer: &Arc<Mutex<VecDeque<u8>>>, max_bytes: usize) -> String {
    let buf = match buffer.lock() {
        Ok(b) => b,
        Err(_) => return String::new(),
    };
    let start = buf.len().saturating_sub(max_bytes);
    let (front, back) = buf.as_slices();
    let mut bytes: Vec<u8> = Vec::with_capacity(buf.len() - start);
    if start < front.len() {
        bytes.extend_from_slice(&front[start..]);
        bytes.extend_from_slice(back);
    } else {
        bytes.extend_from_slice(&back[start - front.len()..]);
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
mod tests {
    use super::process_has_children;

    #[test]
    fn detects_live_child_and_childless_process() {
        let mut child = std::process::Command::new("sleep")
            .arg("5")
            .spawn()
            .expect("spawn sleep");
        assert_eq!(process_has_children(std::process::id()), Some(true));
        assert_eq!(process_has_children(child.id()), Some(false));
        let _ = child.kill();
        let _ = child.wait();
    }

    /// The session monitor's command-completion signal, end to end through a
    /// real pty: for each installed shell, a running command shows a child,
    /// and the shell drops back to childless once it finishes.
    #[test]
    fn pty_shell_child_state_tracks_command_lifecycle() {
        use super::PtySession;
        use std::time::{Duration, Instant};
        let wait_for = |pid: u32, want: bool, within: Duration| -> bool {
            let start = Instant::now();
            while start.elapsed() < within {
                if process_has_children(pid) == Some(want) {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            false
        };
        for shell in ["/bin/bash", "/bin/zsh", "/bin/sh"] {
            if !std::path::Path::new(shell).is_file() {
                continue;
            }
            let mut s = PtySession::new(
                std::env::temp_dir(),
                "t".into(),
                true,
                Some(shell.to_string()),
                None,
                &[],
            )
            .expect("spawn pty shell");
            // Drain output so the shell never blocks on a full pty.
            let mut reader = s.take_reader().expect("reader");
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                while matches!(std::io::Read::read(&mut reader, &mut buf), Ok(n) if n > 0) {}
            });
            let pid = s.pid.expect("shell pid");
            assert!(
                wait_for(pid, false, Duration::from_secs(5)),
                "{shell}: idle shell should be childless"
            );
            s.write(b"sleep 2\r").expect("write");
            assert!(
                wait_for(pid, true, Duration::from_secs(5)),
                "{shell}: running command should be visible as a child"
            );
            assert!(
                wait_for(pid, false, Duration::from_secs(6)),
                "{shell}: finished command should leave the shell childless"
            );
            let _ = s.write(b"exit\r");
            if let Some(mut c) = s.take_child() {
                let _ = c.kill();
                let _ = c.wait();
            }
        }
    }

    /// A `cmd &` background job keeps the shell's child alive but must not
    /// count as a busy foreground command; a foreground command must.
    #[test]
    fn background_job_is_not_a_busy_command() {
        use super::{shell_command_busy, PtySession};
        use std::time::{Duration, Instant};
        for shell in ["/bin/bash", "/bin/zsh"] {
            if !std::path::Path::new(shell).is_file() {
                continue;
            }
            let mut s = PtySession::new(
                std::env::temp_dir(),
                "t".into(),
                true,
                Some(shell.to_string()),
                None,
                &[],
            )
            .expect("spawn pty shell");
            let mut reader = s.take_reader().expect("reader");
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                while matches!(std::io::Read::read(&mut reader, &mut buf), Ok(n) if n > 0) {}
            });
            let pid = s.pid.expect("shell pid");
            let poll = |s: &PtySession, want: bool, within: Duration| {
                let start = Instant::now();
                while start.elapsed() < within {
                    if shell_command_busy(pid, s.foreground_pgid()) == Some(want) {
                        return true;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                false
            };
            assert!(
                poll(&s, false, Duration::from_secs(5)),
                "{shell}: idle shell is not busy"
            );
            s.write(b"sleep 3 &\r").expect("write");
            assert!(
                super::process_has_children(pid) == Some(true) || {
                    std::thread::sleep(Duration::from_millis(500));
                    super::process_has_children(pid) == Some(true)
                },
                "{shell}: background job is a child"
            );
            std::thread::sleep(Duration::from_millis(400));
            assert_eq!(
                shell_command_busy(pid, s.foreground_pgid()),
                Some(false),
                "{shell}: background job is not busy"
            );
            assert!(
                crate::shell::shell_has_background_job(pid, s.foreground_pgid()),
                "{shell}: idle shell with `cmd &` reports a background job"
            );
            let start = Instant::now();
            while crate::shell::shell_has_background_job(pid, s.foreground_pgid())
                && start.elapsed() < Duration::from_secs(6)
            {
                std::thread::sleep(Duration::from_millis(100));
            }
            assert!(
                !crate::shell::shell_has_background_job(pid, s.foreground_pgid()),
                "{shell}: background job flag clears once the job ends"
            );
            s.write(b"sleep 2\r").expect("write");
            assert!(
                poll(&s, true, Duration::from_secs(5)),
                "{shell}: foreground command is busy"
            );
            assert!(
                poll(&s, false, Duration::from_secs(6)),
                "{shell}: finished foreground command is not busy"
            );
            let _ = s.write(b"exit\r");
            if let Some(mut c) = s.take_child() {
                let _ = c.kill();
                let _ = c.wait();
            }
        }
    }
}

use crate::emulator::TerminalEmulator;
use anyhow::Result;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub type SessionId = u64;

/// Boxed handle to a spawned shell process. Held by the session-monitor thread
/// so it can poll `try_wait()` to detect shell exit independently of the
/// output-reader's EOF — essential on Windows ConPTY, where the reader never
/// sees EOF until the master PseudoConsole is closed (which we only do *after*
/// learning the shell exited). See `spawn_session_monitor`.
pub type BoxedChild = Box<dyn Child + Send + Sync>;

/// Output buffer cap — enough to let an agent read recent output without
/// letting long-running processes blow memory.
pub const OUTPUT_BUFFER_MAX_BYTES: usize = 128 * 1024;

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
}

impl PtySession {
    pub fn new(
        cwd: PathBuf,
        label: String,
        is_agent: bool,
        shell_program: Option<String>,
        initial_size: Option<(u16, u16)>,
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
            let drop_n = (buf.len() - OUTPUT_BUFFER_MAX_BYTES).max(8 * 1024).min(buf.len());
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
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    // SAFETY: standard Toolhelp snapshot walk. We zero-init the entry, set its
    // dwSize as the API requires, and close the snapshot handle on every exit
    // path. All pointers passed are to stack locals that outlive the calls.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return None;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut has_child = false;
        if Process32FirstW(snap, &mut entry) != 0 {
            loop {
                if entry.th32ParentProcessID == pid {
                    has_child = true;
                    break;
                }
                if Process32NextW(snap, &mut entry) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snap);
        Some(has_child)
    }
}

#[cfg(not(windows))]
pub fn process_has_children(_pid: u32) -> Option<bool> {
    // Not implemented off-Windows: returning None makes the idle-close
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
    let bytes: Vec<u8> = buf.iter().skip(start).copied().collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

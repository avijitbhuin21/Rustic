use crate::state::AppState;
use rustic_agent::AgentTerminalExit;
use rustic_terminal::{append_output, BoxedChild, SessionInfo, TerminalEmulator};
use serde::Serialize;
use std::collections::VecDeque;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager, State};

/// Idle grace period for the agent-terminal auto-close (#3): once an agent's
/// shell has run at least one command and then sits at its prompt with no child
/// process for this long, the monitor reclaims it.
const IDLE_CLOSE_TIMEOUT: Duration = Duration::from_secs(30);
/// How often the session-monitor thread polls (shell-exit + idle checks).
const MONITOR_POLL_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Clone, Serialize)]
struct TerminalOutput {
    session_id: u64,
    data: String,
}

/// Emit an event telling the frontend to re-fetch the terminal list.
/// Call this whenever a session is created or destroyed.
pub fn emit_terminal_list_changed(app: &AppHandle) {
    let _ = app.emit("terminal-list-changed", ());
}

/// Spawn a background thread that reads PTY output, streams it to the frontend
/// via `terminal-output` events, and also appends it to the session's rolling
/// buffer so the agent can read back recent output later.
pub fn spawn_output_reader(
    app: AppHandle,
    session_id: u64,
    mut reader: Box<dyn Read + Send>,
    buffer: Arc<Mutex<VecDeque<u8>>>,
    emulator: Arc<Mutex<TerminalEmulator>>,
) {
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break, // EOF
                Ok(n) => {
                    append_output(&buffer, &buf[..n]);
                    // Feed the same bytes into the headless emulator so the
                    // agent can read the rendered screen. A poisoned lock just
                    // means we skip this chunk's grid update — never fatal.
                    if let Ok(mut emu) = emulator.lock() {
                        emu.advance(&buf[..n]);
                    }
                    // PTY output may contain invalid UTF-8, use lossy conversion
                    let text = String::from_utf8_lossy(&buf[..n]).to_string();
                    let _ = app.emit(
                        "terminal-output",
                        TerminalOutput {
                            session_id,
                            data: text,
                        },
                    );
                }
                Err(_) => break,
            }
        }
        // Reader ended — the pty closed (the master was dropped, which on
        // Windows ConPTY is what finally unblocks this read with EOF). Finalize
        // the session. This is idempotent and races safely with the
        // session-monitor thread, which may have already finalized on detecting
        // the shell's exit via try_wait — whichever gets there first wins.
        finalize_session_exit(&app, session_id);
    });
}

/// Tear down a session exactly once: atomically remove it from the manager,
/// queue a pty-exit notification for the owning agent task (if any), and tell
/// the UI to drop the row. Safe to call from multiple threads — the
/// `take_for_exit` removal is the gate, so only the first caller does the work
/// and any later callers no-op.
pub fn finalize_session_exit(app: &AppHandle, session_id: u64) {
    let state = app.state::<AppState>();
    let snapshot = match state.terminal_manager.lock() {
        Ok(mut manager) => manager.take_for_exit(session_id, 4 * 1024),
        Err(_) => None,
    };
    // None → the session was already removed (by another finalize, an explicit
    // close_terminal/kill, etc.). Nothing left to do.
    let Some((task_id_opt, label, last_command, tail)) = snapshot else {
        return;
    };

    // Agent-owned sessions route an exit notification to their task so the
    // executor can surface "your terminal closed" to the model next turn.
    if let Some(task_id) = task_id_opt {
        let exited_at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let entry = AgentTerminalExit {
            session_id,
            label,
            last_command,
            output_tail: tail,
            exited_at_ms,
        };
        if let Ok(mut q) = state.agent_terminal_exits.lock() {
            q.entry(task_id).or_default().push(entry);
        }
    }

    emit_terminal_list_changed(app);
}

/// Per-session monitor thread. Owns the shell's `Child` handle and polls it so
/// we learn the shell exited *independently of the output reader's EOF* — which
/// on Windows ConPTY never arrives until the master PseudoConsole is closed,
/// the very thing we're trying to decide to do. Without this, a shell that runs
/// the model's `exit` would linger forever in the UI (see the bug report).
///
/// It also implements the agent-terminal idle auto-close (#3): once an agent's
/// shell has run at least one command and then sits at its prompt — no child
/// process — for `IDLE_CLOSE_TIMEOUT`, the terminal is reclaimed. We gate on
/// "has the shell ever had a child" so we never close a freshly-spawned
/// terminal or a foreground display terminal that never runs anything in-pty,
/// and we gate on the *live* child-process check so a quiet-but-working command
/// (e.g. a silent `cargo build`) is never killed.
pub fn spawn_session_monitor(
    app: AppHandle,
    session_id: u64,
    mut child: BoxedChild,
    is_agent: bool,
    pid: Option<u32>,
) {
    std::thread::spawn(move || {
        let mut idle_since: Option<Instant> = None;
        // Only arm the idle-close once we've observed the shell actually
        // running something. Protects brand-new and display-only terminals.
        let mut seen_running = false;

        loop {
            // (1) Shell-exit detection — the reliable, cross-platform signal.
            match child.try_wait() {
                Ok(Some(_status)) => {
                    finalize_session_exit(&app, session_id);
                    break;
                }
                Err(_) => {
                    // Lost the ability to query the process; finalize rather
                    // than leak a row, and stop polling a handle we can't read.
                    finalize_session_exit(&app, session_id);
                    break;
                }
                Ok(None) => {}
            }

            // If another path already finalized this session, stop monitoring.
            let still_alive = state_session_exists(&app, session_id);
            if !still_alive {
                break;
            }

            // (2) Idle auto-close — agent terminals only.
            if is_agent {
                if let Some(pid) = pid {
                    match rustic_terminal::process_has_children(pid) {
                        Some(true) => {
                            // A command is running — (re)arm and clear the timer.
                            seen_running = true;
                            idle_since = None;
                        }
                        Some(false) if seen_running => {
                            let since = idle_since.get_or_insert_with(Instant::now);
                            if since.elapsed() >= IDLE_CLOSE_TIMEOUT {
                                finalize_session_exit(&app, session_id);
                                break;
                            }
                        }
                        // Either the shell never ran anything yet, or we
                        // couldn't determine child state — leave it alone.
                        _ => {}
                    }
                }
            }

            std::thread::sleep(MONITOR_POLL_INTERVAL);
        }
    });
}

/// Does a session still exist in the manager? Used by the monitor thread to
/// bail once the reader/close path has finalized the session.
fn state_session_exists(app: &AppHandle, session_id: u64) -> bool {
    let state = app.state::<AppState>();
    state
        .terminal_manager
        .lock()
        .map(|m| m.exists(session_id))
        .unwrap_or(false)
}

/// F-07: validate that a `shell_program` supplied to `create_terminal` matches
/// a detected shell on this machine. Without this, a successful XSS in the
/// webview could invoke `create_terminal` with an arbitrary executable path
/// (e.g. an attacker-dropped `evil.exe` in `%TEMP%`) and obtain PTY-attached
/// process execution, bypassing the user-prompt approval flow.
fn validate_shell_program(candidate: &str) -> Result<(), String> {
    let allowed = detect_shells_blocking().unwrap_or_default();
    if allowed.iter().any(|s| s.path == candidate) {
        return Ok(());
    }
    // Allow short-name resolution against detect_shells (e.g. "powershell"
    // when the user picked a friendly name in the UI). Anything else is
    // refused so XSS can't smuggle in an arbitrary binary path.
    if allowed
        .iter()
        .any(|s| s.name.eq_ignore_ascii_case(candidate))
    {
        return Ok(());
    }
    Err(format!(
        "shell_program `{}` is not in the allowlist returned by detect_shells; \
         refusing to spawn",
        candidate
    ))
}

#[tauri::command]
pub fn create_terminal(
    app: AppHandle,
    state: State<'_, AppState>,
    cwd: Option<String>,
    label: Option<String>,
    is_agent: bool,
    shell_program: Option<String>,
    cols: Option<u16>,
    rows: Option<u16>,
) -> Result<SessionInfo, String> {
    let cwd = cwd
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let label = label.unwrap_or_else(|| "Terminal".to_string());

    // F-07: verify shell_program against the detected-shells allowlist before
    // forwarding to the PTY manager. The frontend ordinarily picks from
    // detect_shells() output, but the IPC has no integrity check that an XSS
    // payload didn't substitute a different value.
    if let Some(ref prog) = shell_program {
        validate_shell_program(prog)?;
    }

    // When the frontend doesn't specify a shell, prefer PowerShell over the
    // portable-pty default (cmd.exe on Windows / $SHELL elsewhere). We skip
    // validate_shell_program here because the resolver returns trusted
    // hardcoded paths (or PATH-resolved pwsh.exe), not user input.
    let shell_program = shell_program.or_else(crate::commands::agent_terminals::preferred_agent_shell);

    // Pass the frontend-measured panel size to the PTY at spawn time so TUIs
    // that read window-size at startup (claude, etc.) don't lay out for a
    // cramped default before the post-render fit() resize lands. Both dims
    // must be sane (> 0) or we fall back to the PtySession default.
    let initial_size = match (cols, rows) {
        (Some(c), Some(r)) if c > 0 && r > 0 => Some((c, r)),
        _ => None,
    };

    let mut manager = state.terminal_manager.lock().unwrap();
    let (info, reader, buffer, emulator, child) = manager
        .create_session(cwd, label, is_agent, shell_program, initial_size)
        .map_err(|e| e.to_string())?;
    drop(manager);

    let session_id = info.id;
    let pid = info.pid;
    spawn_output_reader(app.clone(), session_id, reader, buffer, emulator);
    spawn_session_monitor(app.clone(), session_id, child, is_agent, pid);
    emit_terminal_list_changed(&app);

    Ok(info)
}

#[derive(Clone, Serialize)]
pub struct ShellInfo {
    pub name: String,
    pub path: String,
    pub is_default: bool,
}

/// Detect available shells on the system.
#[tauri::command]
pub async fn detect_shells() -> Result<Vec<ShellInfo>, String> {
    tauri::async_runtime::spawn_blocking(detect_shells_blocking)
        .await
        .map_err(|e| format!("detect_shells task panicked: {e}"))?
}

fn detect_shells_blocking() -> Result<Vec<ShellInfo>, String> {
    let mut shells: Vec<ShellInfo> = Vec::new();

    #[cfg(target_os = "windows")]
    {
        // PowerShell (modern - pwsh)
        if let Some(path) = find_in_path("pwsh.exe") {
            shells.push(ShellInfo {
                name: "PowerShell".to_string(),
                path,
                is_default: false,
            });
        }

        // Windows PowerShell (legacy)
        let win_ps = r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe";
        if Path::new(win_ps).exists() {
            shells.push(ShellInfo {
                name: "Windows PowerShell".to_string(),
                path: win_ps.to_string(),
                is_default: false,
            });
        }

        // Command Prompt
        let cmd = r"C:\Windows\System32\cmd.exe";
        if Path::new(cmd).exists() {
            shells.push(ShellInfo {
                name: "Command Prompt".to_string(),
                path: cmd.to_string(),
                is_default: false,
            });
        }

        // Git Bash — check common install locations plus user-local and PATH
        let mut git_bash_found = false;
        let mut git_bash_candidates: Vec<PathBuf> = vec![
            PathBuf::from(r"C:\Program Files\Git\bin\bash.exe"),
            PathBuf::from(r"C:\Program Files (x86)\Git\bin\bash.exe"),
        ];
        // User-local install (e.g. winget / scoop installs Git here)
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            git_bash_candidates.push(PathBuf::from(&local).join(r"Programs\Git\bin\bash.exe"));
        }
        if let Ok(appdata) = std::env::var("APPDATA") {
            git_bash_candidates.push(PathBuf::from(&appdata).join(r"..\Local\Programs\Git\bin\bash.exe"));
        }
        // Derive from git.exe location in PATH
        if let Some(git_exe) = find_in_path("git.exe") {
            // git.exe is usually at <git-root>\cmd\git.exe or <git-root>\bin\git.exe
            let git_path = PathBuf::from(&git_exe);
            if let Some(parent) = git_path.parent() {
                // Try sibling bin/bash.exe
                git_bash_candidates.push(parent.join("bash.exe"));
                git_bash_candidates.push(parent.join(r"..\bin\bash.exe"));
            }
        }
        for candidate in &git_bash_candidates {
            if candidate.exists() {
                shells.push(ShellInfo {
                    name: "Git Bash".to_string(),
                    path: candidate.to_string_lossy().to_string(),
                    is_default: false,
                });
                git_bash_found = true;
                break;
            }
        }
        // Last resort: bash.exe anywhere in PATH
        if !git_bash_found {
            if let Some(path) = find_in_path("bash.exe") {
                shells.push(ShellInfo {
                    name: "Git Bash".to_string(),
                    path,
                    is_default: false,
                });
            }
        }

        // Mark default: first shell is default
        if !shells.is_empty() {
            shells[0].is_default = true;
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        let unix_shells = [
            ("/bin/zsh", "zsh"),
            ("/bin/bash", "bash"),
            ("/bin/sh", "sh"),
            ("/bin/fish", "fish"),
            ("/usr/bin/fish", "fish"),
        ];
        let mut seen = std::collections::HashSet::new();
        for (path, name) in &unix_shells {
            if Path::new(path).exists() && seen.insert(*name) {
                shells.push(ShellInfo {
                    name: name.to_string(),
                    path: path.to_string(),
                    is_default: false,
                });
            }
        }
        // Mark user's default shell
        if let Ok(default_shell) = std::env::var("SHELL") {
            for s in &mut shells {
                if s.path == default_shell {
                    s.is_default = true;
                    break;
                }
            }
        }
        // If no default set, mark first
        if !shells.iter().any(|s| s.is_default) && !shells.is_empty() {
            shells[0].is_default = true;
        }
    }

    Ok(shells)
}

/// Search PATH for an executable
fn find_in_path(exe: &str) -> Option<String> {
    if let Ok(path_var) = std::env::var("PATH") {
        let separator = if cfg!(windows) { ';' } else { ':' };
        for dir in path_var.split(separator) {
            let full = PathBuf::from(dir).join(exe);
            if full.exists() {
                return Some(full.to_string_lossy().to_string());
            }
        }
    }
    None
}

#[tauri::command]
pub fn write_terminal(
    state: State<'_, AppState>,
    session_id: u64,
    data: String,
) -> Result<(), String> {
    let mut manager = state.terminal_manager.lock().unwrap();
    manager
        .write_session(session_id, data.as_bytes())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn resize_terminal(
    state: State<'_, AppState>,
    session_id: u64,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let manager = state.terminal_manager.lock().unwrap();
    manager
        .resize_session(session_id, cols, rows)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn close_terminal(
    app: AppHandle,
    state: State<'_, AppState>,
    session_id: u64,
) -> Result<(), String> {
    let mut manager = state.terminal_manager.lock().unwrap();
    manager.destroy_session(session_id);
    drop(manager);
    emit_terminal_list_changed(&app);
    Ok(())
}

#[tauri::command]
pub fn list_terminals(state: State<'_, AppState>) -> Result<Vec<SessionInfo>, String> {
    let manager = state.terminal_manager.lock().unwrap();
    Ok(manager.list_sessions())
}

/// Render a terminal's *current visible screen* as plain text (escape codes
/// resolved by the headless emulator). Used by the chat composer when the user
/// tags a terminal, so the snapshot it attaches to the message is clean text —
/// not raw control codes — even for a TUI the user is interacting with.
#[tauri::command]
pub fn read_terminal_screen(state: State<'_, AppState>, session_id: u64) -> Result<String, String> {
    let manager = state.terminal_manager.lock().unwrap();
    manager.render_screen(session_id).map_err(|e| e.to_string())
}

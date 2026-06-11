//! Terminal commands — server dispatch, including PTY output streaming.
//!
//! Mirrors `src-tauri/src/commands/terminal.rs`. The desktop reader/monitor
//! threads emit via Tauri's `AppHandle`; here they capture a cloned
//! [`ServerContext`] (cheap — all `Arc`s) and emit `terminal-output` /
//! `terminal-list-changed` onto the WebSocket hub instead. Event names and
//! payload shapes are byte-identical to desktop so the frontend is unchanged.

use std::collections::VecDeque;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::Value;

use rustic_agent::{AgentTerminalExit, AgentTerminalInfo, AgentTerminals};
use rustic_app::context::{AppContext, EventEmitterExt};
use rustic_app::sync_ext::MutexExt;
use rustic_terminal::{append_output, BoxedChild, TerminalEmulator};

use crate::api::{ok, parse, ApiError};
use crate::context::ServerContext;

/// Idle grace period for the agent-terminal auto-close: once an agent's shell
/// has run at least one command and then sits at its prompt with no child
/// process for this long, the monitor reclaims it.
const IDLE_CLOSE_TIMEOUT: Duration = Duration::from_secs(30);
/// How often the session-monitor thread polls (shell-exit + idle checks).
const MONITOR_POLL_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Clone, Serialize)]
struct TerminalOutput {
    session_id: u64,
    data: String,
}

#[derive(Clone, Serialize)]
pub struct ShellInfo {
    pub name: String,
    pub path: String,
    pub is_default: bool,
}

pub async fn dispatch(
    ctx: &ServerContext,
    command: &str,
    args: &Value,
) -> Option<Result<Value, ApiError>> {
    Some(match command {
        "create_terminal" => create_terminal(ctx, args),
        "write_terminal" => write_terminal(ctx, args),
        "resize_terminal" => resize_terminal(ctx, args),
        "close_terminal" => close_terminal(ctx, args),
        "list_terminals" => list_terminals(ctx),
        "read_terminal_screen" => read_terminal_screen(ctx, args),
        "read_terminal_buffer" => read_terminal_buffer(ctx, args),
        "read_terminal_scrollback" => read_terminal_scrollback(ctx, args),
        "detect_shells" => detect_shells().await,
        _ => return None,
    })
}

// ─── streaming threads (emit via the WS hub instead of AppHandle) ───────────

/// Emit an event telling the frontend to re-fetch the terminal list.
fn emit_terminal_list_changed(ctx: &ServerContext) {
    ctx.emit("terminal-list-changed", ());
}

/// Spawn a background thread that reads PTY output, streams it to the frontend
/// via `terminal-output` events, and appends it to the session's rolling buffer
/// so the agent can read back recent output later.
fn spawn_output_reader(
    ctx: ServerContext,
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
                    // PTY output may contain invalid UTF-8 — lossy conversion.
                    let text = String::from_utf8_lossy(&buf[..n]).to_string();
                    ctx.emit(
                        "terminal-output",
                        TerminalOutput { session_id, data: text },
                    );
                }
                Err(_) => break,
            }
        }
        // Reader ended — the pty closed. Finalize the session (idempotent,
        // races safely with the monitor thread).
        finalize_session_exit(&ctx, session_id);
    });
}

/// Tear down a session exactly once: atomically remove it from the manager,
/// queue a pty-exit notification for the owning agent task (if any), and tell
/// the UI to drop the row.
fn finalize_session_exit(ctx: &ServerContext, session_id: u64) {
    let state = ctx.state();
    let snapshot = match state.terminal_manager.lock() {
        Ok(mut manager) => manager.take_for_exit(session_id, 4 * 1024),
        Err(_) => None,
    };
    // None → the session was already removed. Nothing left to do.
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

    emit_terminal_list_changed(ctx);
}

/// Per-session monitor thread. Owns the shell's `Child` handle and polls it so
/// we learn the shell exited independently of the output reader's EOF — which
/// on Windows ConPTY never arrives until the master PseudoConsole is closed.
/// Also implements the agent-terminal idle auto-close.
fn spawn_session_monitor(
    ctx: ServerContext,
    session_id: u64,
    mut child: BoxedChild,
    is_agent: bool,
    pid: Option<u32>,
) {
    std::thread::spawn(move || {
        let mut idle_since: Option<Instant> = None;
        let mut seen_running = false;

        loop {
            // (1) Shell-exit detection — the reliable, cross-platform signal.
            match child.try_wait() {
                Ok(Some(_status)) => {
                    finalize_session_exit(&ctx, session_id);
                    break;
                }
                Err(_) => {
                    finalize_session_exit(&ctx, session_id);
                    break;
                }
                Ok(None) => {}
            }

            // If another path already finalized this session, stop monitoring.
            if !state_session_exists(&ctx, session_id) {
                break;
            }

            // (2) Idle auto-close — agent terminals only.
            if is_agent {
                if let Some(pid) = pid {
                    match rustic_terminal::process_has_children(pid) {
                        Some(true) => {
                            seen_running = true;
                            idle_since = None;
                        }
                        Some(false) if seen_running => {
                            let since = idle_since.get_or_insert_with(Instant::now);
                            if since.elapsed() >= IDLE_CLOSE_TIMEOUT {
                                finalize_session_exit(&ctx, session_id);
                                break;
                            }
                        }
                        _ => {}
                    }
                }
            }

            std::thread::sleep(MONITOR_POLL_INTERVAL);
        }
    });
}

fn state_session_exists(ctx: &ServerContext, session_id: u64) -> bool {
    ctx.state()
        .terminal_manager
        .lock()
        .map(|m| m.exists(session_id))
        .unwrap_or(false)
}

// ─── shell preference + validation (ported from desktop) ────────────────────

/// Pick a sane default agent shell when the frontend doesn't specify one.
/// Mirrors `commands::agent_terminals::preferred_agent_shell`.
#[cfg(target_os = "windows")]
fn preferred_agent_shell() -> Option<String> {
    if let Some(p) = find_in_path("pwsh.exe") {
        return Some(p);
    }
    let legacy = r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe";
    if Path::new(legacy).exists() {
        return Some(legacy.to_string());
    }
    None
}

#[cfg(not(target_os = "windows"))]
fn preferred_agent_shell() -> Option<String> {
    None
}

/// Validate that a `shell_program` supplied to `create_terminal` matches a
/// detected shell on this machine (anti-XSS: an arbitrary executable path must
/// not be smuggled in).
fn validate_shell_program(candidate: &str) -> Result<(), String> {
    let allowed = detect_shells_blocking().unwrap_or_default();
    if allowed.iter().any(|s| s.path == candidate) {
        return Ok(());
    }
    if allowed.iter().any(|s| s.name.eq_ignore_ascii_case(candidate)) {
        return Ok(());
    }
    Err(format!(
        "shell_program `{}` is not in the allowlist returned by detect_shells; refusing to spawn",
        candidate
    ))
}

// ─── commands ───────────────────────────────────────────────────────────────

fn create_terminal(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct A {
        cwd: Option<String>,
        label: Option<String>,
        is_agent: bool,
        shell_program: Option<String>,
        cols: Option<u16>,
        rows: Option<u16>,
    }
    let a: A = parse(args)?;

    let cwd = a
        .cwd
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let label = a.label.unwrap_or_else(|| "Terminal".to_string());

    if let Some(ref prog) = a.shell_program {
        validate_shell_program(prog)?;
    }
    let shell_program = a.shell_program.or_else(preferred_agent_shell);

    let initial_size = match (a.cols, a.rows) {
        (Some(c), Some(r)) if c > 0 && r > 0 => Some((c, r)),
        _ => None,
    };

    let (info, reader, buffer, emulator, child) = {
        let mut manager = ctx.state().terminal_manager.lock_safe();
        manager
            .create_session(cwd, label, a.is_agent, shell_program, initial_size)
            .map_err(|e| e.to_string())?
    };

    let session_id = info.id;
    let pid = info.pid;
    spawn_output_reader(ctx.clone(), session_id, reader, buffer, emulator);
    spawn_session_monitor(ctx.clone(), session_id, child, a.is_agent, pid);
    emit_terminal_list_changed(ctx);

    ok(info)
}

fn write_terminal(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct A {
        session_id: u64,
        data: String,
    }
    let a: A = parse(args)?;
    let mut manager = ctx.state().terminal_manager.lock_safe();
    manager
        .write_session(a.session_id, a.data.as_bytes())
        .map_err(|e| e.to_string())?;
    ok(serde_json::json!(null))
}

fn resize_terminal(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct A {
        session_id: u64,
        cols: u16,
        rows: u16,
    }
    let a: A = parse(args)?;
    let manager = ctx.state().terminal_manager.lock_safe();
    manager
        .resize_session(a.session_id, a.cols, a.rows)
        .map_err(|e| e.to_string())?;
    ok(serde_json::json!(null))
}

fn close_terminal(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct A {
        session_id: u64,
    }
    let a: A = parse(args)?;
    {
        let mut manager = ctx.state().terminal_manager.lock_safe();
        manager.destroy_session(a.session_id);
    }
    emit_terminal_list_changed(ctx);
    ok(serde_json::json!(null))
}

fn list_terminals(ctx: &ServerContext) -> Result<Value, ApiError> {
    let manager = ctx.state().terminal_manager.lock_safe();
    ok(manager.list_sessions())
}

fn read_terminal_screen(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct A {
        session_id: u64,
    }
    let a: A = parse(args)?;
    let manager = ctx.state().terminal_manager.lock_safe();
    ok(manager.render_screen(a.session_id).map_err(|e| e.to_string())?)
}

fn read_terminal_buffer(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct A {
        session_id: u64,
    }
    let a: A = parse(args)?;
    let manager = ctx.state().terminal_manager.lock_safe();
    ok(manager
        .read_output_tail(a.session_id, rustic_terminal::OUTPUT_BUFFER_MAX_BYTES)
        .map_err(|e| e.to_string())?)
}

fn read_terminal_scrollback(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct A {
        session_id: u64,
    }
    let a: A = parse(args)?;
    let manager = ctx.state().terminal_manager.lock_safe();
    ok(manager
        .render_scrollback_ansi(a.session_id)
        .map_err(|e| e.to_string())?)
}

async fn detect_shells() -> Result<Value, ApiError> {
    let shells = tokio::task::spawn_blocking(detect_shells_blocking)
        .await
        .map_err(|e| ApiError::bad(format!("detect_shells task panicked: {e}")))??;
    ok(shells)
}

fn detect_shells_blocking() -> Result<Vec<ShellInfo>, String> {
    let mut shells: Vec<ShellInfo> = Vec::new();

    #[cfg(target_os = "windows")]
    {
        if let Some(path) = find_in_path("pwsh.exe") {
            shells.push(ShellInfo { name: "PowerShell".to_string(), path, is_default: false });
        }
        let win_ps = r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe";
        if Path::new(win_ps).exists() {
            shells.push(ShellInfo {
                name: "Windows PowerShell".to_string(),
                path: win_ps.to_string(),
                is_default: false,
            });
        }
        let cmd = r"C:\Windows\System32\cmd.exe";
        if Path::new(cmd).exists() {
            shells.push(ShellInfo {
                name: "Command Prompt".to_string(),
                path: cmd.to_string(),
                is_default: false,
            });
        }
        let mut git_bash_found = false;
        let mut git_bash_candidates: Vec<PathBuf> = vec![
            PathBuf::from(r"C:\Program Files\Git\bin\bash.exe"),
            PathBuf::from(r"C:\Program Files (x86)\Git\bin\bash.exe"),
        ];
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            git_bash_candidates.push(PathBuf::from(&local).join(r"Programs\Git\bin\bash.exe"));
        }
        if let Ok(appdata) = std::env::var("APPDATA") {
            git_bash_candidates
                .push(PathBuf::from(&appdata).join(r"..\Local\Programs\Git\bin\bash.exe"));
        }
        if let Some(git_exe) = find_in_path("git.exe") {
            let git_path = PathBuf::from(&git_exe);
            if let Some(parent) = git_path.parent() {
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
        if !git_bash_found {
            if let Some(path) = find_in_path("bash.exe") {
                shells.push(ShellInfo { name: "Git Bash".to_string(), path, is_default: false });
            }
        }
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
        if let Ok(default_shell) = std::env::var("SHELL") {
            for s in &mut shells {
                if s.path == default_shell {
                    s.is_default = true;
                    break;
                }
            }
        }
        if !shells.iter().any(|s| s.is_default) && !shells.is_empty() {
            shells[0].is_default = true;
        }
    }

    Ok(shells)
}

/// Search PATH for an executable.
#[allow(dead_code)]
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

// ─── agent-owned background terminals (AgentTerminals broker) ────────────────
//
// Server implementation of the `rustic_agent::AgentTerminals` trait — the
// bridge the agent's terminal tools (spawn/send/read/kill a shell) call. Mirrors
// the desktop `commands::agent_terminals::TauriAgentTerminals`, but reaches state
// + emits through a cloned [`ServerContext`] instead of an `AppHandle`, and
// reuses this module's hub-emitting reader/monitor threads.

/// Block up to `timeout` waiting for a freshly-spawned shell to print its first
/// output, so the first writes don't land before the PTY input loop is live.
fn wait_for_shell_output(buffer: &Arc<Mutex<VecDeque<u8>>>, timeout: Duration) {
    let start = Instant::now();
    loop {
        let has_output = buffer.lock().map(|b| !b.is_empty()).unwrap_or(true);
        if has_output || start.elapsed() >= timeout {
            return;
        }
        std::thread::sleep(Duration::from_millis(40));
    }
}

/// Map a user-supplied `shell` value (short name or full path) to an executable
/// `portable_pty` can spawn. `None` when blank → caller falls back to default.
fn resolve_shell_name(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.contains('/') || trimmed.contains('\\') {
        return Some(trimmed.to_string());
    }
    #[cfg(target_os = "windows")]
    {
        match trimmed.to_ascii_lowercase().as_str() {
            "cmd" => Some(r"C:\Windows\System32\cmd.exe".to_string()),
            "powershell" | "ps" => {
                let legacy = r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe";
                if Path::new(legacy).exists() {
                    Some(legacy.to_string())
                } else {
                    find_in_path("powershell.exe")
                }
            }
            "pwsh" => find_in_path("pwsh.exe"),
            "bash" => find_in_path("bash.exe").or_else(|| {
                for candidate in [
                    r"C:\Program Files\Git\bin\bash.exe",
                    r"C:\Program Files (x86)\Git\bin\bash.exe",
                ] {
                    if Path::new(candidate).exists() {
                        return Some(candidate.to_string());
                    }
                }
                None
            }),
            "zsh" => find_in_path("zsh.exe"),
            "sh" => find_in_path("sh.exe"),
            "fish" => find_in_path("fish.exe"),
            other => find_in_path(&format!("{}.exe", other)),
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        find_in_path(trimmed)
    }
}

/// Server-side `AgentTerminals` broker. Cheap to construct (holds a cloned ctx).
pub(crate) struct ServerAgentTerminals {
    ctx: ServerContext,
}

impl ServerAgentTerminals {
    pub(crate) fn new(ctx: ServerContext) -> Self {
        Self { ctx }
    }
}

impl AgentTerminals for ServerAgentTerminals {
    fn spawn(
        &self,
        cwd: PathBuf,
        label: String,
        task_id: &str,
        shell_override: Option<String>,
    ) -> Result<u64, String> {
        let state = self.ctx.state();
        let mut manager = state
            .terminal_manager
            .lock()
            .map_err(|e| format!("terminal manager lock poisoned: {}", e))?;
        let shell = shell_override
            .as_deref()
            .and_then(resolve_shell_name)
            .or_else(preferred_agent_shell);
        let is_powershell = shell
            .as_ref()
            .map(|p| {
                let low = p.to_lowercase();
                low.ends_with("powershell.exe")
                    || low.ends_with("pwsh.exe")
                    || low == "powershell"
                    || low == "pwsh"
            })
            .unwrap_or(false);
        let (info, reader, buffer, emulator, child) = manager
            .create_session(cwd, label, true, shell.clone(), None)
            .map_err(|e| e.to_string())?;
        let id = info.id;
        let pid = info.pid;
        let _ = manager.set_task_id(id, task_id);
        drop(manager);

        let poll_buffer = Arc::clone(&buffer);
        spawn_output_reader(self.ctx.clone(), id, reader, buffer, emulator);
        spawn_session_monitor(self.ctx.clone(), id, child, true, pid);

        wait_for_shell_output(&poll_buffer, Duration::from_millis(1500));

        if is_powershell {
            let init = "Set-ExecutionPolicy -Scope Process Bypass -Force; Clear-Host\r";
            if let Ok(mut manager) = state.terminal_manager.lock() {
                let _ = manager.write_session(id, init.as_bytes());
            }
        }

        emit_terminal_list_changed(&self.ctx);
        Ok(id)
    }

    fn send_command(&self, session_id: u64, command: &str) -> Result<(), String> {
        let state = self.ctx.state();
        let manager = state
            .terminal_manager
            .lock()
            .map_err(|e| format!("terminal manager lock poisoned: {}", e))?;
        manager
            .set_last_command(session_id, command)
            .map_err(|e| e.to_string())?;
        let mark = format!("\n$ {}\n", command);
        let _ = manager.append_buffer(session_id, mark.as_bytes());
        drop(manager);

        let mut manager = state
            .terminal_manager
            .lock()
            .map_err(|e| format!("terminal manager lock poisoned: {}", e))?;
        let mut line = command.to_string();
        line.push('\r');
        let write_result = manager.write_session(session_id, line.as_bytes());
        drop(manager);
        write_result.map_err(|e| e.to_string())?;

        emit_terminal_list_changed(&self.ctx);
        Ok(())
    }

    fn read_output(&self, session_id: u64, max_bytes: usize) -> Result<String, String> {
        let state = self.ctx.state();
        let manager = state
            .terminal_manager
            .lock()
            .map_err(|e| format!("terminal manager lock poisoned: {}", e))?;
        manager.read_output_tail(session_id, max_bytes).map_err(|e| e.to_string())
    }

    fn render_screen(&self, session_id: u64) -> Result<String, String> {
        let state = self.ctx.state();
        let manager = state
            .terminal_manager
            .lock()
            .map_err(|e| format!("terminal manager lock poisoned: {}", e))?;
        manager.render_screen(session_id).map_err(|e| e.to_string())
    }

    fn kill(&self, session_id: u64) -> Result<(), String> {
        let state = self.ctx.state();
        let mut manager = state
            .terminal_manager
            .lock()
            .map_err(|e| format!("terminal manager lock poisoned: {}", e))?;
        manager.destroy_session(session_id);
        drop(manager);
        emit_terminal_list_changed(&self.ctx);
        Ok(())
    }

    fn is_agent_session(&self, session_id: u64) -> bool {
        let state = self.ctx.state();
        let manager = match state.terminal_manager.lock() {
            Ok(m) => m,
            Err(_) => return false,
        };
        manager.exists(session_id) && manager.is_agent(session_id)
    }

    fn drain_pending_exits(&self, task_id: &str) -> Vec<AgentTerminalExit> {
        let state = self.ctx.state();
        let mut q = match state.agent_terminal_exits.lock() {
            Ok(g) => g,
            Err(_) => return Vec::new(),
        };
        q.remove(task_id).unwrap_or_default()
    }

    fn available_shells(&self) -> Vec<String> {
        #[cfg(target_os = "windows")]
        let candidates: &[&str] = &["pwsh", "powershell", "cmd", "bash", "zsh", "sh", "fish"];
        #[cfg(not(target_os = "windows"))]
        let candidates: &[&str] = &["bash", "zsh", "sh", "fish"];

        candidates
            .iter()
            .filter(|name| resolve_shell_name(name).is_some())
            .map(|s| s.to_string())
            .collect()
    }

    fn write_raw(&self, session_id: u64, data: &str) -> Result<(), String> {
        let state = self.ctx.state();
        let manager = state
            .terminal_manager
            .lock()
            .map_err(|e| format!("terminal manager lock poisoned: {}", e))?;
        manager
            .append_buffer(session_id, data.as_bytes())
            .map_err(|e| e.to_string())?;
        drop(manager);
        self.ctx.emit(
            "terminal-output",
            TerminalOutput { session_id, data: data.to_string() },
        );
        Ok(())
    }

    fn list_agent_sessions(&self) -> Vec<AgentTerminalInfo> {
        let state = self.ctx.state();
        let manager = match state.terminal_manager.lock() {
            Ok(m) => m,
            Err(_) => return Vec::new(),
        };
        manager
            .list_sessions()
            .into_iter()
            .filter(|s| s.is_agent)
            .map(|s| AgentTerminalInfo {
                session_id: s.id,
                label: s.label,
                cwd: s.cwd,
                last_command: s.last_command,
                task_id: s.task_id,
                created_at_ms: s.created_at_ms,
            })
            .collect()
    }
}

//! Implementation of the `AgentTerminals` broker trait (defined in
//! rustic-agent) that bridges the agent's terminal tools to the host app's
//! shared pty-backed `TerminalManager`.
//!
//! Given an `AppHandle` we can access the managed `AppState`, create/write/
//! destroy pty sessions, and emit `terminal-list-changed` events so the
//! agent panel refreshes its "Active Terminals" list.

use crate::commands::terminal::{emit_terminal_list_changed, spawn_output_reader};
use crate::state::AppState;
use rustic_agent::{AgentTerminalExit, AgentTerminalInfo, AgentTerminals};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

/// Pick a sensible shell for agent-spawned terminals.
///
/// On Windows, `portable-pty`'s default (cmd.exe via %COMSPEC%) is a poor fit
/// for agent workflows: cmd.exe can't execute `.ps1` files, so when an agent
/// runs `.venv\Scripts\Activate` (a PowerShell script) Windows falls back to
/// the file association and pops Notepad. PowerShell handles both `.ps1`
/// activation scripts AND `.bat`/`.cmd` files natively, so we prefer it when
/// available.
///
/// Returns `None` to let portable-pty use its platform default.
#[cfg(target_os = "windows")]
fn preferred_agent_shell() -> Option<String> {
    // pwsh (PowerShell 7+) if installed
    if let Some(p) = find_in_path("pwsh.exe") {
        return Some(p);
    }
    // Windows PowerShell 5.1 ships with every supported Windows version
    let legacy = r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe";
    if Path::new(legacy).exists() {
        return Some(legacy.to_string());
    }
    None
}

#[cfg(not(target_os = "windows"))]
fn preferred_agent_shell() -> Option<String> {
    // On Unix, $SHELL / portable-pty's default is already the user's shell.
    None
}

/// Map a user-supplied `shell` value (short name or full path) to an
/// executable `portable_pty` can spawn. Returns `None` when the input is
/// blank so the caller falls back to the platform default.
fn resolve_shell_name(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Already a path → pass through unchanged.
    if trimmed.contains('/') || trimmed.contains('\\') {
        return Some(trimmed.to_string());
    }
    #[cfg(target_os = "windows")]
    {
        match trimmed.to_ascii_lowercase().as_str() {
            "cmd" => Some(r"C:\Windows\System32\cmd.exe".to_string()),
            "powershell" | "ps" => {
                let legacy = r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe";
                if std::path::Path::new(legacy).exists() {
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
                    if std::path::Path::new(candidate).exists() {
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
        // Actually walk $PATH so `available_shells()` only reports shells
        // that will spawn. portable-pty resolves via PATH at spawn time too,
        // but we want the list we advertise to the model to be accurate.
        find_in_path(trimmed)
    }
}

#[cfg(target_os = "windows")]
fn find_in_path(exe: &str) -> Option<String> {
    let path_var = std::env::var("PATH").ok()?;
    for dir in path_var.split(';') {
        let candidate = PathBuf::from(dir).join(exe);
        if candidate.exists() {
            return Some(candidate.to_string_lossy().to_string());
        }
    }
    None
}

#[cfg(not(target_os = "windows"))]
fn find_in_path(exe: &str) -> Option<String> {
    let path_var = std::env::var("PATH").ok()?;
    for dir in path_var.split(':') {
        let candidate = PathBuf::from(dir).join(exe);
        if candidate.is_file() {
            return Some(candidate.to_string_lossy().to_string());
        }
    }
    None
}

pub struct TauriAgentTerminals {
    app: AppHandle,
}

impl TauriAgentTerminals {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

impl AgentTerminals for TauriAgentTerminals {
    fn spawn(
        &self,
        cwd: PathBuf,
        label: String,
        task_id: &str,
        shell_override: Option<String>,
    ) -> Result<u64, String> {
        let state = self.app.state::<AppState>();
        let mut manager = state
            .terminal_manager
            .lock()
            .map_err(|e| format!("terminal manager lock poisoned: {}", e))?;
        // Agent-specified shell takes priority; fall back to the platform
        // auto-pick. Short names (`pwsh`, `bash`, …) are resolved to full
        // paths on Windows because `portable_pty` doesn't always walk PATH
        // for non-default shells.
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
        let (info, reader, buffer) = manager
            .create_session(cwd, label, true, shell)
            .map_err(|e| e.to_string())?;
        let id = info.id;
        // Tag the session with the owning task so its eventual pty-exit
        // notification gets routed to the right task's queue.
        let _ = manager.set_task_id(id, task_id);

        // Windows PowerShell's default execution policy (Restricted on many
        // installs) blocks local .ps1 scripts — which includes python venv
        // `Activate.ps1`. Scope the bypass to this process only so it doesn't
        // leak system-wide. Also Clear-Host to hide the prep noise.
        if is_powershell {
            let init = "Set-ExecutionPolicy -Scope Process Bypass -Force; Clear-Host\r";
            let _ = manager.write_session(id, init.as_bytes());
        }

        drop(manager);

        spawn_output_reader(self.app.clone(), id, reader, buffer);
        emit_terminal_list_changed(&self.app);
        Ok(id)
    }

    fn send_command(&self, session_id: u64, command: &str) -> Result<(), String> {
        let state = self.app.state::<AppState>();
        let manager = state
            .terminal_manager
            .lock()
            .map_err(|e| format!("terminal manager lock poisoned: {}", e))?;

        // Record the command for UI display and pre-seed the buffer so
        // `read_terminal_output` shows the command even before the pty
        // echoes it back.
        manager
            .set_last_command(session_id, command)
            .map_err(|e| e.to_string())?;
        let mark = format!("\n$ {}\n", command);
        let _ = manager.append_buffer(session_id, mark.as_bytes());
        drop(manager);

        // Write the command followed by CR to the pty stdin. This is what
        // pressing Enter actually sends; `\n` alone does NOT submit the
        // line on Windows ConPTY (cmd.exe/powershell) — the command just
        // sits at the prompt untyped-looking. `\r` works universally:
        // cooked Unix ttys translate CR→NL via ICRNL, and Windows ConPTY
        // treats CR as the line terminator.
        let mut manager = state
            .terminal_manager
            .lock()
            .map_err(|e| format!("terminal manager lock poisoned: {}", e))?;
        let mut line = command.to_string();
        line.push('\r');
        manager
            .write_session(session_id, line.as_bytes())
            .map_err(|e| e.to_string())?;
        drop(manager);

        emit_terminal_list_changed(&self.app);
        Ok(())
    }

    fn read_output(&self, session_id: u64, max_bytes: usize) -> Result<String, String> {
        let state = self.app.state::<AppState>();
        let manager = state
            .terminal_manager
            .lock()
            .map_err(|e| format!("terminal manager lock poisoned: {}", e))?;
        manager
            .read_output_tail(session_id, max_bytes)
            .map_err(|e| e.to_string())
    }

    fn kill(&self, session_id: u64) -> Result<(), String> {
        let state = self.app.state::<AppState>();
        let mut manager = state
            .terminal_manager
            .lock()
            .map_err(|e| format!("terminal manager lock poisoned: {}", e))?;
        manager.destroy_session(session_id);
        drop(manager);
        emit_terminal_list_changed(&self.app);
        Ok(())
    }

    fn is_agent_session(&self, session_id: u64) -> bool {
        let state = self.app.state::<AppState>();
        let manager = match state.terminal_manager.lock() {
            Ok(m) => m,
            Err(_) => return false,
        };
        manager.exists(session_id) && manager.is_agent(session_id)
    }

    fn drain_pending_exits(&self, task_id: &str) -> Vec<AgentTerminalExit> {
        let state = self.app.state::<AppState>();
        let mut q = match state.agent_terminal_exits.lock() {
            Ok(g) => g,
            Err(_) => return Vec::new(),
        };
        q.remove(task_id).unwrap_or_default()
    }

    fn available_shells(&self) -> Vec<String> {
        // Order matters: the first item is what the model will pick by
        // default when it wants "a shell." Put the most capable one first.
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

    fn list_agent_sessions(&self) -> Vec<AgentTerminalInfo> {
        let state = self.app.state::<AppState>();
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
                created_at_ms: s.created_at_ms,
            })
            .collect()
    }
}

use crate::state::AppState;
use rustic_agent::AgentTerminalExit;
use rustic_terminal::{append_output, SessionInfo};
use serde::Serialize;
use std::collections::VecDeque;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager, State};

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
) {
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break, // EOF
                Ok(n) => {
                    append_output(&buffer, &buf[..n]);
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
        // Reader ended — the pty (shell) exited. If this was an agent-owned
        // background terminal, queue a pty-exit notification on the owning
        // task so the executor can surface it to the model on the next turn.
        //
        // We snapshot the session BEFORE destroying it so we can read its
        // task_id / last_command / buffered tail.
        let state = app.state::<AppState>();
        if let Ok(manager) = state.terminal_manager.lock() {
            if manager.is_agent(session_id) {
                if let Some((task_id_opt, label, last_command, tail)) =
                    manager.exit_snapshot(session_id, 4 * 1024)
                {
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
                }
            }
        }
        // Drop the dead session so it disappears from list_terminals().
        if let Ok(mut manager) = state.terminal_manager.lock() {
            manager.destroy_session(session_id);
        }
        // Notify the UI so the terminal row is removed from the active-terminals panel.
        emit_terminal_list_changed(&app);
    });
}

#[tauri::command]
pub fn create_terminal(
    app: AppHandle,
    state: State<'_, AppState>,
    cwd: Option<String>,
    label: Option<String>,
    is_agent: bool,
    shell_program: Option<String>,
) -> Result<SessionInfo, String> {
    let cwd = cwd
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let label = label.unwrap_or_else(|| "Terminal".to_string());

    let mut manager = state.terminal_manager.lock().unwrap();
    let (info, reader, buffer) = manager
        .create_session(cwd, label, is_agent, shell_program)
        .map_err(|e| e.to_string())?;
    drop(manager);

    spawn_output_reader(app.clone(), info.id, reader, buffer);
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
pub fn detect_shells() -> Result<Vec<ShellInfo>, String> {
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

use crate::state::AppState;
use rustic_terminal::SessionInfo;
use serde::Serialize;
use std::io::Read;
use std::path::PathBuf;
use tauri::{AppHandle, Emitter, State};

#[derive(Clone, Serialize)]
struct TerminalOutput {
    session_id: u64,
    data: String,
}

/// Spawn a background thread that reads PTY output and emits events to the frontend.
fn spawn_output_reader(app: AppHandle, session_id: u64, mut reader: Box<dyn Read + Send>) {
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break, // EOF
                Ok(n) => {
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
    });
}

#[tauri::command]
pub fn create_terminal(
    app: AppHandle,
    state: State<'_, AppState>,
    cwd: Option<String>,
    label: Option<String>,
    is_agent: bool,
) -> Result<SessionInfo, String> {
    let cwd = cwd
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let label = label.unwrap_or_else(|| "Terminal".to_string());

    let mut manager = state.terminal_manager.lock().unwrap();
    let (info, reader) = manager
        .create_session(cwd, label, is_agent)
        .map_err(|e| e.to_string())?;

    spawn_output_reader(app, info.id, reader);

    Ok(info)
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
pub fn close_terminal(state: State<'_, AppState>, session_id: u64) -> Result<(), String> {
    let mut manager = state.terminal_manager.lock().unwrap();
    manager.destroy_session(session_id);
    Ok(())
}

#[tauri::command]
pub fn list_terminals(state: State<'_, AppState>) -> Result<Vec<SessionInfo>, String> {
    let manager = state.terminal_manager.lock().unwrap();
    Ok(manager.list_sessions())
}

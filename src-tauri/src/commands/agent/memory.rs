//! Per-project memory commands. The agent persists project-specific
//! context to .rustic/memory.md inside the project root, kept in git so
//! it travels with the project.

use crate::state::AppState;
use tauri::State;

#[tauri::command]
pub fn get_memory(state: State<'_, AppState>, project_id: String) -> Result<String, String> {
    let workspace = state.workspace.lock().unwrap();
    let project = workspace
        .list_projects()
        .into_iter()
        .find(|p| p.id.to_string() == project_id)
        .ok_or_else(|| "Project not found".to_string())?;
    let memory_path = project.root_path.join(".rustic/memory.md");
    drop(workspace);
    // Create the file (and parent dir) if it doesn't exist yet
    if !memory_path.exists() {
        if let Some(parent) = memory_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = rustic_core::io_util::atomic_write(&memory_path, b"");
    }
    Ok(std::fs::read_to_string(&memory_path).unwrap_or_default())
}
#[tauri::command]
pub fn clear_memory(state: State<'_, AppState>, project_id: String) -> Result<(), String> {
    let workspace = state.workspace.lock().unwrap();
    let project = workspace
        .list_projects()
        .into_iter()
        .find(|p| p.id.to_string() == project_id)
        .ok_or_else(|| "Project not found".to_string())?;
    let memory_path = project.root_path.join(".rustic/memory.md");
    drop(workspace);
    if memory_path.exists() {
        rustic_core::io_util::atomic_write(&memory_path, b"").map_err(|e| e.to_string())
    } else {
        Ok(())
    }
}

// ProjectDefaults extracted to ./project_defaults.rs

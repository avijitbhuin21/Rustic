use crate::state::AppState;
use rustic_agent::{checkpoint_ops, CheckpointInfo, FileChange};
use tauri::State;

#[tauri::command]
pub fn list_checkpoints(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<Vec<CheckpointInfo>, String> {
    let db = state.db.lock().unwrap();
    checkpoint_ops::list_checkpoints(&db, &task_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn revert_to_checkpoint(
    state: State<'_, AppState>,
    checkpoint_id: String,
) -> Result<Vec<FileChange>, String> {
    let db = state.db.lock().unwrap();
    checkpoint_ops::revert_to(&db, &checkpoint_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn preview_checkpoint(
    state: State<'_, AppState>,
    checkpoint_id: String,
) -> Result<Vec<FileChange>, String> {
    let db = state.db.lock().unwrap();
    checkpoint_ops::preview_checkpoint(&db, &checkpoint_id).map_err(|e| e.to_string())
}

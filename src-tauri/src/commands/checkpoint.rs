use crate::state::AppState;
use rustic_agent::{checkpoint_ops, CheckpointInfo, FileChange, TaskDiff};
use std::path::PathBuf;
use tauri::State;

/// Look up a checkpoint's owning project root. Needed by revert / preview
/// since the project snapshot machinery operates on paths and doesn't know
/// about task → project → root mappings.
fn project_root_for_checkpoint(
    state: &State<'_, AppState>,
    checkpoint_id: &str,
) -> Result<PathBuf, String> {
    let db = state.db.lock().unwrap();
    let checkpoint = db
        .get_checkpoint(checkpoint_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Checkpoint not found: {}", checkpoint_id))?;
    let task = db
        .get_task(&checkpoint.task_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Task not found for checkpoint: {}", checkpoint_id))?;
    let project = db
        .get_project(&task.project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found for task: {}", task.id))?;
    Ok(PathBuf::from(project.root_path))
}

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
    let project_root = project_root_for_checkpoint(&state, &checkpoint_id)?;
    let db = state.db.lock().unwrap();
    checkpoint_ops::revert_to(&db, &checkpoint_id, &project_root, &state.snapshot_root)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn preview_checkpoint(
    state: State<'_, AppState>,
    checkpoint_id: String,
) -> Result<Vec<FileChange>, String> {
    let project_root = project_root_for_checkpoint(&state, &checkpoint_id)?;
    let db = state.db.lock().unwrap();
    checkpoint_ops::preview_checkpoint(&db, &checkpoint_id, &project_root, &state.snapshot_root)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_checkpoint_diff(
    state: State<'_, AppState>,
    task_id: String,
    checkpoint_id: String,
) -> Result<TaskDiff, String> {
    let project_root = project_root_for_checkpoint(&state, &checkpoint_id)?;
    let db = state.db.lock().unwrap();
    checkpoint_ops::compute_checkpoint_diff(
        &db,
        &task_id,
        &checkpoint_id,
        &project_root,
        &state.snapshot_root,
    )
    .map_err(|e| e.to_string())
}

/// Delete all messages for a task starting from `message_index` (inclusive).
/// Used to truncate the chat history back to a checkpoint so the user can retry.
#[tauri::command]
pub fn truncate_task_messages(
    state: State<'_, AppState>,
    task_id: String,
    message_index: i64,
) -> Result<(), String> {
    let db = state.db.lock().unwrap();
    db.truncate_messages_from(&task_id, message_index)
        .map_err(|e| e.to_string())
}

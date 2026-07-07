//! Worktree-per-task + merge queue commands — desktop transport.
//! Thin wrappers over `rustic_app::worktree`; mirrors
//! `rustic-server/src/commands/worktree.rs` byte-for-byte in behaviour.

use std::sync::Arc;

use tauri::{AppHandle, State};

use rustic_app::context::EventEmitter;
use rustic_app::sync_ext::MutexExt;
use rustic_app::worktree;

use crate::state::AppState;
use crate::transport::TauriEmitter;

fn emitter_of(app: &AppHandle) -> Arc<dyn EventEmitter> {
    Arc::new(TauriEmitter::new(app.clone()))
}

#[tauri::command]
pub fn worktree_get_settings(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    Ok(worktree::get_worktree_settings(&state.db))
}

#[tauri::command]
pub fn worktree_set_settings(
    state: State<'_, AppState>,
    settings: serde_json::Value,
) -> Result<(), String> {
    worktree::set_worktree_settings(&state.db, &settings)
}

#[tauri::command]
pub fn worktree_get(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<Option<rustic_db::TaskWorktreeRow>, String> {
    state
        .db
        .lock_safe()
        .wt_get(&task_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn worktree_list(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<Vec<rustic_db::TaskWorktreeRow>, String> {
    state
        .db
        .lock_safe()
        .wt_list_for_project(&project_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn worktree_merge(
    app: AppHandle,
    state: State<'_, AppState>,
    task_id: String,
) -> Result<(), String> {
    worktree::enqueue_merge(state.inner(), &emitter_of(&app), &task_id)
}

#[tauri::command]
pub async fn worktree_discard(
    app: AppHandle,
    state: State<'_, AppState>,
    task_id: String,
) -> Result<(), String> {
    if worktree::task_turn_running(state.inner(), &task_id) {
        return Err(
            "This task is currently running — stop it before discarding its worktree.".into(),
        );
    }
    let db = state.db.clone();
    let tid = task_id.clone();
    let data_dir = crate::app_paths::app_data_dir(&app).map_err(|e| e.to_string())?;
    let root =
        tokio::task::spawn_blocking(move || worktree::discard_task_worktree(&db, &data_dir, &tid))
            .await
            .map_err(|e| format!("discard worker panicked: {e}"))??;
    emitter_of(&app).emit_json(
        worktree::WORKTREE_EVENT,
        serde_json::json!({ "task_id": task_id, "state": "discarded" }),
    );
    if let Some(root) = root {
        state
            .merge_queues
            .clone()
            .ensure_worker(state.db.clone(), emitter_of(&app), root);
    }
    Ok(())
}

fn row_of(
    state: &State<'_, AppState>,
    task_id: &str,
) -> Result<rustic_db::TaskWorktreeRow, String> {
    state
        .db
        .lock_safe()
        .wt_get(task_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("no worktree for task {task_id}"))
}

#[tauri::command]
pub async fn worktree_review_files(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<Vec<rustic_git::FileDiff>, String> {
    let row = row_of(&state, &task_id)?;
    tokio::task::spawn_blocking(move || worktree::review_diff(&row))
        .await
        .map_err(|e| format!("review worker panicked: {e}"))?
}

#[tauri::command]
pub async fn worktree_file_diff(
    state: State<'_, AppState>,
    task_id: String,
    path: String,
) -> Result<rustic_git::FileDiff, String> {
    let row = row_of(&state, &task_id)?;
    tokio::task::spawn_blocking(move || worktree::review_file_diff(&row, &path))
        .await
        .map_err(|e| format!("diff worker panicked: {e}"))?
}

#[tauri::command]
pub async fn worktree_conflict_prompt(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<String, String> {
    let row = row_of(&state, &task_id)?;
    tokio::task::spawn_blocking(move || worktree::conflict_resolution_prompt(&row))
        .await
        .map_err(|e| format!("prompt worker panicked: {e}"))?
}

/// Reconcile a parked worktree: returns `{action:"resolve", prompt}` when a
/// rebase is genuinely mid-flight (agent should fix it), or `{action:"requeued"}`
/// when the tree is clean and the item was simply re-queued.
#[tauri::command]
pub async fn worktree_reconcile(
    app: AppHandle,
    state: State<'_, AppState>,
    task_id: String,
) -> Result<serde_json::Value, String> {
    let row = row_of(&state, &task_id)?;
    let plan = tokio::task::spawn_blocking(move || worktree::reconcile_plan(&row))
        .await
        .map_err(|e| format!("reconcile worker panicked: {e}"))??;
    match plan {
        worktree::Reconcile::AgentPrompt(prompt) => {
            Ok(serde_json::json!({ "action": "resolve", "prompt": prompt }))
        }
        worktree::Reconcile::AlreadyClean => {
            worktree::enqueue_merge(state.inner(), &emitter_of(&app), &task_id)?;
            Ok(serde_json::json!({ "action": "requeued" }))
        }
    }
}

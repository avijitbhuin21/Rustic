//! file_history commands — server dispatch.
//!
//! Mirrors `src-tauri/src/commands/file_history.rs` exactly: every server
//! handler resolves a per-project `FileHistoryHandle` (lazily building the
//! shadow store + `SweepWorker` the first time a project is touched) and calls
//! the same `FileHistory` core methods the desktop `#[tauri::command]` bodies
//! call, so browser behavior matches desktop.
//!
//! Desktop differences mapped to the server:
//!   * `crate::app_paths::app_data_dir(app)` → `ctx.data_dir()` (the same base
//!     the server's DB + shadow store already live under).
//!   * Tauri `app.emit(event, payload)` → `ctx.hub.publish(event, json)`.
//!   * `tauri::async_runtime::handle()` (tokio) → `tokio::runtime::Handle::current()`
//!     (the axum/tokio runtime the server already runs on).
//!   * `tauri::async_runtime::spawn_blocking` → `tokio::task::spawn_blocking`.

use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use rustic_app::context::AppContext;
use rustic_app::state::{AppState, FileHistoryHandle};
use rustic_agent::{FileHistory, SweepWorker, TodoItem};

use crate::api::{ok, parse, ApiError};
use crate::context::ServerContext;
use crate::hub::EventHub;

pub async fn dispatch(
    ctx: &ServerContext,
    command: &str,
    args: &Value,
) -> Option<Result<Value, ApiError>> {
    Some(match command {
        "fh_list_files" => fh_list_files(ctx, args).await,
        "fh_file_diff" => fh_file_diff(ctx, args).await,
        "fh_revert" => fh_revert(ctx, args).await,
        "fh_revert_from_message" => fh_revert_from_message(ctx, args).await,
        "fh_revert_task" => fh_revert_task(ctx, args).await,
        "fh_plan_revert_from_message" => fh_plan_revert_from_message(ctx, args).await,
        "fh_plan_revert_task" => fh_plan_revert_task(ctx, args).await,
        "fh_list_snapshots" => fh_list_snapshots(ctx, args).await,
        "fh_list_task_net_changes" => fh_list_task_net_changes(ctx, args).await,
        "fh_list_task_paths" => fh_list_task_paths(ctx, args).await,
        "fh_revert_path" => fh_revert_path(ctx, args).await,
        _ => return None,
    })
}

// ---- arg structs (snake_case fields matching desktop command params) ----

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectMessageArg {
    project_root: String,
    message_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectFileDiffArg {
    project_root: String,
    message_id: String,
    path: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectTaskArg {
    project_root: String,
    task_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectTaskPathArg {
    project_root: String,
    task_id: String,
    path: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TaskOnlyArg {
    task_id: String,
}

// ---- payload structs (camelCase wire format the frontend expects) ----

#[derive(Debug, Clone, Serialize)]
struct ChangedFile {
    path: String,
    kind: &'static str,
    binary: bool,
    additions: u32,
    deletions: u32,
}

#[derive(Debug, Clone, Serialize)]
struct FileDiffPayload {
    path: String,
    kind: &'static str,
    binary: bool,
    additions: u32,
    deletions: u32,
    unified: String,
}

#[derive(Debug, Clone, Serialize)]
struct RevertPlanRow {
    path: String,
    action: &'static str,
}

#[derive(Debug, Clone, Serialize)]
struct RevertOutcome {
    path: String,
    /// "rewritten" | "deleted" | "unchanged" | "failed"
    action: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct SnapshotRow {
    message_id: String,
    sequence: i64,
    created_at: String,
}

#[derive(Debug, Clone, Serialize)]
struct TaskNetChangePayload {
    path: String,
    kind: &'static str,
    binary: bool,
    additions: u32,
    deletions: u32,
    anchor_message_id: String,
    is_dir: bool,
    exists_on_disk: bool,
}

/// Event payload mirror of desktop `AgentTodoUpdatedEvent` (`#[derive(Serialize)]`,
/// no rename, so wire fields are snake_case: `task_id`, `todos`).
#[derive(Clone, Serialize)]
struct AgentTodoUpdatedEvent {
    task_id: String,
    todos: Vec<TodoItem>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
enum FileTrackedKindPayload {
    #[allow(dead_code)]
    EditTool,
    BashSweep,
}

#[derive(Debug, Clone, Serialize)]
struct FileTrackedPayload {
    task_id: String,
    message_id: String,
    kind: FileTrackedKindPayload,
    paths: Vec<String>,
}

// ---- shared helpers ----

/// Mirrors the desktop `outcomes_to_payload`.
fn outcomes_to_payload(outcomes: Vec<rustic_agent::RestoreOutcome>) -> Vec<RevertOutcome> {
    outcomes
        .into_iter()
        .map(|o| match o {
            rustic_agent::RestoreOutcome::Rewritten(p) => RevertOutcome {
                path: p,
                action: "rewritten",
                error: None,
            },
            rustic_agent::RestoreOutcome::Deleted(p) => RevertOutcome {
                path: p,
                action: "deleted",
                error: None,
            },
            rustic_agent::RestoreOutcome::Unchanged(p) => RevertOutcome {
                path: p,
                action: "unchanged",
                error: None,
            },
            rustic_agent::RestoreOutcome::Failed { path, error } => RevertOutcome {
                path,
                action: "failed",
                error: Some(error),
            },
        })
        .collect()
}

/// Mirrors the desktop `plan_to_payload`.
fn plan_to_payload(plan: Vec<rustic_agent::RevertPlanEntry>) -> Vec<RevertPlanRow> {
    plan.into_iter()
        .map(|e| RevertPlanRow {
            path: e.path,
            action: e.action,
        })
        .collect()
}

/// Canonicalize the supplied project_root, mirroring the desktop bodies.
fn canon_root(project_root: &str) -> Result<std::path::PathBuf, ApiError> {
    std::path::PathBuf::from(project_root)
        .canonicalize()
        .map_err(|e| ApiError::from(format!("canonicalize {project_root}: {e}")))
}

/// Get-or-create a `FileHistoryHandle` for `project_root`. Server-side mirror
/// of the desktop `get_or_create_handle`: lazily initializes the shadow store +
/// tokio sweep worker the first time a project is touched, then caches the
/// handle in `state.file_history_registry` keyed by canonical root.
///
/// `project_root` must already be canonicalized (callers go through
/// [`canon_root`]).
pub(crate) fn get_or_create_handle(
    state: &Arc<AppState>,
    data_dir: &Path,
    hub: &EventHub,
    project_root: &Path,
) -> Result<FileHistoryHandle, String> {
    // Fast path: already in the registry.
    {
        let registry = state
            .file_history_registry
            .lock()
            .map_err(|e| format!("file_history_registry mutex poisoned: {e}"))?;
        if let Some(handle) = registry.get(&project_root.to_string_lossy().to_string()) {
            return Ok(handle.clone());
        }
    }

    // Slow path: construct outside the registry lock to keep contention low.
    // Spin up the FS-watcher accumulator before FileHistory so the targeted-
    // track path has it from the very first open_snapshot. Watcher failure is
    // non-fatal — FileHistory falls back to full walks with no accumulator.
    let accumulator = Arc::new(rustic_agent::DirtyPathAccumulator::new(
        project_root.to_path_buf(),
    ));

    let history = Arc::new(
        FileHistory::new_with_accumulator(
            Arc::clone(&state.db),
            project_root.to_path_buf(),
            data_dir,
            Some(Arc::clone(&accumulator)),
        )
        .map_err(|e| format!("FileHistory::new: {e}"))?,
    );

    let watcher = match rustic_agent::FileWatcher::spawn(
        project_root.to_path_buf(),
        Arc::clone(&accumulator),
    ) {
        Ok(w) => Some(Arc::new(w)),
        Err(e) => {
            tracing::warn!(
                root = %project_root.display(),
                ?e,
                "fs watcher registration failed — falling back to full-walk sweeps"
            );
            None
        }
    };

    // Sweep callback forwards completed background sweeps to the web event hub
    // as `agent-file-tracked`, matching the desktop Tauri event surface so the
    // same UI path renders both edit-tool and bash-sweep changes.
    let hub_for_cb = hub.clone();
    let cb: rustic_agent::file_history::ChangeCallback =
        Arc::new(move |task_id: &str, message_id: &str, paths: &[String]| {
            let payload = FileTrackedPayload {
                task_id: task_id.to_string(),
                message_id: message_id.to_string(),
                kind: FileTrackedKindPayload::BashSweep,
                paths: paths.to_vec(),
            };
            if let Ok(json) = serde_json::to_value(&payload) {
                hub_for_cb.publish("agent-file-tracked", json);
            }
        });

    // The server always runs inside the tokio runtime (axum), so the current
    // handle is the runtime the SweepWorker should live on.
    let rt_handle = tokio::runtime::Handle::current();
    let sweep = Arc::new(SweepWorker::spawn(rt_handle, (*history).clone(), cb));

    let handle = FileHistoryHandle {
        history,
        sweep,
        _watcher: watcher,
    };
    let mut registry = state
        .file_history_registry
        .lock()
        .map_err(|e| format!("file_history_registry mutex poisoned: {e}"))?;
    let key = project_root.to_string_lossy().to_string();
    let entry = registry.entry(key).or_insert(handle);
    Ok(entry.clone())
}

/// Resolve + (lazily) build the handle for a project_root arg, mapping errors
/// to `ApiError`.
fn handle_for(
    ctx: &ServerContext,
    project_root: &str,
) -> Result<(std::path::PathBuf, FileHistoryHandle), ApiError> {
    let canon = canon_root(project_root)?;
    let handle = get_or_create_handle(ctx.state(), &ctx.data_dir(), &ctx.hub, &canon)
        .map_err(ApiError::from)?;
    Ok((canon, handle))
}

/// Server mirror of desktop `restore_todos_for_message`. Looks up the todo
/// snapshot anchored at `message_id`, writes it back as the current list, and
/// emits `agent-todo-updated`. Silent no-op when the snapshot doesn't exist.
fn restore_todos_for_message(ctx: &ServerContext, message_id: &str) {
    let state = ctx.state();
    let snapshot = match state.db.lock() {
        Ok(db) => match db.get_todo_snapshot(message_id) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(message_id, ?e, "get_todo_snapshot failed; todos not restored");
                return;
            }
        },
        Err(e) => {
            tracing::warn!(message_id, %e, "db lock poisoned; todos not restored");
            return;
        }
    };
    let Some((task_id, todos_json)) = snapshot else {
        return;
    };
    apply_restored_todos(ctx, &task_id, &todos_json);
}

/// Server mirror of desktop `restore_todos_for_task` — uses the EARLIEST
/// snapshot in the task (its pre-task state).
fn restore_todos_for_task(ctx: &ServerContext, task_id: &str) {
    let state = ctx.state();
    let snapshot = match state.db.lock() {
        Ok(db) => match db.get_first_todo_snapshot_for_task(task_id) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(task_id, ?e, "get_first_todo_snapshot_for_task failed; todos not restored");
                return;
            }
        },
        Err(e) => {
            tracing::warn!(task_id, %e, "db lock poisoned; todos not restored");
            return;
        }
    };
    let Some(todos_json) = snapshot else {
        return;
    };
    apply_restored_todos(ctx, task_id, &todos_json);
}

fn apply_restored_todos(ctx: &ServerContext, task_id: &str, todos_json: &str) {
    let state = ctx.state();
    if let Ok(db) = state.db.lock() {
        if let Err(e) = db.set_task_todos(task_id, todos_json) {
            tracing::warn!(task_id, ?e, "set_task_todos during revert failed");
            return;
        }
    } else {
        return;
    }
    let todos: Vec<TodoItem> = serde_json::from_str(todos_json).unwrap_or_default();
    let event = AgentTodoUpdatedEvent {
        task_id: task_id.to_string(),
        todos,
    };
    if let Ok(json) = serde_json::to_value(&event) {
        ctx.hub.publish("agent-todo-updated", json);
    }
}

// ---- command handlers (one per desktop #[tauri::command]) ----

async fn fh_list_files(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: ProjectMessageArg = parse(args)?;
    let (_canon, handle) = handle_for(ctx, &a.project_root)?;
    let message_id = a.message_id;
    let result: Result<Vec<ChangedFile>, String> = tokio::task::spawn_blocking(move || {
        let stats = handle
            .history
            .list_files_with_stats(&message_id)
            .map_err(|e| format!("fh_list_files: {e}"))?;
        Ok(stats
            .into_iter()
            .map(|s| ChangedFile {
                path: s.path,
                kind: s.kind,
                binary: s.binary,
                additions: s.additions,
                deletions: s.deletions,
            })
            .collect())
    })
    .await
    .map_err(|e| ApiError::from(format!("fh_list_files task panicked: {e}")))?;
    ok(result?)
}

async fn fh_file_diff(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: ProjectFileDiffArg = parse(args)?;
    let (_canon, handle) = handle_for(ctx, &a.project_root)?;
    let message_id = a.message_id;
    let path = a.path;
    let result: Result<FileDiffPayload, String> = tokio::task::spawn_blocking(move || {
        let diff = handle
            .history
            .file_diff(&message_id, &path)
            .map_err(|e| format!("fh_file_diff: {e}"))?;
        Ok(FileDiffPayload {
            path: diff.path,
            kind: diff.kind,
            binary: diff.binary,
            additions: diff.additions,
            deletions: diff.deletions,
            unified: diff.unified,
        })
    })
    .await
    .map_err(|e| ApiError::from(format!("fh_file_diff task panicked: {e}")))?;
    ok(result?)
}

async fn fh_revert(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: ProjectMessageArg = parse(args)?;
    let (_canon, handle) = handle_for(ctx, &a.project_root)?;
    let message_id = a.message_id;
    let msg = message_id.clone();
    let outcomes = tokio::task::spawn_blocking(move || {
        handle.history.revert(&msg).map_err(|e| format!("revert: {e}"))
    })
    .await
    .map_err(|e| ApiError::from(format!("fh_revert task panicked: {e}")))?
    .map_err(ApiError::from)?;
    restore_todos_for_message(ctx, &message_id);
    ok(outcomes_to_payload(outcomes))
}

async fn fh_plan_revert_from_message(
    ctx: &ServerContext,
    args: &Value,
) -> Result<Value, ApiError> {
    let a: ProjectMessageArg = parse(args)?;
    let (_canon, handle) = handle_for(ctx, &a.project_root)?;
    let message_id = a.message_id;
    let result: Result<Vec<RevertPlanRow>, String> = tokio::task::spawn_blocking(move || {
        let plan = handle
            .history
            .plan_revert_from_message(&message_id)
            .map_err(|e| format!("plan_revert_from_message: {e}"))?;
        Ok(plan_to_payload(plan))
    })
    .await
    .map_err(|e| ApiError::from(format!("fh_plan_revert_from_message task panicked: {e}")))?;
    ok(result?)
}

async fn fh_revert_from_message(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: ProjectMessageArg = parse(args)?;
    let (_canon, handle) = handle_for(ctx, &a.project_root)?;
    let message_id = a.message_id;
    let msg = message_id.clone();
    let outcomes = tokio::task::spawn_blocking(move || {
        handle
            .history
            .revert_from_message(&msg)
            .map_err(|e| format!("revert_from_message: {e}"))
    })
    .await
    .map_err(|e| ApiError::from(format!("fh_revert_from_message task panicked: {e}")))?
    .map_err(ApiError::from)?;
    restore_todos_for_message(ctx, &message_id);
    ok(outcomes_to_payload(outcomes))
}

async fn fh_plan_revert_task(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: ProjectTaskArg = parse(args)?;
    let (_canon, handle) = handle_for(ctx, &a.project_root)?;
    let task_id = a.task_id;
    let result: Result<Vec<RevertPlanRow>, String> = tokio::task::spawn_blocking(move || {
        let plan = handle
            .history
            .plan_revert_task(&task_id)
            .map_err(|e| format!("plan_revert_task: {e}"))?;
        Ok(plan_to_payload(plan))
    })
    .await
    .map_err(|e| ApiError::from(format!("fh_plan_revert_task task panicked: {e}")))?;
    ok(result?)
}

async fn fh_revert_path(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: ProjectTaskPathArg = parse(args)?;
    let (_canon, handle) = handle_for(ctx, &a.project_root)?;
    let tid = a.task_id;
    let target_path = a.path;
    let outcomes = tokio::task::spawn_blocking(move || {
        handle
            .history
            .revert_path(&tid, &target_path)
            .map_err(|e| format!("revert_path: {e}"))
    })
    .await
    .map_err(|e| ApiError::from(format!("fh_revert_path task panicked: {e}")))?
    .map_err(ApiError::from)?;
    ok(outcomes_to_payload(outcomes))
}

async fn fh_revert_task(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: ProjectTaskArg = parse(args)?;
    let (_canon, handle) = handle_for(ctx, &a.project_root)?;
    let task_id = a.task_id;
    let tid = task_id.clone();
    let outcomes = tokio::task::spawn_blocking(move || {
        handle
            .history
            .revert_task(&tid)
            .map_err(|e| format!("revert_task: {e}"))
    })
    .await
    .map_err(|e| ApiError::from(format!("fh_revert_task task panicked: {e}")))?
    .map_err(ApiError::from)?;
    restore_todos_for_task(ctx, &task_id);
    ok(outcomes_to_payload(outcomes))
}

async fn fh_list_task_paths(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: ProjectTaskArg = parse(args)?;
    let (_canon, handle) = handle_for(ctx, &a.project_root)?;
    let task_id = a.task_id;
    let task_id_for_log = task_id.clone();
    let project_root_for_log = a.project_root.clone();
    let result: Result<Vec<String>, String> = tokio::task::spawn_blocking(move || {
        let paths = handle
            .history
            .list_task_paths(&task_id)
            .map_err(|e| format!("list_task_paths: {e}"))?;
        tracing::info!(
            target: "rustic::file_history",
            task = %task_id_for_log,
            project_root = %project_root_for_log,
            count = paths.len(),
            "fh_list_task_paths returned"
        );
        Ok(paths)
    })
    .await
    .map_err(|e| ApiError::from(format!("fh_list_task_paths task panicked: {e}")))?;
    ok(result?)
}

async fn fh_list_task_net_changes(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: ProjectTaskArg = parse(args)?;
    let (canon, handle) = handle_for(ctx, &a.project_root)?;
    let task_id = a.task_id;
    let state = ctx.state();

    // Live-disk diff while the task is actively running (real-time progress);
    // otherwise diff against the stored final_tree_oid.
    let task_is_running = state
        .agent
        .lock()
        .ok()
        .and_then(|agent| {
            agent
                .tasks
                .get(&task_id)
                .map(|t| matches!(t.info.status, rustic_agent::TaskStatus::Running))
        })
        .unwrap_or(false);

    let snapshot_count = state
        .db
        .lock()
        .ok()
        .and_then(|db| db.fh_list_snapshots_for_task(&task_id).ok())
        .map(|s| s.len())
        .unwrap_or(0);

    let project_root_for_log = a.project_root.clone();
    let task_id_for_log = task_id.clone();

    let result: Result<Vec<TaskNetChangePayload>, String> =
        tokio::task::spawn_blocking(move || {
            let rows: Vec<rustic_agent::TaskNetChange> = if task_is_running {
                handle
                    .history
                    .list_task_net_changes(&task_id)
                    .map_err(|e| format!("fh_list_task_net_changes: {e}"))?
            } else {
                handle
                    .history
                    .list_task_net_changes_final(&task_id)
                    .map_err(|e| format!("fh_list_task_net_changes_final: {e}"))?
            };

            tracing::info!(
                target: "rustic::file_history",
                task = %task_id_for_log,
                project_root = %project_root_for_log,
                task_is_running,
                snapshot_count,
                row_count = rows.len(),
                "fh_list_task_net_changes returned"
            );

            let proj_root = canon.clone();
            Ok(rows
                .into_iter()
                .map(|r| {
                    let abs = proj_root.join(&r.path);
                    let (is_dir, exists_on_disk) = match std::fs::metadata(&abs) {
                        Ok(md) => (md.is_dir(), true),
                        Err(_) => (false, false),
                    };
                    TaskNetChangePayload {
                        path: r.path,
                        kind: r.kind,
                        binary: r.binary,
                        additions: r.additions,
                        deletions: r.deletions,
                        anchor_message_id: r.anchor_message_id,
                        is_dir,
                        exists_on_disk,
                    }
                })
                .collect())
        })
        .await
        .map_err(|e| ApiError::from(format!("fh_list_task_net_changes task panicked: {e}")))?;
    ok(result?)
}

async fn fh_list_snapshots(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: TaskOnlyArg = parse(args)?;
    let task_id = a.task_id;
    let db = Arc::clone(&ctx.state().db);
    let result: Result<Vec<SnapshotRow>, String> = tokio::task::spawn_blocking(move || {
        let db = db.lock().map_err(|e| format!("db mutex poisoned: {e}"))?;
        let rows = db
            .fh_list_snapshots_for_task(&task_id)
            .map_err(|e| format!("fh_list_snapshots: {e}"))?;
        Ok(rows
            .into_iter()
            .map(|r| SnapshotRow {
                message_id: r.message_id,
                sequence: r.sequence,
                created_at: r.created_at,
            })
            .collect())
    })
    .await
    .map_err(|e| ApiError::from(format!("fh_list_snapshots task panicked: {e}")))?;
    ok(result?)
}

//! Host-side surface for the changed-files tracker.
//!
//! Owns the per-project `FileHistory` + `SweepWorker` registry so the heavy
//! state (blob store, SQLite handle, background tokio task) is built once per
//! project and reused across every `send_message` turn. Also exposes the
//! Tauri commands the UI calls to list snapshots, list a snapshot's changed
//! files, and revert.

use std::path::Path;
use std::sync::Arc;

use rustic_agent::{FileHistory, FileTrackedKind, SweepWorker, TaskNetChange, TodoItem};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

use crate::commands::agent::AgentTodoUpdatedEvent;
use crate::state::{AppState, FileHistoryHandle};

/// Shared between the three revert commands. Looks up the todo snapshot tied
/// to `message_id` (the same message_id the file tracker anchored at turn-start),
/// writes it back as the current list, and emits `agent-todo-updated` so the
/// UI panel refreshes. Silent no-op when the snapshot doesn't exist (old
/// tasks that predate this feature, or a turn that never opened a snapshot).
fn restore_todos_for_message(state: &AppState, app: &AppHandle, message_id: &str) {
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
    apply_restored_todos(state, app, &task_id, &todos_json);
}

/// Sibling of `restore_todos_for_message` for `revert_task` — uses the
/// EARLIEST snapshot in the task (its pre-task state).
fn restore_todos_for_task(state: &AppState, app: &AppHandle, task_id: &str) {
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
    apply_restored_todos(state, app, task_id, &todos_json);
}

fn apply_restored_todos(state: &AppState, app: &AppHandle, task_id: &str, todos_json: &str) {
    if let Ok(db) = state.db.lock() {
        if let Err(e) = db.set_task_todos(task_id, todos_json) {
            tracing::warn!(task_id, ?e, "set_task_todos during revert failed");
            return;
        }
    } else {
        return;
    }
    // Decode the JSON to TodoItem for the UI event payload. Fall back to an
    // empty list rather than skipping the emit — the frontend would otherwise
    // keep showing the post-revert-stale list.
    let todos: Vec<TodoItem> =
        serde_json::from_str(todos_json).unwrap_or_default();
    let _ = app.emit(
        "agent-todo-updated",
        AgentTodoUpdatedEvent {
            task_id: task_id.to_string(),
            todos,
        },
    );
}

/// Get-or-create a `FileHistoryHandle` for `project_root`. Lazily initializes
/// the blob store + tokio sweep worker the first time a project takes a turn.
///
/// `project_root` should be the canonicalized absolute path of the project's
/// worktree root. Same canonical form must be used on every call so the
/// registry hits the same map key.
pub fn get_or_create_handle(
    state: &AppState,
    app: &AppHandle,
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
    let app_data_dir =
        crate::app_paths::app_data_dir(app).map_err(|e| format!("app_data_dir: {e}"))?;

    // R.2: spin up the FS-watcher accumulator before FileHistory, so the
    // history's targeted-track path has the accumulator from the very first
    // open_snapshot. The watcher itself is created next; failure is non-
    // fatal — `FileHistory` simply falls back to full walks when no
    // accumulator is attached.
    let accumulator = Arc::new(rustic_agent::DirtyPathAccumulator::new(
        project_root.to_path_buf(),
    ));

    let history = Arc::new(
        FileHistory::new_with_accumulator(
            Arc::clone(&state.db),
            project_root.to_path_buf(),
            &app_data_dir,
            Some(Arc::clone(&accumulator)),
        )
        .map_err(|e| format!("FileHistory::new: {e}"))?,
    );

    // Watcher registration may fail on resource-constrained systems (Linux
    // inotify limit, network drives without poll fallback). We log and
    // continue — the sweep will revert to the libgit2-era full-walk path.
    let watcher = match rustic_agent::FileWatcher::spawn(
        project_root.to_path_buf(),
        Arc::clone(&accumulator),
    ) {
        Ok(w) => Some(Arc::new(w)),
        Err(e) => {
            tracing::warn!(
                root = %project_root.display(),
                ?e,
                "fs watcher registration failed — falling back to full-walk sweeps. \
                 On Linux this is often the per-user inotify watch limit; raise it \
                 with: echo fs.inotify.max_user_watches=524288 | sudo tee /etc/sysctl.d/40-watches.conf && sudo sysctl --system"
            );
            None
        }
    };

    // Sweep callback fires after each background sweep completes. We forward
    // it as a `TaskEvent::FileTracked` via the host's Tauri event surface so
    // the same UI path renders both edit-tool and bash-sweep changes.
    let app_for_cb = app.clone();
    let cb: rustic_agent::file_history::ChangeCallback =
        Arc::new(move |task_id: &str, message_id: &str, paths: &[String]| {
            let payload = FileTrackedPayload {
                task_id: task_id.to_string(),
                message_id: message_id.to_string(),
                kind: FileTrackedKindPayload::BashSweep,
                paths: paths.to_vec(),
            };
            let _ = app_for_cb.emit("agent-file-tracked", payload);
        });
    // SweepWorker spawns onto a tokio runtime — `send_message` is a sync
    // Tauri command running on the main thread with no ambient runtime, so we
    // route through Tauri's shared async runtime (which is tokio).
    let rt_handle = tauri::async_runtime::handle().inner().clone();
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
    // Race-tolerance: another caller may have inserted while we were
    // constructing. If so, drop our work and return theirs — the SweepWorker
    // task we just spawned will idle forever with no senders, which we
    // accept as a one-time cost (Tauri command timing makes this rare).
    let key = project_root.to_string_lossy().to_string();
    let entry = registry.entry(key).or_insert(handle);
    Ok(entry.clone())
}

/// Push a `TaskEvent::FileTracked` from edit-tool capture out to the UI as
/// `agent-file-tracked`. The host's existing event-forwarder loop calls this
/// whenever it sees a `FileTracked` task event.
pub fn forward_file_tracked(
    app: &AppHandle,
    task_id: String,
    message_id: String,
    kind: FileTrackedKind,
    paths: Vec<String>,
) {
    let payload = FileTrackedPayload {
        task_id,
        message_id,
        kind: match kind {
            FileTrackedKind::EditTool => FileTrackedKindPayload::EditTool,
            FileTrackedKind::BashSweep => FileTrackedKindPayload::BashSweep,
        },
        paths,
    };
    let _ = app.emit("agent-file-tracked", payload);
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileTrackedKindPayload {
    EditTool,
    BashSweep,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileTrackedPayload {
    pub task_id: String,
    pub message_id: String,
    pub kind: FileTrackedKindPayload,
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChangedFile {
    pub path: String,
    /// "modified" | "created" | "deleted" — derived from whether the snapshot
    /// has a pre-blob and whether the file exists on disk now.
    pub kind: &'static str,
    /// True when either side of the diff is non-utf8 / contains NUL bytes.
    /// Independent of `kind`: a created PDF is `kind: "created", binary: true`.
    pub binary: bool,
    pub additions: u32,
    pub deletions: u32,
}

/// List the files captured in a snapshot, with computed +/- line stats. Used
/// by the changed-files panel to show "+N -M" badges next to each path.
#[tauri::command]
pub async fn fh_list_files(
    state: State<'_, AppState>,
    app: AppHandle,
    project_root: String,
    message_id: String,
) -> Result<Vec<ChangedFile>, String> {
    let canon = std::path::PathBuf::from(&project_root)
        .canonicalize()
        .map_err(|e| format!("canonicalize {project_root}: {e}"))?;
    let handle = get_or_create_handle(&state, &app, &canon)?;
    tauri::async_runtime::spawn_blocking(move || {
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
    .map_err(|e| format!("fh_list_files task panicked: {e}"))?
}

#[derive(Debug, Clone, Serialize)]
pub struct FileDiffPayload {
    pub path: String,
    pub kind: &'static str,
    pub binary: bool,
    pub additions: u32,
    pub deletions: u32,
    pub unified: String,
}

/// Compute the unified diff for one file in a snapshot. Used when the user
/// clicks a row in the changed-files panel to open the diff in the editor.
#[tauri::command]
pub async fn fh_file_diff(
    state: State<'_, AppState>,
    app: AppHandle,
    project_root: String,
    message_id: String,
    path: String,
) -> Result<FileDiffPayload, String> {
    let canon = std::path::PathBuf::from(&project_root)
        .canonicalize()
        .map_err(|e| format!("canonicalize {project_root}: {e}"))?;
    let handle = get_or_create_handle(&state, &app, &canon)?;
    tauri::async_runtime::spawn_blocking(move || {
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
    .map_err(|e| format!("fh_file_diff task panicked: {e}"))?
}

fn outcomes_to_payload(
    outcomes: Vec<rustic_agent::RestoreOutcome>,
) -> Vec<RevertOutcome> {
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

fn plan_to_payload(
    plan: Vec<rustic_agent::RevertPlanEntry>,
) -> Vec<RevertPlanRow> {
    plan.into_iter()
        .map(|e| RevertPlanRow {
            path: e.path,
            action: e.action,
        })
        .collect()
}

/// Revert the worktree to the snapshot anchored at `message_id`. Returns the
/// list of paths that were actually touched on disk.
#[tauri::command]
pub async fn fh_revert(
    state: State<'_, AppState>,
    app: AppHandle,
    project_root: String,
    message_id: String,
) -> Result<Vec<RevertOutcome>, String> {
    let canon = std::path::PathBuf::from(&project_root)
        .canonicalize()
        .map_err(|e| format!("canonicalize {project_root}: {e}"))?;
    let handle = get_or_create_handle(&state, &app, &canon)?;
    let msg = message_id.clone();
    let outcomes = tauri::async_runtime::spawn_blocking(move || {
        handle.history.revert(&msg).map_err(|e| format!("revert: {e}"))
    })
    .await
    .map_err(|e| format!("fh_revert task panicked: {e}"))??;
    // Restore the todo list to its pre-turn state alongside the worktree.
    restore_todos_for_message(&state, &app, &message_id);
    Ok(outcomes_to_payload(outcomes))
}

/// Preview a `revert_from_message` — returns each path that would be touched
/// and the planned action ("restore" / "delete"). Used by the per-message
/// revert dialog so the user knows what they're agreeing to.
#[tauri::command]
pub async fn fh_plan_revert_from_message(
    state: State<'_, AppState>,
    app: AppHandle,
    project_root: String,
    message_id: String,
) -> Result<Vec<RevertPlanRow>, String> {
    let canon = std::path::PathBuf::from(&project_root)
        .canonicalize()
        .map_err(|e| format!("canonicalize {project_root}: {e}"))?;
    let handle = get_or_create_handle(&state, &app, &canon)?;
    tauri::async_runtime::spawn_blocking(move || {
        let plan = handle
            .history
            .plan_revert_from_message(&message_id)
            .map_err(|e| format!("plan_revert_from_message: {e}"))?;
        Ok(plan_to_payload(plan))
    })
    .await
    .map_err(|e| format!("fh_plan_revert_from_message task panicked: {e}"))?
}

/// Apply `revert_from_message`: revert the snapshot anchored at `message_id`
/// AND every later snapshot in the same task. Used by the per-message revert.
#[tauri::command]
pub async fn fh_revert_from_message(
    state: State<'_, AppState>,
    app: AppHandle,
    project_root: String,
    message_id: String,
) -> Result<Vec<RevertOutcome>, String> {
    let canon = std::path::PathBuf::from(&project_root)
        .canonicalize()
        .map_err(|e| format!("canonicalize {project_root}: {e}"))?;
    let handle = get_or_create_handle(&state, &app, &canon)?;
    let msg = message_id.clone();
    let outcomes = tauri::async_runtime::spawn_blocking(move || {
        handle
            .history
            .revert_from_message(&msg)
            .map_err(|e| format!("revert_from_message: {e}"))
    })
    .await
    .map_err(|e| format!("fh_revert_from_message task panicked: {e}"))??;
    // Same restore as fh_revert — message_id anchors the pre-turn snapshot,
    // and reverting "from this message forward" lands on that same pre-state.
    restore_todos_for_message(&state, &app, &message_id);
    Ok(outcomes_to_payload(outcomes))
}

/// Preview a `revert_task` — same shape as `fh_plan_revert_from_message` but
/// covering every snapshot in the task.
#[tauri::command]
pub async fn fh_plan_revert_task(
    state: State<'_, AppState>,
    app: AppHandle,
    project_root: String,
    task_id: String,
) -> Result<Vec<RevertPlanRow>, String> {
    let canon = std::path::PathBuf::from(&project_root)
        .canonicalize()
        .map_err(|e| format!("canonicalize {project_root}: {e}"))?;
    let handle = get_or_create_handle(&state, &app, &canon)?;
    tauri::async_runtime::spawn_blocking(move || {
        let plan = handle
            .history
            .plan_revert_task(&task_id)
            .map_err(|e| format!("plan_revert_task: {e}"))?;
        Ok(plan_to_payload(plan))
    })
    .await
    .map_err(|e| format!("fh_plan_revert_task task panicked: {e}"))?
}

/// Revert a single path to its pre-task state. Used by the per-file
/// "Revert" button in the Files panel of agent-tool-dock.
///
/// Restores `path` to whatever bytes were captured in the task's earliest
/// snapshot tree. If the path didn't exist in that tree (i.e. the task
/// freshly created it), the file is deleted on disk. The rest of the
/// task's edits are left intact — this is the surgical revert.
#[tauri::command]
pub async fn fh_revert_path(
    state: State<'_, AppState>,
    app: AppHandle,
    project_root: String,
    task_id: String,
    path: String,
) -> Result<Vec<RevertOutcome>, String> {
    let canon = std::path::PathBuf::from(&project_root)
        .canonicalize()
        .map_err(|e| format!("canonicalize {project_root}: {e}"))?;
    let handle = get_or_create_handle(&state, &app, &canon)?;
    let tid = task_id.clone();
    let target_path = path.clone();
    let outcomes = tauri::async_runtime::spawn_blocking(move || {
        handle
            .history
            .revert_path(&tid, &target_path)
            .map_err(|e| format!("revert_path: {e}"))
    })
    .await
    .map_err(|e| format!("fh_revert_path task panicked: {e}"))??;
    Ok(outcomes_to_payload(outcomes))
}

/// Revert every snapshot in the task. Used by the bottom-panel "Revert"
/// button — files only, chat history is left alone (the per-message revert
/// is the path that prunes chat).
#[tauri::command]
pub async fn fh_revert_task(
    state: State<'_, AppState>,
    app: AppHandle,
    project_root: String,
    task_id: String,
) -> Result<Vec<RevertOutcome>, String> {
    let canon = std::path::PathBuf::from(&project_root)
        .canonicalize()
        .map_err(|e| format!("canonicalize {project_root}: {e}"))?;
    let handle = get_or_create_handle(&state, &app, &canon)?;
    let tid = task_id.clone();
    let outcomes = tauri::async_runtime::spawn_blocking(move || {
        handle
            .history
            .revert_task(&tid)
            .map_err(|e| format!("revert_task: {e}"))
    })
    .await
    .map_err(|e| format!("fh_revert_task task panicked: {e}"))??;
    // Restore the todo list to its earliest snapshot (pre-task state).
    restore_todos_for_task(&state, &app, &task_id);
    Ok(outcomes_to_payload(outcomes))
}

/// Fast paths-only variant of `fh_list_task_net_changes` — returns just
/// the relative paths the task touched, without computing diff stats or
/// walking the live worktree. Suitable for the Files tab's initial
/// hydration, which needs the list rendered quickly without blocking the
/// task-open path.
///
/// Internally calls `FileHistory::list_task_paths`, which is pure tree-vs-
/// tree diff: completes in a few ms even for tasks with many snapshots.
#[tauri::command]
pub async fn fh_list_task_paths(
    state: State<'_, AppState>,
    app: AppHandle,
    project_root: String,
    task_id: String,
) -> Result<Vec<String>, String> {
    let canon = std::path::PathBuf::from(&project_root)
        .canonicalize()
        .map_err(|e| format!("canonicalize {project_root}: {e}"))?;
    let handle = get_or_create_handle(&state, &app, &canon)?;

    let task_id_for_log = task_id.clone();
    let project_root_for_log = project_root.clone();

    tauri::async_runtime::spawn_blocking(move || {
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
    .map_err(|e| format!("fh_list_task_paths task panicked: {e}"))?
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskNetChangePayload {
    pub path: String,
    pub kind: &'static str,
    pub binary: bool,
    pub additions: u32,
    pub deletions: u32,
    /// Earliest snapshot anchoring this path. Pass it back to `fh_file_diff`
    /// to render the cumulative (pre-task vs current) diff for this file.
    pub anchor_message_id: String,
    /// `true` when the path currently exists on disk as a directory. The
    /// shadow tree only stores blob paths, but the diff machinery can
    /// occasionally surface a path that the user has since rmdir'd /
    /// mkdir'd over with the same name. The Files panel uses this to
    /// render a folder icon and disable the click-to-diff / revert
    /// actions for the row.
    pub is_dir: bool,
    /// `true` when the path currently exists on disk (as a file OR a
    /// directory). The diff machinery uses the task's recorded final tree
    /// — captured at end-of-turn — so a file that the task created and
    /// the user has since deleted manually still surfaces as "created"
    /// in the diff. The Files panel filters those rows out so the user
    /// doesn't see ghost entries that produce "No changes to display"
    /// when clicked.
    pub exists_on_disk: bool,
}

/// Cumulative net change across an entire task. For each path the agent
/// touched, returns its kind/diff stats compared to the file's pre-task
/// state — the bottom-panel "Changed files" view uses this so a file
/// created-then-modified shows up as "created" (the net result), not
/// "modified" (the latest turn's local kind).
///
/// When the task is actively running we diff against live disk so the panel
/// updates in real-time as the agent edits. When the task is idle we diff
/// against the stored `final_tree_oid` captured at the end of the last turn,
/// so external edits made after the task finished don't appear in the panel.
#[tauri::command]
pub async fn fh_list_task_net_changes(
    state: State<'_, AppState>,
    app: AppHandle,
    project_root: String,
    task_id: String,
) -> Result<Vec<TaskNetChangePayload>, String> {
    let canon = std::path::PathBuf::from(&project_root)
        .canonicalize()
        .map_err(|e| format!("canonicalize {project_root}: {e}"))?;
    let handle = get_or_create_handle(&state, &app, &canon)?;

    // Use live-disk diff while the task is actively running so the panel
    // shows real-time progress. For any other state (Completed, Failed,
    // Cancelled, or not yet in-memory) use the stored final_tree_oid so
    // external edits after the task don't pollute the result.
    let task_is_running = state
        .agent
        .lock()
        .ok()
        .and_then(|agent| agent.tasks.get(&task_id).map(|t| {
            matches!(t.info.status, rustic_agent::TaskStatus::Running)
        }))
        .unwrap_or(false);

    // Snapshot-count probe runs on the calling task (we already hold `state`
    // here) so we don't have to clone the db Arc into spawn_blocking just for
    // a diagnostic. Logged below alongside the row count.
    let snapshot_count = state
        .db
        .lock()
        .ok()
        .and_then(|db| db.fh_list_snapshots_for_task(&task_id).ok())
        .map(|s| s.len())
        .unwrap_or(0);

    // A task with no recorded snapshots has no net changes to diff yet (it
    // hasn't taken a turn, or its history predates this feature). That's a
    // normal empty result, not an error — return early so the frontend simply
    // clears the panel instead of the diff helper erroring on every project
    // switch (the source of the `fh_list_task_net_changes failed` log spam).
    if snapshot_count == 0 {
        return Ok(Vec::new());
    }

    let project_root_for_log = project_root.clone();
    let task_id_for_log = task_id.clone();

    tauri::async_runtime::spawn_blocking(move || {
        let rows: Vec<TaskNetChange> = if task_is_running {
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

        // Diagnostic: pair this with the frontend
        // `[agent.loadTaskHistory] fetched` log to localise a
        // "Files tab empty after restart" report against either
        //   (a) snapshot_count == 0 → no rows in file_history_snapshots
        //       for this task. Either the task predates the gix migration
        //       (likely culprit for old chats) or the agent never wrote
        //       anything for it.
        //   (b) snapshot_count > 0 but row_count == 0 → snapshots exist
        //       but their baseline tree_oid is NULL (pre-gix rows have
        //       this; see migration 014). list_task_net_changes_final
        //       silently returns empty in that case.
        //   (c) snapshot_count > 0 and row_count > 0 → backend is fine;
        //       look upstream for why the frontend dropped them.
        tracing::info!(
            target: "rustic::file_history",
            task = %task_id_for_log,
            project_root = %project_root_for_log,
            task_is_running,
            snapshot_count,
            row_count = rows.len(),
            "fh_list_task_net_changes returned"
        );

        // Stat each path on disk to discover folders. Cheap (single stat
        // per row) and lets the frontend render folder rows distinctly
        // without a second round-trip. Stats off a canonicalised project
        // root so a path containing `..` can't escape.
        let proj_root = canon.clone();
        Ok(rows
            .into_iter()
            .map(|r| {
                let abs = proj_root.join(&r.path);
                // One stat call covers both is_dir and exists_on_disk —
                // the frontend uses these together to suppress ghost
                // entries (kind='created'/'modified' but the file is now
                // gone from disk) and render folder rows distinctly.
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
    .map_err(|e| format!("fh_list_task_net_changes task panicked: {e}"))?
}

/// List snapshots for a task in chronological (sequence ASC) order. The UI
/// uses this to map nth-user-message in the chat to nth-snapshot, which is
/// what the per-message revert button hangs off.
#[tauri::command]
pub async fn fh_list_snapshots(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<Vec<SnapshotRow>, String> {
    let db = state.db.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let db = db
            .lock()
            .map_err(|e| format!("db mutex poisoned: {e}"))?;
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
    .map_err(|e| format!("fh_list_snapshots task panicked: {e}"))?
}

#[derive(Debug, Clone, Serialize)]
pub struct SnapshotRow {
    pub message_id: String,
    pub sequence: i64,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RevertPlanRow {
    pub path: String,
    pub action: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct RevertOutcome {
    pub path: String,
    /// "rewritten" | "deleted" | "unchanged" | "failed"
    pub action: &'static str,
    /// When `action == "failed"`, the OS / IO error message that caused
    /// it (locked file, file-vs-directory mismatch, permission denied).
    /// Null/absent for the other actions to keep the payload small.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Run startup reconciliation for every project that already has blobs on
/// disk. Cheap; runs synchronously on the main thread.
///
/// CAUTION: this MUST NOT call `get_or_create_handle` — that path spawns a
/// `SweepWorker` tokio task, and Tauri's `setup` hook fires before any tokio
/// runtime is in scope. The reconcile pass only needs the `FileHistory`
/// methods that hit the DB + blob store, so we construct a temporary
/// instance, run the orphan sweep, and drop it without registering or
/// spawning anything.
pub fn reconcile_all_projects(
    state: &AppState,
    app: &AppHandle,
    project_roots: &[String],
) {
    let app_data_dir = match crate::app_paths::app_data_dir(app) {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(?e, "skip startup reconcile: app_data_dir resolution failed");
            return;
        }
    };
    for root in project_roots {
        let canon = match std::path::PathBuf::from(root).canonicalize() {
            Ok(p) => p,
            Err(e) => {
                tracing::debug!(root, ?e, "skip orphan reconcile: canonicalize failed");
                continue;
            }
        };
        // Construct a transient FileHistory (no worker, no registry insert).
        // Reconciliation is now an in-shadow reachability prune; no separate
        // disk blob store to scan.
        let history = match FileHistory::new(Arc::clone(&state.db), canon, &app_data_dir) {
            Ok(h) => h,
            Err(e) => {
                tracing::warn!(root, ?e, "skip orphan reconcile: FileHistory::new failed");
                continue;
            }
        };
        match history.reconcile_disk_orphans() {
            Ok(0) => {}
            Ok(n) => tracing::info!(root, removed = n, "reconciled orphan blobs"),
            Err(e) => tracing::warn!(root, ?e, "orphan reconciliation failed"),
        }
    }
}

/// One-shot cleanup of the legacy SHA-256 blob store left over by R.1's
/// clean-break migration. Runs once on startup; the second run is a no-op
/// because we drop a `.migrated-014` marker after a successful removal.
///
/// Emits `agent-file-history-migrated` exactly once — the frontend listens
/// for this event to show a "your file history was upgraded" banner. We
/// only emit if there was actually something to clean up; fresh installs
/// stay silent.
///
/// All errors are logged but never propagated — a stale legacy directory
/// is harmless beyond disk usage, and we don't want a corrupted file_history
/// dir to block startup.
pub fn cleanup_legacy_blob_store(app: &AppHandle) {
    let app_data_dir = match crate::app_paths::app_data_dir(app) {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(?e, "skip legacy blob cleanup: app_data_dir resolution failed");
            return;
        }
    };
    let file_history_dir = app_data_dir.join("file-history");
    let marker = file_history_dir.join(".migrated-014");
    let legacy_blobs = file_history_dir.join("blobs");

    if marker.exists() {
        return;
    }
    if !legacy_blobs.exists() {
        // Either a fresh install or a previously-cleaned environment.
        // Drop the marker so we don't keep checking on every launch.
        if file_history_dir.exists() {
            if let Err(e) = std::fs::write(&marker, b"") {
                tracing::debug!(?e, "could not write migration marker; will retry next launch");
            }
        }
        return;
    }

    // Walk-count for the log line so we know roughly how much was reclaimed.
    let (file_count, total_bytes) = count_legacy_blobs(&legacy_blobs);
    match std::fs::remove_dir_all(&legacy_blobs) {
        Ok(()) => {
            tracing::info!(
                files = file_count,
                bytes = total_bytes,
                "removed legacy SHA-256 blob store (R.1 clean-break migration)"
            );
            if let Err(e) = std::fs::write(&marker, b"") {
                tracing::debug!(?e, "could not write migration marker after cleanup");
            }
            let payload = FileHistoryMigratedPayload {
                legacy_files_removed: file_count,
                legacy_bytes_removed: total_bytes,
            };
            let _ = app.emit("agent-file-history-migrated", payload);
        }
        Err(e) => {
            tracing::warn!(
                ?e,
                "legacy blob cleanup failed; will retry on next launch"
            );
        }
    }
}

/// Best-effort stat-walk of the legacy blob layout
/// (`<root>/<aa>/<sha256...>`) to surface a "freed N files / M bytes"
/// line in the log + the migration event. Failures during the walk
/// silently fall back to zero — the actual cleanup runs regardless.
fn count_legacy_blobs(root: &std::path::Path) -> (u64, u64) {
    let mut files = 0u64;
    let mut bytes = 0u64;
    let bucket_iter = match std::fs::read_dir(root) {
        Ok(it) => it,
        Err(_) => return (files, bytes),
    };
    for bucket in bucket_iter.flatten() {
        let blob_iter = match std::fs::read_dir(bucket.path()) {
            Ok(it) => it,
            Err(_) => continue,
        };
        for blob in blob_iter.flatten() {
            if let Ok(md) = blob.metadata() {
                if md.is_file() {
                    files += 1;
                    bytes += md.len();
                }
            }
        }
    }
    (files, bytes)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileHistoryMigratedPayload {
    pub legacy_files_removed: u64,
    pub legacy_bytes_removed: u64,
}


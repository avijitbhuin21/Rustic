//! Worktree-per-task + merge queue commands (docs/plans/worktree-merge-queue.md).
//!
//! Thin wrappers over `rustic_app::worktree` — all real logic lives in the
//! shared crate so the desktop shell exposes byte-identical behaviour.

use std::sync::Arc;

use serde::Deserialize;
use serde_json::Value;

use rustic_app::context::{AppContext, EventEmitter};
use rustic_app::sync_ext::MutexExt;
use rustic_app::worktree;

use crate::api::{ok, ApiError};
use crate::context::ServerContext;

pub async fn dispatch(
    ctx: &ServerContext,
    command: &str,
    args: &Value,
) -> Option<Result<Value, ApiError>> {
    Some(match command {
        "worktree_get" => worktree_get(ctx, args),
        "worktree_list" => worktree_list(ctx, args),
        "worktree_merge" => worktree_merge(ctx, args),
        "worktree_discard" => worktree_discard(ctx, args).await,
        "worktree_review_files" => worktree_review_files(ctx, args).await,
        "worktree_file_diff" => worktree_file_diff(ctx, args).await,
        "worktree_conflict_prompt" => worktree_conflict_prompt(ctx, args).await,
        "worktree_reconcile" => worktree_reconcile(ctx, args).await,
        "worktree_get_settings" => worktree_get_settings(ctx),
        "worktree_set_settings" => worktree_set_settings(ctx, args),
        _ => return None,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TaskIdArg {
    task_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectIdArg {
    project_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileDiffArg {
    task_id: String,
    path: String,
}

fn emitter_of(ctx: &ServerContext) -> Arc<dyn EventEmitter> {
    Arc::new(ctx.clone())
}

fn worktree_get_settings(ctx: &ServerContext) -> Result<Value, ApiError> {
    ok(worktree::get_worktree_settings(&ctx.state().db))
}

fn worktree_set_settings(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let settings = args.get("settings").unwrap_or(args);
    worktree::set_worktree_settings(&ctx.state().db, settings)?;
    ok(serde_json::json!(null))
}

fn worktree_get(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: TaskIdArg = crate::api::parse(args)?;
    let row = ctx
        .state()
        .db
        .lock_safe()
        .wt_get(&a.task_id)
        .map_err(|e| e.to_string())?;
    ok(row)
}

fn worktree_list(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: ProjectIdArg = crate::api::parse(args)?;
    let rows = ctx
        .state()
        .db
        .lock_safe()
        .wt_list_for_project(&a.project_id)
        .map_err(|e| e.to_string())?;
    ok(rows)
}

fn worktree_merge(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: TaskIdArg = crate::api::parse(args)?;
    worktree::enqueue_merge(ctx.state(), &emitter_of(ctx), &a.task_id)?;
    ok(serde_json::json!(null))
}

async fn worktree_discard(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: TaskIdArg = crate::api::parse(args)?;
    let db = ctx.state().db.clone();
    let task_id = a.task_id.clone();
    let data_dir = ctx.data_dir();
    let root = tokio::task::spawn_blocking(move || {
        worktree::discard_task_worktree(&db, &data_dir, &task_id)
    })
    .await
    .map_err(|e| format!("discard worker panicked: {e}"))??;
    ctx.emit_json(
        worktree::WORKTREE_EVENT,
        serde_json::json!({ "task_id": a.task_id, "state": "discarded" }),
    );
    if let Some(root) = root {
        ctx.state().merge_queues.clone().ensure_worker(
            ctx.state().db.clone(),
            emitter_of(ctx),
            root,
        );
    }
    ok(serde_json::json!(null))
}

async fn worktree_review_files(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: TaskIdArg = crate::api::parse(args)?;
    let row = ctx
        .state()
        .db
        .lock_safe()
        .wt_get(&a.task_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("no worktree for task {}", a.task_id))?;
    let diffs = tokio::task::spawn_blocking(move || worktree::review_diff(&row))
        .await
        .map_err(|e| format!("review worker panicked: {e}"))??;
    ok(diffs)
}

async fn worktree_file_diff(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: FileDiffArg = crate::api::parse(args)?;
    let row = ctx
        .state()
        .db
        .lock_safe()
        .wt_get(&a.task_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("no worktree for task {}", a.task_id))?;
    let path = a.path.clone();
    let diff = tokio::task::spawn_blocking(move || worktree::review_file_diff(&row, &path))
        .await
        .map_err(|e| format!("diff worker panicked: {e}"))??;
    ok(diff)
}

async fn worktree_conflict_prompt(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: TaskIdArg = crate::api::parse(args)?;
    let row = ctx
        .state()
        .db
        .lock_safe()
        .wt_get(&a.task_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("no worktree for task {}", a.task_id))?;
    let prompt = tokio::task::spawn_blocking(move || worktree::conflict_resolution_prompt(&row))
        .await
        .map_err(|e| format!("prompt worker panicked: {e}"))??;
    ok(prompt)
}

async fn worktree_reconcile(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: TaskIdArg = crate::api::parse(args)?;
    let row = ctx
        .state()
        .db
        .lock_safe()
        .wt_get(&a.task_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("no worktree for task {}", a.task_id))?;
    let plan = tokio::task::spawn_blocking(move || worktree::reconcile_plan(&row))
        .await
        .map_err(|e| format!("reconcile worker panicked: {e}"))??;
    match plan {
        worktree::Reconcile::AgentPrompt(prompt) => {
            ok(serde_json::json!({ "action": "resolve", "prompt": prompt }))
        }
        worktree::Reconcile::AlreadyClean => {
            worktree::enqueue_merge(ctx.state(), &emitter_of(ctx), &a.task_id)?;
            ok(serde_json::json!({ "action": "requeued" }))
        }
    }
}

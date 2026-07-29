//! Server routes for the external CLI agents (Claude Code, Codex, `agy`).
//!
//! Mirrors `src-tauri/src/commands/external_agents.rs` route-for-route: all the
//! launch logic lives in `rustic_app::external_agents`, and this module only
//! opens the PTY and writes the prepared command line into it.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

use rustic_app::context::{AppContext, EventEmitterExt};
use rustic_app::external_agents::service::{self, SpawnTarget};
use rustic_app::external_agents::{
    annotate_updates, detect_agents, AgentKind, AgentPermissionMode, ShellKind,
};
use rustic_app::sync_ext::MutexExt;

use crate::api::{ok, parse, ApiError};
use crate::commands::terminal::{
    emit_terminal_list_changed, preferred_agent_shell, spawn_output_reader, spawn_session_monitor,
    wait_for_shell_output,
};
use crate::context::ServerContext;

/// Result of starting (or resuming) an external agent.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SpawnedAgent {
    session_id: u64,
    row_id: String,
    label: String,
}

pub async fn dispatch(
    ctx: &ServerContext,
    command: &str,
    args: &Value,
) -> Option<Result<Value, ApiError>> {
    Some(match command {
        "detect_external_agents" => detect_external_agents().await,
        "spawn_external_agent" => spawn_external_agent(ctx, args).await,
        "resume_external_agent" => resume_external_agent(ctx, args).await,
        "list_external_agent_sessions" => list_external_agent_sessions(ctx, args),
        "delete_external_agent_session" => delete_external_agent_session(ctx, args),
        _ => return None,
    })
}

async fn detect_external_agents() -> Result<Value, ApiError> {
    let mut agents = tokio::task::spawn_blocking(detect_agents)
        .await
        .map_err(|e| format!("detect_external_agents task panicked: {e}"))?;
    annotate_updates(&mut agents).await;
    ok(agents)
}

async fn spawn_external_agent(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct A {
        agent: String,
        project_id: String,
        prompt: Option<String>,
        permission_mode: Option<String>,
    }
    let a: A = parse(args)?;
    let kind =
        AgentKind::from_str(&a.agent).ok_or_else(|| format!("unknown agent `{}`", a.agent))?;
    let permission = a
        .permission_mode
        .as_deref()
        .map(AgentPermissionMode::from_wire)
        .unwrap_or_default();
    launch(
        ctx,
        SpawnTarget::New {
            agent: kind,
            project_id: a.project_id,
            prompt: a.prompt,
            permission,
        },
    )
    .await
}

async fn resume_external_agent(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct A {
        session_row_id: String,
        permission_mode: Option<String>,
    }
    let a: A = parse(args)?;
    let permission = a
        .permission_mode
        .as_deref()
        .map(AgentPermissionMode::from_wire)
        .unwrap_or_default();
    launch(
        ctx,
        SpawnTarget::Resume {
            session_row_id: a.session_row_id,
            permission,
        },
    )
    .await
}

fn list_external_agent_sessions(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct A {
        project_id: String,
        agent: Option<String>,
        limit: Option<i64>,
    }
    let a: A = parse(args)?;
    let agent = match a.agent.as_deref() {
        Some(name) => {
            Some(AgentKind::from_str(name).ok_or_else(|| format!("unknown agent `{name}`"))?)
        }
        None => None,
    };
    let db = ctx.state().db.lock_safe();
    let rows = service::list_sessions(&db, &a.project_id, agent, a.limit.unwrap_or(100))
        .map_err(|e| e.to_string())?;
    ok(rows)
}

fn delete_external_agent_session(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct A {
        session_row_id: String,
        purge_transcript: Option<bool>,
    }
    let a: A = parse(args)?;
    let db = ctx.state().db.lock_safe();
    service::delete_session(&db, &a.session_row_id, a.purge_transcript.unwrap_or(false))
        .map_err(|e| e.to_string())?;
    ok(serde_json::json!(null))
}

/// Shared body of spawn/resume: prepare, open a PTY, type the command in.
///
/// Offloaded to a blocking worker because opening a PTY and waiting for the
/// shell's first output takes up to ~1.5s, which must not stall the async
/// request handler.
async fn launch(ctx: &ServerContext, target: SpawnTarget) -> Result<Value, ApiError> {
    let ctx = ctx.clone();
    let spawned = tokio::task::spawn_blocking(move || launch_blocking(&ctx, target))
        .await
        .map_err(|e| format!("spawn_external_agent task panicked: {e}"))??;
    ok(spawned)
}

fn launch_blocking(ctx: &ServerContext, target: SpawnTarget) -> Result<SpawnedAgent, String> {
    let data_dir = ctx.data_dir();
    let shell = preferred_agent_shell();
    let shell_kind = ShellKind::from_program(shell.as_deref());

    let prep = {
        let db = ctx.state().db.lock_safe();
        let lookup = |project_id: &str| -> anyhow::Result<PathBuf> {
            let project = db
                .get_project(project_id)?
                .ok_or_else(|| anyhow::anyhow!("project {project_id} not found"))?;
            Ok(PathBuf::from(project.root_path))
        };
        service::prepare(&db, &data_dir, lookup, target, shell_kind).map_err(|e| e.to_string())?
    };

    // `is_agent: false` — this is the user's terminal, so it keeps its tab when
    // the CLI exits and is never idle-reclaimed.
    let (info, reader, buffer, emulator, child) = {
        let mut manager = ctx.state().terminal_manager.lock_safe();
        manager
            .create_session(
                prep.cwd.clone(),
                prep.label.clone(),
                false,
                shell.clone(),
                None,
                &prep.env,
            )
            .map_err(|e| e.to_string())?
    };

    let session_id = info.id;
    let poll_buffer = Arc::clone(&buffer);
    spawn_output_reader(ctx.clone(), session_id, reader, buffer, emulator);
    spawn_session_monitor(ctx.clone(), session_id, child, false, info.pid);

    wait_for_shell_output(&poll_buffer, Duration::from_millis(1500));

    let mut line = String::from(shell_kind.launch_prelude());
    line.push_str(&prep.command_line);
    line.push('\r');
    {
        let mut manager = ctx.state().terminal_manager.lock_safe();
        manager
            .write_session(session_id, line.as_bytes())
            .map_err(|e| e.to_string())?;
    }

    service::start_capture(
        Arc::clone(&ctx.state().db),
        Arc::new(ctx.clone()),
        &data_dir,
        &prep,
    );

    tracing::info!(
        target: "rustic::external_agents",
        agent = prep.agent.as_str(),
        session_id,
        row_id = %prep.row_id,
        "spawned external CLI agent"
    );

    emit_terminal_list_changed(ctx);
    ctx.emit(service::SESSIONS_CHANGED_EVENT, ());

    Ok(SpawnedAgent {
        session_id,
        row_id: prep.row_id,
        label: prep.label,
    })
}

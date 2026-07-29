//! Desktop commands for the external CLI agents (Claude Code, Codex, `agy`).
//!
//! Thin host layer: every decision about how an agent is launched lives in
//! `rustic_app::external_agents`, and this module only opens the PTY and writes
//! the prepared command line into it. The matching server routes are in
//! `rustic-server/src/commands/external_agents.rs`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use rustic_app::external_agents::service::{self, SpawnTarget};
use rustic_app::external_agents::{
    annotate_updates, detect_agents, AgentKind, AgentPermissionMode, DetectedAgent, ShellKind,
};
use rustic_app::{AppState, MutexExt};
use rustic_db::ExternalAgentSessionRow;
use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::app_paths::app_data_dir;
use crate::commands::agent_terminals::{preferred_agent_shell, wait_for_shell_output};
use crate::commands::terminal::{
    emit_terminal_list_changed, spawn_output_reader, spawn_session_monitor,
};
use crate::transport::TauriEmitter;

/// Result of starting (or resuming) an external agent.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpawnedAgent {
    /// PTY session id — what the frontend opens as a terminal tab.
    pub session_id: u64,
    /// `external_agent_sessions.id` for the conversation behind that tab.
    pub row_id: String,
    pub label: String,
}

/// Which of the supported CLI agents are installed on this machine.
#[tauri::command]
pub async fn detect_external_agents() -> Result<Vec<DetectedAgent>, String> {
    let mut agents = tauri::async_runtime::spawn_blocking(detect_agents)
        .await
        .map_err(|e| format!("detect_external_agents task panicked: {e}"))?;
    annotate_updates(&mut agents).await;
    Ok(agents)
}

#[tauri::command]
pub async fn spawn_external_agent(
    app: AppHandle,
    agent: String,
    project_id: String,
    prompt: Option<String>,
    permission_mode: Option<String>,
) -> Result<SpawnedAgent, String> {
    let kind = AgentKind::from_str(&agent).ok_or_else(|| format!("unknown agent `{agent}`"))?;
    let permission = permission_mode
        .as_deref()
        .map(AgentPermissionMode::from_wire)
        .unwrap_or_default();
    launch(
        app,
        SpawnTarget::New {
            agent: kind,
            project_id,
            prompt,
            permission,
        },
    )
    .await
}

#[tauri::command]
pub async fn resume_external_agent(
    app: AppHandle,
    session_row_id: String,
    permission_mode: Option<String>,
) -> Result<SpawnedAgent, String> {
    let permission = permission_mode
        .as_deref()
        .map(AgentPermissionMode::from_wire)
        .unwrap_or_default();
    launch(
        app,
        SpawnTarget::Resume {
            session_row_id,
            permission,
        },
    )
    .await
}

#[tauri::command]
pub async fn list_external_agent_sessions(
    state: State<'_, AppState>,
    project_id: String,
    agent: Option<String>,
    limit: Option<i64>,
) -> Result<Vec<ExternalAgentSessionRow>, String> {
    let agent = match agent.as_deref() {
        Some(a) => Some(AgentKind::from_str(a).ok_or_else(|| format!("unknown agent `{a}`"))?),
        None => None,
    };
    let db = state.db.lock_safe();
    service::list_sessions(&db, &project_id, agent, limit.unwrap_or(100)).map_err(|e| e.to_string())
}

/// Forget a conversation. With `purge_transcript` (what the UI sends) the CLI's
/// own transcript is deleted too, so the session also disappears from that
/// tool's native resume picker.
#[tauri::command]
pub async fn delete_external_agent_session(
    state: State<'_, AppState>,
    session_row_id: String,
    purge_transcript: Option<bool>,
) -> Result<(), String> {
    let db = state.db.lock_safe();
    service::delete_session(&db, &session_row_id, purge_transcript.unwrap_or(false))
        .map_err(|e| e.to_string())
}

/// Shared body of spawn/resume: prepare, open a PTY, type the command in.
///
/// Runs on a blocking worker because opening a PTY and waiting for the shell's
/// first output takes up to ~1.5s, and Tauri would otherwise run it on the main
/// thread and freeze the window.
async fn launch(app: AppHandle, target: SpawnTarget) -> Result<SpawnedAgent, String> {
    let data_dir = app_data_dir(&app).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || launch_blocking(app, data_dir, target))
        .await
        .map_err(|e| format!("spawn_external_agent task panicked: {e}"))?
}

fn launch_blocking(
    app: AppHandle,
    data_dir: PathBuf,
    target: SpawnTarget,
) -> Result<SpawnedAgent, String> {
    let state = app.state::<AppState>();
    let shell = preferred_agent_shell();
    let shell_kind = ShellKind::from_program(shell.as_deref());

    let prep = {
        let db = state.db.lock_safe();
        let lookup = |project_id: &str| -> anyhow::Result<PathBuf> {
            let project = db
                .get_project(project_id)?
                .ok_or_else(|| anyhow::anyhow!("project {project_id} not found"))?;
            Ok(PathBuf::from(project.root_path))
        };
        service::prepare(&db, &data_dir, lookup, target, shell_kind).map_err(|e| e.to_string())?
    };

    // `is_agent: false` — this is the user's terminal, so it keeps its tab (with
    // frozen scrollback) when the CLI exits and is never idle-reclaimed.
    let (info, reader, buffer, emulator, child) = {
        let mut manager = state.terminal_manager.lock_safe();
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
    spawn_output_reader(app.clone(), session_id, reader, buffer, emulator);
    spawn_session_monitor(app.clone(), session_id, child, false, info.pid);

    // Don't type into a shell that hasn't finished starting — early writes get
    // eaten (see `wait_for_shell_output`).
    wait_for_shell_output(&poll_buffer, Duration::from_millis(1500));

    // Prelude and command share one input line so the shell's echo of the
    // command is wiped along with the banner (see `launch_prelude`).
    let mut line = String::from(shell_kind.launch_prelude());
    line.push_str(&prep.command_line);
    line.push('\r');
    {
        let mut manager = state.terminal_manager.lock_safe();
        manager
            .write_session(session_id, line.as_bytes())
            .map_err(|e| e.to_string())?;
    }

    service::start_capture(
        Arc::clone(&state.db),
        Arc::new(TauriEmitter::new(app.clone())),
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

    emit_terminal_list_changed(&app);
    app.emit_or_log(service::SESSIONS_CHANGED_EVENT);

    Ok(SpawnedAgent {
        session_id,
        row_id: prep.row_id,
        label: prep.label,
    })
}

/// Tiny convenience so the two emit sites above read the same way.
trait EmitOrLog {
    fn emit_or_log(&self, event: &str);
}

impl EmitOrLog for AppHandle {
    fn emit_or_log(&self, event: &str) {
        use tauri::Emitter;
        if let Err(e) = self.emit(event, ()) {
            tracing::warn!(target: "rustic::external_agents", "emit {event} failed: {e}");
        }
    }
}

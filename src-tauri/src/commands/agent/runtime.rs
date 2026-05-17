//! Runtime control commands: cost, abort, permission/question responses,
//! sensitive-file approvals, model switching, subagent records.

use crate::state::AppState;
use rustic_agent::{
    is_harness_provider_key, ContentBlock, HarnessKind, Message, NativePermissionDecision,
    PermissionDecision, Role, TaskCost, TaskStatus,
};
use rustic_db::SubagentRecord;
use std::sync::atomic::Ordering;
use tauri::{AppHandle, Emitter, State};
use super::{AgentModelSwitchedEvent, AgentStatusEvent};

#[tauri::command]
pub fn get_task_cost(state: State<'_, AppState>, task_id: String) -> Result<TaskCost, String> {
    let map = state.task_costs.lock().map_err(|e| e.to_string())?;
    Ok(map.get(&task_id).cloned().unwrap_or_default())
}

/// List the task IDs that currently have a live harness CLI session in the
/// registry. Used by the agent panel to render a per-project "N agents
/// active" banner and a global counter (plan §B.6 / §B.14). Lightweight —
/// just a snapshot of the registry's keys.
#[tauri::command]
pub async fn harness_active_task_ids(
    state: State<'_, AppState>,
) -> Result<Vec<String>, String> {
    Ok(state.harness_registry.task_ids().await)
}

/// Broadcast that a follow-up message was queued mid-turn so other windows
/// can mirror the queue. `preview` is truncated to avoid leaking full content.
#[tauri::command]
pub fn notify_input_queued(
    app: AppHandle,
    task_id: String,
    preview: String,
    image_count: u32,
    queue_depth: u32,
) -> Result<(), String> {
    let _ = app.emit(
        "agent-input-queued",
        AgentInputQueuedEvent {
            task_id,
            preview,
            image_count,
            queue_depth,
        },
    );
    Ok(())
}

/// Broadcast that the queued follow-ups for `task_id` were just flushed
/// to the running CLI / executor as the next user turn (plan §14, §B.9).
/// `count` is how many queued entries were merged.
#[tauri::command]
pub fn notify_input_delivered(
    app: AppHandle,
    task_id: String,
    count: u32,
) -> Result<(), String> {
    let _ = app.emit(
        "agent-input-delivered",
        AgentInputDeliveredEvent { task_id, count },
    );
    Ok(())
}

#[derive(Clone, serde::Serialize)]
struct AgentInputQueuedEvent {
    task_id: String,
    preview: String,
    image_count: u32,
    queue_depth: u32,
}

#[derive(Clone, serde::Serialize)]
struct AgentInputDeliveredEvent {
    task_id: String,
    count: u32,
}

#[tauri::command]
pub fn get_subagent_records(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<Vec<SubagentRecord>, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    db.get_subagent_records_for_task(&task_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn abort_task(app: AppHandle, state: State<'_, AppState>, task_id: String) -> Result<(), String> {
    // Flip the cancellation token first (cheap, synchronous). For native
    // tasks this is the whole story — the executor checks it between
    // provider calls. For harness tasks the token is also checked between
    // events, but the CLI is usually mid-stream and won't emit another
    // event until its current turn finishes; we have to send an explicit
    // `interrupt` control_request to actually unblock it.
    let agent = state.agent.lock().unwrap();
    let token = agent
        .cancellation_tokens
        .get(&task_id)
        .cloned()
        .ok_or_else(|| format!("No running task found: {}", task_id))?;
    token.store(true, Ordering::SeqCst);

    // Determine if this is a harness task before releasing the lock so we
    // don't have to relock for the registry lookup.
    let is_harness = agent
        .tasks
        .get(&task_id)
        .map(|t| is_harness_provider_key(&t.info.provider_type))
        .unwrap_or(false);
    drop(agent);

    // Emit the Cancelled status immediately so the UI updates without
    // waiting for the worker thread to finish draining events.
    let _ = app.emit(
        "agent-task-status",
        AgentStatusEvent {
            task_id: task_id.clone(),
            status: TaskStatus::Cancelled,
        },
    );

    if is_harness {
        // Send the CLI an interrupt envelope on a worker thread so the
        // Tauri command returns immediately. The CLI normally aborts its
        // turn loop and emits a `result` envelope, after which the harness
        // event loop sees `TurnComplete` and tears the registry slot down.
        //
        // Escalation ladder (plan §13.5): if 2 s after the interrupt the
        // session is still in the registry, fall through to a hard kill.
        // SIGINT-style escalation isn't reliable on Windows when the child
        // is reached via `cmd /C`, so we go straight to TerminateProcess
        // (which is what `shutdown()` does via the Job Object).
        let registry = state.harness_registry.clone();
        let task_id_for_async = task_id.clone();
        std::thread::spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    tracing::error!(error = %e, "abort_task: tokio init failed");
                    return;
                }
            };
            rt.block_on(async move {
                let Some(session) = registry.get(&task_id_for_async).await else {
                    return;
                };
                if let Err(e) = session.interrupt().await {
                    tracing::warn!(
                        task = %task_id_for_async,
                        error = %e,
                        "harness interrupt write failed (CLI may be unresponsive)"
                    );
                }
                drop(session);

                // Wait up to 2 s for the CLI to die naturally (the harness
                // event loop calls `registry.remove` on crash detection).
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;

                if let Some(still_alive) = registry.remove(&task_id_for_async).await {
                    tracing::warn!(
                        task = %task_id_for_async,
                        "harness CLI did not honour interrupt within 2s — hard-killing"
                    );
                    if let Err(e) = still_alive.shutdown().await {
                        tracing::warn!(
                            task = %task_id_for_async,
                            error = %e,
                            "force shutdown failed"
                        );
                    }
                }
            });
        });
    }

    Ok(())
}

/// Handle permission card response. `decision` ("accept"/"acceptForSession"/"deny")
/// wins over legacy `approved` bool. Harness tasks route through `HarnessRegistry`;
/// native tasks through `permission_broker`.
#[tauri::command]
pub fn respond_to_permission(
    app: AppHandle,
    state: State<'_, AppState>,
    task_id: String,
    request_id: String,
    approved: Option<bool>,
    decision: Option<String>,
) -> Result<(), String> {
    let parsed_decision = match decision.as_deref() {
        Some("accept") => Some(PermissionDecision::Accept),
        Some("acceptForSession") => Some(PermissionDecision::AcceptForSession),
        Some("deny") => Some(PermissionDecision::Deny),
        Some(other) => return Err(format!("Unknown permission decision: {other}")),
        None => None,
    };

    let is_harness_task = {
        let agent = state.agent.lock().map_err(|e| e.to_string())?;
        agent
            .tasks
            .get(&task_id)
            .map(|t| is_harness_provider_key(&t.info.provider_type))
            .unwrap_or(false)
    };

    if is_harness_task {
        // Resolve the decision: explicit `decision` wins; otherwise fall back
        // to the legacy bool. We default to `Deny` if neither is present so
        // a buggy caller can't accidentally auto-allow.
        let final_decision = parsed_decision.clone().unwrap_or_else(|| match approved {
            Some(true) => PermissionDecision::Accept,
            _ => PermissionDecision::Deny,
        });

        let registry = state.harness_registry.clone();
        // Releasing the lock before the await — we're going to call into
        // the harness session, which writes to the CLI's stdin, and we
        // don't want to hold the agent state mutex across that I/O.
        let task_id_for_async = task_id.clone();
        let app_for_async = app.clone();
        // Run on a worker so we don't block the Tauri command thread.
        std::thread::spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    tracing::error!(error = %e, "respond_to_permission: tokio init failed");
                    return;
                }
            };
            rt.block_on(async move {
                let Some(session) = registry.get(&task_id_for_async).await else {
                    let _ = app_for_async; // silence unused if we don't surface
                    tracing::warn!(
                        task = %task_id_for_async,
                        "respond_to_permission: no live harness session for task"
                    );
                    return;
                };
                if session.kind() != HarnessKind::ClaudeCode {
                    tracing::warn!(
                        task = %task_id_for_async,
                        "respond_to_permission: non-ClaudeCode harness not yet supported"
                    );
                    return;
                }
                if let Err(e) = session
                    .respond_to_permission(request_id.clone(), final_decision)
                    .await
                {
                    tracing::error!(
                        task = %task_id_for_async,
                        error = %e,
                        "respond_to_permission failed"
                    );
                }
            });
        });
        return Ok(());
    }

    // Native path: the broker now understands the three-state decision
    // directly (plan §B.3). `AcceptForSession` records the call's signature
    // in the per-task allowlist so subsequent matching ops auto-allow.
    let native_decision = match (parsed_decision, approved) {
        (Some(PermissionDecision::Accept), _) => NativePermissionDecision::Accept,
        (Some(PermissionDecision::AcceptForSession), _) => {
            NativePermissionDecision::AcceptForSession
        }
        (Some(PermissionDecision::Deny), _) => NativePermissionDecision::Deny,
        (None, Some(true)) => NativePermissionDecision::Accept,
        (None, Some(false)) => NativePermissionDecision::Deny,
        (None, None) => return Err("Either `approved` or `decision` is required".into()),
    };

    let agent = state.agent.lock().unwrap();
    agent
        .permission_broker
        .respond_with_decision(&request_id, native_decision);
    Ok(())
}

#[tauri::command]
pub fn set_task_sensitive_access(
    state: State<'_, AppState>,
    task_id: String,
    allowed: bool,
) -> Result<(), String> {
    let mut agent = state.agent.lock().unwrap();
    let task = agent
        .tasks
        .get_mut(&task_id)
        .ok_or_else(|| format!("Task not found: {}", task_id))?;
    task.sensitive_files_allowed = allowed;
    // Update shared permissions so the running executor sees the change immediately
    if let Some(ref shared) = task.shared_permissions {
        shared.set_sensitive_files_allowed(allowed);
    }
    Ok(())
}

/// P0.3: flip the per-task plan-mode flag. The flag is read at the top of
/// the next `send_message` and snapshot-captured into `ToolContext` for
/// the duration of that turn — mid-turn toggling is intentionally not
/// honoured (the running executor already holds its `is_plan_mode` view).
/// The UI should disable the toggle while the task is `Running` to keep
/// behaviour predictable; the backend doesn't enforce that gate (it just
/// won't take effect until the next user message).
#[tauri::command]
pub fn set_task_plan_mode(
    state: State<'_, AppState>,
    task_id: String,
    enabled: bool,
) -> Result<(), String> {
    let mut agent = state.agent.lock().unwrap();
    let task = agent
        .tasks
        .get_mut(&task_id)
        .ok_or_else(|| format!("Task not found: {}", task_id))?;
    task.is_plan_mode = enabled;
    Ok(())
}

/// Enable/disable goal-loop mode. Next `send_message` routes through
/// `run_goal_loop`; flag clears automatically when the loop exits.
/// `iteration_cap = 0` uses the default (50).
#[tauri::command]
pub fn set_task_goal_mode(
    state: State<'_, AppState>,
    task_id: String,
    enabled: bool,
    iteration_cap: Option<u32>,
) -> Result<(), String> {
    let mut agent = state.agent.lock().unwrap();
    let task = agent
        .tasks
        .get_mut(&task_id)
        .ok_or_else(|| format!("Task not found: {}", task_id))?;
    task.is_goal_mode = enabled;
    task.goal_iteration_cap = iteration_cap.unwrap_or(0);
    Ok(())
}

/// Forward user answers to the parked `ask_user` tool. `cancelled` when user dismissed.
#[tauri::command]
pub fn respond_to_ask_user(
    state: State<'_, AppState>,
    request_id: String,
    answers: serde_json::Value,
    cancelled: bool,
) -> Result<(), String> {
    let agent = state.agent.lock().map_err(|e| e.to_string())?;
    agent.ask_user_broker.respond(
        &request_id,
        rustic_agent::task::ask_user_broker::AskUserResponse { answers, cancelled },
    );
    Ok(())
}

/// Resolve a parked ceiling-breach. `action`: "raise" (persists new ceiling) or "stop".
#[tauri::command]
pub fn respond_to_ceiling_breach(
    state: State<'_, AppState>,
    request_id: String,
    action: String,
    new_ceiling_cents: Option<u64>,
) -> Result<(), String> {
    let mut agent = state.agent.lock().map_err(|e| e.to_string())?;
    let resolution = match action.as_str() {
        "raise" => {
            let new_cents = new_ceiling_cents
                .ok_or_else(|| "raise action requires new_ceiling_cents".to_string())?;
            if new_cents == 0 {
                return Err("new_ceiling_cents must be > 0".to_string());
            }
            // Persist so subsequent send_message calls (which rebuild the
            // Budget from this snapshot) see the new value too. The
            // executor's in-memory Budget is updated via the broker
            // resolution path on the receiving end.
            agent.ai_config.budget.daily_cost_ceiling_cents = Some(new_cents);
            rustic_agent::task::ceiling_broker::CeilingResolution::RaiseTo(new_cents)
        }
        "stop" => rustic_agent::task::ceiling_broker::CeilingResolution::Stop,
        other => return Err(format!("unknown ceiling-breach action: {}", other)),
    };
    agent.ceiling_broker.respond(&request_id, resolution);
    Ok(())
}

/// Forward a free-text reply for an `UnknownPrompt` harness envelope.
#[tauri::command]
pub fn respond_to_unknown_prompt(
    app: AppHandle,
    state: State<'_, AppState>,
    task_id: String,
    request_id: String,
    answer: String,
) -> Result<(), String> {
    let registry = state.harness_registry.clone();
    let task_id_for_async = task_id.clone();
    let app_for_async = app.clone();
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                tracing::error!(error = %e, "respond_to_unknown_prompt: tokio init failed");
                return;
            }
        };
        rt.block_on(async move {
            let Some(session) = registry.get(&task_id_for_async).await else {
                tracing::warn!(
                    task = %task_id_for_async,
                    "respond_to_unknown_prompt: no live harness session for task"
                );
                return;
            };
            if let Err(e) = session.respond_to_question(request_id.clone(), answer).await {
                tracing::error!(
                    task = %task_id_for_async,
                    error = %e,
                    "respond_to_unknown_prompt: forwarding failed (CLI may not support this envelope type)"
                );
                // Surface the failure so the UI can toast it. Tauri silently
                // drops emit errors so the worst case is the user sees no
                // toast and has to abort the task manually.
                let _ = app_for_async.emit(
                    "agent-unknown-prompt-error",
                    serde_json::json!({
                        "task_id": task_id_for_async,
                        "error": e.to_string(),
                    }),
                );
            }
        });
    });
    Ok(())
}

#[tauri::command]
pub fn switch_model(
    app: AppHandle,
    state: State<'_, AppState>,
    task_id: String,
    provider_type: String,
    model: String,
) -> Result<(), String> {
    let mut agent = state.agent.lock().unwrap();
    let task = agent
        .tasks
        .get_mut(&task_id)
        .ok_or_else(|| format!("Task not found: {}", task_id))?;

    let from_model = task.info.model.clone();
    let from_provider = task.info.provider_type.clone();

    // No-op if selecting the same model already in use
    if from_model == model && from_provider == provider_type {
        return Ok(());
    }

    // Cross-family switching is disallowed mid-chat. Claude Code and Codex own
    // their own session context (resumed via session id) — Rustic only mirrors
    // visible messages. Switching family would silently drop that context, so
    // we lock the chat to the family it started with. Within the same harness
    // family the CLI handles the model swap on resume, and API↔API is fine
    // because Rustic owns the full message history.
    //
    // Exception: a brand-new task with no user message yet has no session to
    // preserve. The welcome-screen flow always creates a task with the
    // `default_provider`, then immediately calls `switch_model` to apply the
    // user's pick. Blocking that here was the source of "ClaudeCode picked,
    // Anthropic charged" — the frontend silently swallowed the error and the
    // task ran on whatever provider was the install-time default.
    let has_user_text = task.messages.iter().any(|m| {
        matches!(m.role, Role::User)
            && m.content.iter().any(|c| matches!(c, ContentBlock::Text { .. }))
    });
    let from_harness = is_harness_provider_key(&from_provider);
    let to_harness = is_harness_provider_key(&provider_type);
    let cross_family = from_harness != to_harness
        || (from_harness && to_harness && from_provider != provider_type);
    if cross_family && has_user_text {
        return Err(format!(
            "Cannot switch from {} to {} mid-chat — start a new chat to use a different provider family.",
            from_provider, provider_type
        ));
    }

    task.info.model = model.clone();
    task.info.provider_type = provider_type.clone();

    // Push a UI-only marker into the message history (persisted to DB, never sent to API)
    task.messages.push(Message {
        role: Role::User,
        content: vec![ContentBlock::ModelSwitch {
            from_model: from_model.clone(),
            to_model: model.clone(),
        }],
    });
    drop(agent);

    // Persist the new model to DB
    let db = state.db.lock().unwrap();
    let _ = db.update_task_model(&task_id, &provider_type, &model);
    drop(db);

    let _ = app.emit(
        "agent-model-switched",
        AgentModelSwitchedEvent { task_id, from_model, to_model: model, provider_type },
    );
    Ok(())
}


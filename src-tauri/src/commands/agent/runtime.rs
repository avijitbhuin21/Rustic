//! Runtime control commands: cost, abort, permission/question responses,
//! sensitive-file approvals, model switching, subagent records.

use crate::state::AppState;
use crate::sync_ext::MutexExt;
use rustic_agent::{ContentBlock, Message, NativePermissionDecision, Role, TaskCost, TaskStatus};
use rustic_db::SubagentRecord;
use std::sync::atomic::Ordering;
use tauri::{AppHandle, Emitter, State};
use super::{AgentModelSwitchedEvent, AgentStatusEvent};

#[tauri::command]
pub fn get_task_cost(state: State<'_, AppState>, task_id: String) -> Result<TaskCost, String> {
    let map = state.task_costs.lock().map_err(|e| e.to_string())?;
    Ok(map.get(&task_id).cloned().unwrap_or_default())
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
    let agent = state.agent.lock_safe();
    let token = agent
        .cancellation_tokens
        .get(&task_id)
        .cloned()
        .ok_or_else(|| format!("No running task found: {}", task_id))?;
    token.store(true, Ordering::SeqCst);
    drop(agent);

    let _ = app.emit(
        "agent-task-status",
        AgentStatusEvent {
            task_id: task_id.clone(),
            status: TaskStatus::Cancelled,
        },
    );

    Ok(())
}

/// Handle permission card response. `decision` ("accept"/"acceptForSession"/"deny")
/// wins over the legacy `approved` bool.
#[tauri::command]
pub fn respond_to_permission(
    _app: AppHandle,
    state: State<'_, AppState>,
    _task_id: String,
    request_id: String,
    approved: Option<bool>,
    decision: Option<String>,
) -> Result<(), String> {
    let parsed_decision = match decision.as_deref() {
        Some("accept") => Some(NativePermissionDecision::Accept),
        Some("acceptForSession") => Some(NativePermissionDecision::AcceptForSession),
        Some("deny") => Some(NativePermissionDecision::Deny),
        Some(other) => return Err(format!("Unknown permission decision: {other}")),
        None => None,
    };

    let native_decision = match (parsed_decision, approved) {
        (Some(d), _) => d,
        (None, Some(true)) => NativePermissionDecision::Accept,
        (None, Some(false)) => NativePermissionDecision::Deny,
        (None, None) => return Err("Either `approved` or `decision` is required".into()),
    };

    let agent = state.agent.lock_safe();
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
    let mut agent = state.agent.lock_safe();
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
    let mut agent = state.agent.lock_safe();
    let task = agent
        .tasks
        .get_mut(&task_id)
        .ok_or_else(|| format!("Task not found: {}", task_id))?;
    task.is_plan_mode = enabled;
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

#[tauri::command]
pub fn switch_model(
    app: AppHandle,
    state: State<'_, AppState>,
    task_id: String,
    provider_type: String,
    model: String,
) -> Result<(), String> {
    let mut agent = state.agent.lock_safe();
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
    let db = state.db.lock_safe();
    let _ = db.update_task_model(&task_id, &provider_type, &model);
    drop(db);

    let _ = app.emit(
        "agent-model-switched",
        AgentModelSwitchedEvent { task_id, from_model, to_model: model, provider_type },
    );
    Ok(())
}


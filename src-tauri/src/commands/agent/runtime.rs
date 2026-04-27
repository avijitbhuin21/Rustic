//! Runtime control commands: cost, abort, permission/question responses,
//! sensitive-file approvals, model switching, subagent records.

use crate::state::AppState;
use rustic_agent::{ContentBlock, Message, Role, TaskCost, TaskStatus};
use rustic_db::SubagentRecord;
use std::sync::atomic::Ordering;
use tauri::{AppHandle, Emitter, State};
use super::{AgentModelSwitchedEvent, AgentStatusEvent};

#[tauri::command]
pub fn get_task_cost(state: State<'_, AppState>, task_id: String) -> Result<TaskCost, String> {
    let map = state.task_costs.lock().map_err(|e| e.to_string())?;
    Ok(map.get(&task_id).cloned().unwrap_or_default())
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
    let agent = state.agent.lock().unwrap();
    if let Some(token) = agent.cancellation_tokens.get(&task_id) {
        token.store(true, Ordering::SeqCst);
        // Emit status event immediately so the UI updates without waiting for the executor
        let _ = app.emit("agent-task-status", AgentStatusEvent {
            task_id: task_id.clone(),
            status: TaskStatus::Cancelled,
        });
        Ok(())
    } else {
        Err(format!("No running task found: {}", task_id))
    }
}

#[tauri::command]
pub fn respond_to_permission(
    state: State<'_, AppState>,
    task_id: String,
    request_id: String,
    approved: bool,
) -> Result<(), String> {
    // task_id is accepted for future per-task broker support; currently we have one global broker.
    let _ = task_id;
    let agent = state.agent.lock().unwrap();
    agent.permission_broker.respond(&request_id, approved);
    Ok(())
}

#[tauri::command]
pub fn respond_to_question(
    state: State<'_, AppState>,
    task_id: String,
    request_id: String,
    answer: String,
) -> Result<(), String> {
    let _ = task_id;
    let agent = state.agent.lock().unwrap();
    agent.question_broker.respond(&request_id, answer);
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

    // No-op if selecting the same model already in use
    if from_model == model && task.info.provider_type == provider_type {
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
    let db = state.db.lock().unwrap();
    let _ = db.update_task_model(&task_id, &provider_type, &model);
    drop(db);

    let _ = app.emit(
        "agent-model-switched",
        AgentModelSwitchedEvent { task_id, from_model, to_model: model, provider_type },
    );
    Ok(())
}


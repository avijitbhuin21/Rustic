//! Task-list loading shared by both hosts.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use rustic_agent::{
    calculate_cost_breakdown, ContentBlock, Message, Role, TaskCost, TaskInfo, TaskStatus,
    TokenUsage,
};

use crate::host_ops::agent_history::{
    archived_history_dtos, merge_archived_history, strip_synthetic_blocks, MessageDto, TurnUsage,
};
use crate::state::{AgentTask, AppState};
use crate::sync_ext::MutexExt;

/// Keep `Some(v)` only for a configured (positive) custom price.
fn custom_price(v: f64) -> Option<f64> {
    (v > 0.0).then_some(v)
}

/// List a project's chats (hydrating DB rows into the in-memory task table), or every in-memory task when `project_id` is `None`.
pub fn list_tasks(state: &AppState, project_id: Option<&str>) -> Result<Vec<TaskInfo>, String> {
    let Some(pid) = project_id else {
        let agent = state.agent.lock_safe();
        return Ok(agent.tasks.values().map(|t| t.info.clone()).collect());
    };
    let pid_root: Option<String> = state
        .workspace
        .lock_safe()
        .list_projects()
        .into_iter()
        .find(|p| p.id == pid)
        .map(|p| p.root_path.to_string_lossy().to_string());
    let rows = state.read_db(|db| -> Result<_, String> {
        let mut rows = db.list_tasks_for_project(pid).map_err(|e| e.to_string())?;
        // First-send persistence maps the task to the DB project that owns
        // this root path, which can have a different id than the workspace
        // project (issue #1). Include those rows so the chat isn't orphaned.
        if let Some(root) = pid_root {
            if let Ok(Some(alias)) = db.get_project_by_path(&root) {
                if alias.id != pid {
                    if let Ok(extra) = db.list_tasks_for_project(&alias.id) {
                        rows.extend(extra);
                    }
                }
            }
        }
        Ok(rows)
    })?;

    // Persisted "Running"/"Waiting" rows are leftovers from a crashed session.
    if rows
        .iter()
        .any(|r| r.status == "Running" || r.status == "Waiting")
    {
        let db = state.db.lock_safe();
        for row in &rows {
            if row.status == "Running" || row.status == "Waiting" {
                tracing::warn!(
                    target: "rustic::list_tasks",
                    task = %row.id,
                    db_status = %row.status,
                    "found stale Running/Waiting task on startup — defensively marking Completed",
                );
                let _ = db.update_task_status(&row.id, "Completed");
            }
        }
    }

    let mut agent = state.agent.lock_safe();
    for row in &rows {
        if agent.tasks.contains_key(&row.id) {
            continue;
        }
        let status = match row.status.as_str() {
            "Failed" => TaskStatus::Failed,
            "Cancelled" => TaskStatus::Cancelled,
            _ => TaskStatus::Completed,
        };
        let permissions = agent
            .project_permissions
            .get(&row.project_id)
            .cloned()
            .unwrap_or_default();
        // The DB persists token totals + the rolled-up cost; re-derive the
        // per-category breakdown so the cost popover isn't all $0.00 on reload.
        let usage = TokenUsage {
            input_tokens: row.total_input_tokens as u32,
            output_tokens: row.total_output_tokens as u32,
            cache_read_tokens: row.total_cache_read_tokens as u32,
            cache_write_tokens: 0,
        };
        let (custom_inp, custom_out, custom_cr, custom_cw) = agent
            .ai_config
            .providers
            .iter()
            .find(|p| p.provider_key() == row.provider_type)
            .map(|pe| {
                (
                    custom_price(pe.custom_input_cost),
                    custom_price(pe.custom_output_cost),
                    custom_price(pe.custom_cached_input_cost),
                    custom_price(pe.custom_cached_output_cost),
                )
            })
            .unwrap_or((None, None, None, None));
        let breakdown = calculate_cost_breakdown(
            &row.model, &usage, custom_inp, custom_out, custom_cr, custom_cw,
        );
        let cost = row
            .cost_json
            .as_deref()
            .and_then(|j| serde_json::from_str::<TaskCost>(j).ok())
            .unwrap_or_else(|| TaskCost {
                total_input_tokens: row.total_input_tokens as u64,
                total_output_tokens: row.total_output_tokens as u64,
                total_cache_read_tokens: row.total_cache_read_tokens as u64,
                total_cache_write_tokens: 0,
                estimated_cost_usd: row.estimated_cost_usd,
                turn_count: row.turn_count as u32,
                input_cost_usd: breakdown.input_usd,
                output_cost_usd: breakdown.output_usd,
                cache_read_cost_usd: breakdown.cache_read_usd,
                cache_write_cost_usd: breakdown.cache_write_usd,
                ..Default::default()
            });
        if let Ok(mut cost_map) = state.task_costs.lock() {
            cost_map
                .entry(row.id.clone())
                .or_insert_with(|| cost.clone());
        }
        agent.tasks.insert(
            row.id.clone(),
            AgentTask {
                info: TaskInfo {
                    id: row.id.clone(),
                    project_id: row.project_id.clone(),
                    title: row.title.clone(),
                    status,
                    provider_type: row.provider_type.clone(),
                    model: row.model.clone(),
                    created_at: row.created_at.clone(),
                    updated_at: row.updated_at.clone(),
                    thinking_tier: row.thinking_tier.clone(),
                    pinned: row.pinned,
                    goal: row.goal.clone(),
                },
                messages: Vec::new(),
                permissions,
                sensitive_files_allowed: false,
                is_plan_mode: false,
                shared_permissions: None,
                goal: Arc::new(Mutex::new(row.goal.clone().map(|condition| {
                    rustic_agent::task::goal::GoalState {
                        condition,
                        turns: 0,
                    }
                }))),
                last_input_tokens: Arc::new(std::sync::atomic::AtomicU32::new(0)),
            },
        );
    }

    // Keep the frontend-visible goal in sync with the DB — the executor and
    // event forwarder clear tasks.goal on "met" without touching TaskInfo.
    for row in &rows {
        if let Some(t) = agent.tasks.get_mut(&row.id) {
            t.info.goal = row.goal.clone();
        }
    }

    Ok(rows
        .iter()
        .filter_map(|row| agent.tasks.get(&row.id).map(|t| t.info.clone()))
        .collect())
}

const MESSAGE_DTO_CACHE_CAP: usize = 32;

/// Per-task cache of slimmed in-memory transcripts, keyed by an exact history fingerprint.
type DtoCache = Mutex<HashMap<String, (u64, Vec<MessageDto>)>>;

fn message_dto_cache() -> &'static DtoCache {
    static CACHE: OnceLock<DtoCache> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// Load a chat's transcript as DTOs (live in-memory history first, else the DB), with synthetic injections stripped and archived history merged in.
pub fn get_task_messages(state: &AppState, task_id: &str) -> Result<Vec<MessageDto>, String> {
    let times: HashMap<i64, String> = state
        .read_db(|db| db.get_message_times(task_id).unwrap_or_default())
        .into_iter()
        .collect();
    let mem_dtos: Option<Vec<MessageDto>> = {
        let agent = state.agent.lock_safe();
        agent
            .tasks
            .get(task_id)
            .filter(|task| !task.messages.is_empty())
            .map(|task| {
                let fingerprint = rustic_agent::history_dto::history_fingerprint(&task.messages);
                let mut cache = message_dto_cache().lock_safe();
                if let Some((cached_fp, cached)) = cache.get(task_id) {
                    if *cached_fp == fingerprint {
                        return cached.clone();
                    }
                }
                let built: Vec<MessageDto> = task
                    .messages
                    .iter()
                    .enumerate()
                    .filter_map(|(i, m)| {
                        let clean = strip_synthetic_blocks(m)?;
                        Some(MessageDto {
                            role: match m.role {
                                Role::User => "user".to_string(),
                                Role::Assistant => "assistant".to_string(),
                                Role::System => "system".to_string(),
                            },
                            content: rustic_agent::history_dto::slim_content(&clean),
                            turn_usage: None,
                            sort_order: Some(i as i64),
                            created_at: None,
                        })
                    })
                    .collect();
                if cache.len() >= MESSAGE_DTO_CACHE_CAP {
                    cache.clear();
                }
                cache.insert(task_id.to_string(), (fingerprint, built.clone()));
                built
            })
    };
    if let Some(mut dtos) = mem_dtos {
        for dto in dtos.iter_mut() {
            dto.created_at = dto.sort_order.and_then(|i| times.get(&i).cloned());
        }
        let archived = state.read_db(|db| archived_history_dtos(db, task_id));
        return Ok(merge_archived_history(archived, dtos));
    }

    let rows = state
        .read_db(|db| db.get_messages_for_task(task_id))
        .map_err(|e| e.to_string())?;

    let mut dtos = Vec::new();
    let mut messages_for_cache = Vec::new();
    for row in &rows {
        let role = match row.role.as_str() {
            "user" => Role::User,
            _ => Role::Assistant,
        };
        let content: Vec<ContentBlock> =
            serde_json::from_str(&row.content_json).unwrap_or_else(|e| {
                tracing::warn!(
                    "[get_task_messages] Failed to deserialize content_json for message {}: {}. Falling back to raw text.",
                    row.id,
                    e
                );
                vec![ContentBlock::Text {
                    text: row.content_json.clone(),
                }]
            });
        let turn_usage: Option<TurnUsage> = row
            .turn_usage_json
            .as_deref()
            .and_then(|j| serde_json::from_str(j).ok());
        let msg_for_cache = Message { role, content };
        if let Some(clean_content) = strip_synthetic_blocks(&msg_for_cache) {
            dtos.push(MessageDto {
                role: row.role.clone(),
                content: rustic_agent::history_dto::slim_content(&clean_content),
                turn_usage,
                sort_order: Some(row.sort_order),
                created_at: Some(row.created_at.clone()),
            });
        }
        messages_for_cache.push(msg_for_cache);
    }

    if !rows.is_empty() {
        let mut agent = state.agent.lock_safe();
        if let Some(task) = agent.tasks.get_mut(task_id) {
            task.messages = messages_for_cache;
        }
    }

    let archived = state.read_db(|db| archived_history_dtos(db, task_id));
    Ok(merge_archived_history(archived, dtos))
}

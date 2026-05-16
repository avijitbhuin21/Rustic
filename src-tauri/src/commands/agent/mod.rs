// Submodules. Re-exported so existing handler paths like
// `commands::agent::fetch_ai_models` keep resolving for tauri::generate_handler!.
mod harness_models;
mod harness_probe;
mod harness_runtime;
mod harness_slash;
mod memory;
mod mcp;
mod models;
mod project_defaults;
mod runtime;
pub use harness_models::*;
pub use harness_probe::*;
pub use harness_slash::*;
pub use memory::*;
pub use mcp::*;
pub use models::*;
pub use project_defaults::*;
pub use runtime::*;

use crate::state::{AgentTask, AppState};
use rustic_agent::{
    AiConfig, AiProvider, ContentBlock, Message,
    PermissionLevel, ProviderConfig, ProviderType, Role, SharedPermissions, TaskCost,
    TodoItem, TaskEvent, TaskExecutor, TaskInfo, TaskStatus, ToolConfig, ToolContext, ToolDef,
    build_skills_system_section, discover_skills,
    build_workflows_system_section, discover_workflows,
    build_user_rules_system_section,
};
use rustic_db::{MessageRow, TaskRow};
use std::sync::atomic::{AtomicBool, Ordering};
use rustic_agent::provider::claude::ClaudeProvider;
use rustic_agent::provider::compatible::CompatibleProvider;
use rustic_agent::provider::gemini::GeminiProvider;
use rustic_agent::provider::openai::OpenAiProvider;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};

/// Per-user-turn token/cost breakdown, stored in messages.turn_usage_json and
/// returned by get_task_messages so history views can show per-message stats.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct TurnUsage {
    pub input: i64,
    pub output: i64,
    pub cache_read: i64,
    pub cache_write: i64,
    pub cost: f64,
}

/// Returned by get_task_messages — message content plus optional per-turn stats.
#[derive(Serialize)]
pub struct MessageDto {
    pub role: String,
    pub content: Vec<ContentBlock>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_usage: Option<TurnUsage>,
}

#[derive(Clone, Serialize)]
pub(super) struct AgentStreamEvent {
    pub task_id: String,
    pub text: String,
}

#[derive(Clone, Serialize)]
pub(super) struct AgentToolUseEvent {
    pub task_id: String,
    pub tool_use_id: String,
    pub tool_name: String,
    pub tool_input: serde_json::Value,
}

#[derive(Clone, Serialize)]
pub(super) struct AgentToolUseStartEvent {
    pub task_id: String,
    pub tool_use_id: String,
    pub tool_name: String,
}

#[derive(Clone, Serialize)]
pub(super) struct AgentToolUseInputDeltaEvent {
    pub task_id: String,
    pub tool_use_id: String,
    pub partial_json: String,
}

#[derive(Clone, Serialize)]
pub(super) struct AgentToolUseStopEvent {
    pub task_id: String,
    pub tool_use_id: String,
}

#[derive(Clone, Serialize)]
pub(super) struct AgentToolResultEvent {
    pub task_id: String,
    pub tool_use_id: String,
    pub output: String,
    pub is_error: bool,
}

#[derive(Clone, Serialize)]
struct AgentToolProgressEvent {
    task_id: String,
    tool_use_id: String,
    progress_text: String,
}

#[derive(Clone, Serialize)]
pub(super) struct AgentStatusEvent {
    pub task_id: String,
    pub status: TaskStatus,
}

#[derive(Clone, Serialize)]
pub(super) struct AgentTaskCompleteEvent {
    pub task_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

#[derive(Clone, Serialize)]
pub(super) struct AgentPermissionRequestEvent {
    pub task_id: String,
    pub request_id: String,
    pub operation: String,
    pub description: String,
    pub preview: Option<String>,
}

#[derive(Clone, Serialize)]
pub(super) struct AgentCostUpdateEvent {
    pub task_id: String,
    pub cost: TaskCost,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AgentRequestUsageEvent {
    pub task_id: String,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_read_tokens: u32,
    pub cache_write_tokens: u32,
    pub cost_usd: f64,
}

#[derive(Clone, Serialize)]
struct AgentMemoryUpdatedEvent {
    task_id: String,
}

#[derive(Clone, Serialize)]
pub(super) struct AgentModelSwitchedEvent {
    pub task_id: String,
    pub from_model: String,
    pub to_model: String,
    pub provider_type: String,
}

#[derive(Clone, Serialize)]
pub(super) struct AgentSubagentSpawnedEvent {
    pub task_id: String,
    pub agent_id: String,
    pub model: String,
    pub prompt: String,
}

#[derive(Clone, Serialize)]
pub(super) struct AgentSubagentCompletedEvent {
    pub task_id: String,
    pub agent_id: String,
    /// C8.2-followup: carries the resolved model the sub-agent ran on so the
    /// completion card displays the right tier even if the `SubagentSpawned`
    /// event was missed.
    pub model: String,
    pub summary: String,
}

#[derive(Clone, Serialize)]
pub(super) struct AgentSubagentFailedEvent {
    pub task_id: String,
    pub agent_id: String,
    pub error: String,
}

#[derive(Clone, Serialize)]
struct AgentSubagentTextDeltaEvent {
    task_id: String,
    agent_id: String,
    text: String,
}

#[derive(Clone, Serialize)]
struct AgentSubagentCostUpdateEvent {
    task_id: String,
    agent_id: String,
    cost: TaskCost,
}

#[derive(Clone, Serialize)]
struct AgentSubagentToolUseEvent {
    task_id: String,
    agent_id: String,
    tool_name: String,
    tool_use_id: String,
    input: serde_json::Value,
}

#[derive(Clone, Serialize)]
struct AgentSubagentToolResultEvent {
    task_id: String,
    agent_id: String,
    tool_use_id: String,
    content: String,
    is_error: bool,
}

#[derive(Clone, Serialize)]
pub(super) struct AgentThinkingDeltaEvent {
    pub task_id: String,
    pub text: String,
}

#[derive(Clone, Serialize)]
struct AgentThinkingDoneEvent {
    task_id: String,
    duration_secs: u64,
}

#[derive(Clone, Serialize)]
pub(super) struct AgentQuestionRequestEvent {
    pub task_id: String,
    pub request_id: String,
    pub question: String,
    pub choices: Vec<String>,
}

#[derive(Clone, Serialize)]
pub struct AgentTodoUpdatedEvent {
    pub task_id: String,
    pub todos: Vec<TodoItem>,
}

#[derive(Clone, Serialize)]
pub(super) struct AgentTitleChangedEvent {
    pub task_id: String,
    pub title: String,
}

#[derive(Clone, Serialize)]
struct AgentContextCondenseStartedEvent {
    task_id: String,
}

#[derive(Clone, Serialize)]
struct AgentContextCondenseCompletedEvent {
    task_id: String,
    original_messages: u32,
    condensed_to: u32,
}

#[tauri::command]
pub fn create_task(
    state: State<'_, AppState>,
    project_id: String,
    _project_name: String,
    _project_root: String,
    title: String,
) -> Result<TaskInfo, String> {
    // Load project defaults from DB (if any)
    let project_defaults: ProjectDefaults = {
        let db = state.db.lock().unwrap();
        db.get_project(&project_id)
            .ok()
            .flatten()
            .and_then(|p| p.settings_json)
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default()
    };

    let mut agent = state.agent.lock().unwrap();

    let task_id = uuid::Uuid::new_v4().to_string();

    // Use project default model/provider if available, otherwise global defaults
    let (provider_key, model) = if let (Some(ref pt_str), Some(ref m)) =
        (&project_defaults.provider_type, &project_defaults.model)
    {
        (pt_str.clone(), m.clone())
    } else {
        let pt = agent
            .ai_config
            .default_provider
            .clone()
            .unwrap_or(ProviderType::Claude);
        // Pick the first matching entry for the default provider type
        let entry = agent
            .ai_config
            .providers
            .iter()
            .find(|p| p.provider_type == pt);
        let key = entry
            .map(|e| e.provider_key())
            .unwrap_or_else(|| format!("{:?}", pt));
        let m = entry
            .map(|p| p.default_model.clone())
            .unwrap_or_else(|| "claude-sonnet-4-20250514".to_string());
        (key, m)
    };

    let now = chrono::Utc::now().to_rfc3339();
    let info = TaskInfo {
        id: task_id.clone(),
        project_id: project_id.clone(),
        title,
        status: TaskStatus::Completed, // idle until a message is sent
        provider_type: provider_key,
        model,
        created_at: now.clone(),
        updated_at: now,
    };

    // Use project default permissions if available, otherwise in-memory project default
    let permissions = if let Some(ref perm_str) = project_defaults.permission_level {
        parse_permission_level(perm_str).unwrap_or_default()
    } else {
        agent
            .project_permissions
            .get(&project_id)
            .cloned()
            .unwrap_or_default()
    };

    agent.tasks.insert(
        task_id.clone(),
        AgentTask {
            info: info.clone(),
            messages: Vec::new(),
            permissions,
            sensitive_files_allowed: false,
            is_plan_mode: false,
            is_goal_mode: false,
            goal_iteration_cap: 0,
            shared_permissions: None,
            cost: Default::default(),
        },
    );

    // Task stays in-memory only until the first message is sent.
    // DB persistence is deferred to send_message() so that empty chats
    // never appear in history.

    Ok(info)
}

#[derive(Debug, Clone, Deserialize)]
pub struct ImageAttachment {
    pub media_type: String,
    pub data: String,
}

#[tauri::command]
pub fn send_message(
    app: AppHandle,
    state: State<'_, AppState>,
    task_id: String,
    message: String,
    thinking_budget: Option<u32>,
    images: Option<Vec<ImageAttachment>>,
) -> Result<(), String> {
    // ── Harness-provider early dispatch ───────────────────────────────────
    // Tasks bound to a harness provider (Claude Code, Codex) are driven by an
    // external CLI process, not the in-Rust tool loop. The harness owns the
    // system prompt, tool set, and permission model, so we skip *all* of
    // the native pipeline — system prompt assembly, MCP loading, ProviderConfig
    // building, TaskExecutor — and hand off to a dedicated runtime that just
    // pumps stream-json events to the existing `agent-*` Tauri events.
    //
    // We peek at provider_type under a short lock so we don't pay the cost of
    // building everything below for harness tasks. `thinking_budget` is
    // ignored — Claude Code's CLI controls that itself per the user's config.
    let _ = thinking_budget;
    let harness_provider_key: Option<String> = {
        let agent = state.agent.lock().unwrap();
        agent
            .tasks
            .get(&task_id)
            .map(|t| t.info.provider_type.clone())
            .filter(|key| rustic_agent::is_harness_provider_key(key))
    };
    if harness_provider_key.is_some() {
        return harness_runtime::dispatch_harness_send(app, state, task_id, message, images);
    }

    let (mut messages, project_root, _permissions, _sensitive_files_allowed, task_is_plan_mode, task_is_goal_mode, task_goal_iteration_cap, shared_perms, provider_config, provider_type_str, cancel_token, permission_broker, ask_user_broker, ceiling_broker, mcp_manager_arc, ai_config, tool_config, allowed_paths, task_project_id, subagent_override, fh_handle_opt, snapshot_message_id) = {
        let mut agent = state.agent.lock().unwrap();

        // Read config values first (immutable access)
        let max_tokens = agent.ai_config.max_tokens;
        let temperature = agent.ai_config.temperature;

        let task = agent
            .tasks
            .get_mut(&task_id)
            .ok_or_else(|| format!("Task not found: {}", task_id))?;

        // Detect first real user message: the task may already contain non-user
        // entries (e.g. ModelSwitch markers from switch_model) but the task has
        // not been persisted to DB yet.  We need to persist on the first actual
        // user text message, not just when the messages vec is empty.
        let has_user_text = task.messages.iter().any(|m| {
            matches!(m.role, Role::User)
                && m.content.iter().any(|c| matches!(c, ContentBlock::Text { .. }))
        });
        let is_first_message = !has_user_text;
        if is_first_message && (task.info.title.is_empty() || task.info.title == "New Task") {
            // Strip `<pasted-text id="N">…</pasted-text>` wrappers from the
            // title source so a paste-only first message uses the inner body
            // instead of literally naming the task `<pasted-text id="…`.
            let unwrapped: String = {
                let mut out = String::with_capacity(message.len().min(512));
                let mut rest = message.as_str();
                while let Some(start) = rest.find("<pasted-text id=\"") {
                    out.push_str(&rest[..start]);
                    let after = &rest[start..];
                    if let Some(close) = after.find("\">") {
                        let body_start = close + 2;
                        let body = &after[body_start..];
                        if let Some(end) = body.find("</pasted-text>") {
                            // Trim the optional leading newline after `">` and
                            // trailing newline before `</pasted-text>` so the
                            // unwrapped body reads cleanly.
                            let inner = body[..end].trim_matches('\n');
                            out.push_str(inner);
                            rest = &body[end + "</pasted-text>".len()..];
                            continue;
                        }
                    }
                    // Malformed marker — keep the remainder verbatim.
                    out.push_str(after);
                    rest = "";
                    break;
                }
                out.push_str(rest);
                out
            };
            let raw: String = unwrapped.chars().take(70).collect();
            let safe: String = raw
                .chars()
                .map(|c| if "/\\:*?\"<>|\n\r\t".contains(c) { ' ' } else { c })
                .collect();
            let title = safe.trim().to_string();
            if !title.is_empty() {
                task.info.title = title.clone();
                {
                    let db = state.db.lock().unwrap();
                    let _ = db.update_task_title(&task_id, &title);
                }
                let _ = app.emit("agent-title-changed", AgentTitleChangedEvent {
                    task_id: task_id.clone(),
                    title,
                });
            }
        }

        let task_project_id = task.info.project_id.clone();
        let task_provider_type = task.info.provider_type.clone();
        let task_model = task.info.model.clone();
        let task_permissions = task.permissions.clone();
        let task_sensitive_files_allowed = task.sensitive_files_allowed;
        // P0.3: capture plan-mode at send-message time; the executor sees a
        // snapshot of the flag for the duration of this turn. Mid-turn
        // toggling is intentionally not supported — would need a shared
        // Atomic and we'd rather the user explicitly cancels + retoggles
        // than have an active tool race the flag.
        let task_is_plan_mode = task.is_plan_mode;
        // P1.8: snapshot goal-mode + iteration cap at send-message time. The
        // executor branches between `run_turn` (default) and `run_goal_loop`
        // on this flag for the duration of this turn.
        let task_is_goal_mode = task.is_goal_mode;
        let task_goal_iteration_cap = task.goal_iteration_cap;

        // Create or reuse shared permissions — the executor reads from this Arc in real-time.
        let shared_perms = task.shared_permissions.get_or_insert_with(|| {
            SharedPermissions::new(task_permissions.clone(), task_sensitive_files_allowed)
        }).clone();
        // Sync current values into shared permissions (user may have changed mode between messages)
        shared_perms.set_level(task_permissions.clone());
        shared_perms.set_sensitive_files_allowed(task_sensitive_files_allowed);

        // P0.6 fix #5: compute the gitignore flag here so the project-
        // structure injection below (and the system-prompt rebuild later)
        // can both see it from a single binding.
        let include_gitignored = matches!(task_permissions, PermissionLevel::FullAuto);

        // Get project info (needed before memory loading and DB persistence)
        let (project_root, project_name) = {
            let workspace = state.workspace.lock().unwrap();
            let proj = workspace
                .list_projects()
                .into_iter()
                .find(|p| p.id.to_string() == task_project_id)
                .ok_or_else(|| "Project not found".to_string())?;
            (proj.root_path.clone(), proj.name.clone())
        };

        // Persist task to DB on first message (deferred from create_task)
        if is_first_message {
            let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
            let db = state.db.lock().unwrap();
            // Use ensure_project to handle root_path/ID mismatches (e.g. after
            // app restart where the in-memory UUID diverged from the DB row).
            let actual_project_id = db.ensure_project(&rustic_db::models::ProjectRow {
                id: task_project_id.clone(),
                name: project_name,
                root_path: project_root.to_string_lossy().to_string(),
                created_at: now.clone(),
                settings_json: None,
            }).map_err(|e| format!("Failed to persist project: {}", e))?;
            db.insert_task(&TaskRow {
                id: task_id.clone(),
                project_id: actual_project_id,
                title: task.info.title.clone(),
                status: format!("{:?}", task.info.status),
                provider_type: task_provider_type.clone(),
                model: task_model.clone(),
                created_at: now.clone(),
                updated_at: now,
                total_input_tokens: 0,
                total_output_tokens: 0,
                total_cache_read_tokens: 0,
                estimated_cost_usd: 0.0,
                turn_count: 0,
                harness_session_id: None,
            }).map_err(|e| format!("Failed to persist task: {}", e))?;
        }

        // P0.6 fix #5: inject the project-structure block as a synthetic
        // first message pair on the first turn. Mirrors the memory.md
        // pattern below — keeps the tree out of the system prompt so
        // the prefix is cross-task cacheable, while still giving the
        // model the file layout from turn 1. Subsequent turns reuse
        // the cached tree out of the conversation history.
        if is_first_message {
            let structure_block = rustic_agent::build_first_message_context_block(
                &project_root,
                include_gitignored,
            );
            if !structure_block.trim().is_empty() {
                task.messages.push(Message {
                    role: Role::User,
                    content: vec![ContentBlock::Text { text: structure_block }],
                });
                task.messages.push(Message {
                    role: Role::Assistant,
                    content: vec![ContentBlock::Text {
                        text: "Project structure received. I'll reference it when planning file operations.".into(),
                    }],
                });
            }
        }

        // Inject memory.md as first message pair on the first turn
        if is_first_message {
            let memory_path = project_root.join(".rustic/memory.md");
            if let Ok(memory_content) = std::fs::read_to_string(&memory_path) {
                let trimmed = memory_content.trim().to_string();
                if !trimmed.is_empty() {
                    task.messages.push(Message {
                        role: Role::User,
                        content: vec![ContentBlock::Text {
                            text: format!("[Project Memory]\n{}", trimmed),
                        }],
                    });
                    task.messages.push(Message {
                        role: Role::Assistant,
                        content: vec![ContentBlock::Text {
                            text: "Memory loaded. I'll reference this context as needed.".into(),
                        }],
                    });
                }
            }
        }

        // Resume-after-interrupt repair: if the last message is an assistant
        // turn that contains tool_use blocks WITHOUT corresponding tool_result
        // blocks in the next message (which is the case when a process kill
        // tore down the executor while a tool was mid-run, or when a
        // chat_message question was awaiting a broker response that died with
        // the worker thread), the API will reject the next request with
        // `tool_use ids were found without tool_result blocks immediately
        // after`. Synthesize tool_result blocks for the dangling ids so the
        // conversation re-validates. The synthetic content explains that the
        // run was interrupted, which the model handles gracefully (it sees
        // its own tool_use, sees the "interrupted" result, and proceeds with
        // the user's new instruction).
        // IMPORTANT: Anthropic server-side tools (web_search / web_fetch)
        // arrive as a `server_tool_use` block paired with a
        // `web_search_tool_result` block in the SAME assistant message. Both
        // map to ContentBlock::ToolUse + ContentBlock::ToolResult internally.
        // Those ids are not dangling — the result is already inline.
        // Synthesizing a user tool_result for them would emit a generic
        // `tool_result` block with a `srvtoolu_*` id on the wire, which
        // Anthropic rejects with "unexpected tool_use_id found in tool_result
        // blocks". Skip any ToolUse whose id already has a matching ToolResult
        // in the same message — that covers server tools generically.
        let dangling_ids: Vec<String> = match task.messages.last() {
            Some(m) if matches!(m.role, Role::Assistant) => {
                let resolved_in_place: std::collections::HashSet<&str> = m
                    .content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::ToolResult { tool_use_id, .. } => {
                            Some(tool_use_id.as_str())
                        }
                        _ => None,
                    })
                    .collect();
                m.content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::ToolUse { id, .. }
                            if !resolved_in_place.contains(id.as_str()) =>
                        {
                            Some(id.clone())
                        }
                        _ => None,
                    })
                    .collect()
            }
            _ => Vec::new(),
        };
        if !dangling_ids.is_empty() {
            tracing::warn!(
                target: "rustic::resume_repair",
                task = %task_id,
                count = dangling_ids.len(),
                "synthesizing tool_result blocks for dangling tool_use ids before resume"
            );
            let blocks: Vec<ContentBlock> = dangling_ids
                .into_iter()
                .map(|tid| ContentBlock::ToolResult {
                    tool_use_id: tid,
                    content: "[Run was interrupted before this tool finished. Continuing from the user's next message.]".to_string(),
                    is_error: false,
                })
                .collect();
            task.messages.push(Message {
                role: Role::User,
                content: blocks,
            });
        }

        // Add user message (text + optional images).
        // Clone the body — P1.8 needs the original `message` string after
        // this point to weave into the goal-mode system-prompt addendum.
        let mut user_content = vec![ContentBlock::Text { text: message.clone() }];
        if let Some(ref imgs) = images {
            for img in imgs {
                user_content.push(ContentBlock::Image {
                    media_type: img.media_type.clone(),
                    data: img.data.clone(),
                });
            }
        }
        task.messages.push(Message {
            role: Role::User,
            content: user_content,
        });
        task.info.status = TaskStatus::Running;

        let task_messages = task.messages.clone();

        // ── Open a snapshot for this user message ─────────────────────────────
        // The tracker uses a UUID independent of the message persistence's
        // `{task_id}-{i}` ids — those shift when context condense rewrites
        // history, but our snapshot anchor needs to stay valid until eviction.
        // Generated here so every edit/bash this turn lands in the same
        // snapshot, and `/rewind` to this UUID restores the pre-turn state.
        // open_snapshot (which walks the entire worktree via shadow.track()) is
        // deferred to the spawned executor thread so it runs on spawn_blocking
        // and does not stall the synchronous Tauri command handler while the
        // worktree walk completes (can take several seconds on large projects).
        let snapshot_message_id = uuid::Uuid::new_v4().to_string();
        let fh_handle_opt = match std::path::PathBuf::from(&project_root).canonicalize() {
            Ok(canon_root) => match crate::commands::file_history::get_or_create_handle(
                state.inner(),
                &app,
                &canon_root,
            ) {
                Ok(handle) => Some(handle),
                Err(e) => {
                    tracing::warn!(task = %task_id, %e, "file_history handle init failed; tracker disabled for this turn");
                    None
                }
            },
            Err(e) => {
                tracing::warn!(task = %task_id, ?e, "project_root canonicalize failed; tracker disabled for this turn");
                None
            }
        };

        // Cancel any previous run that's still alive before installing the
        // new token. Without this signal, an older run_turn that was blocked
        // inside a `chat_message` (or any cancellation-aware tool) keeps
        // running in parallel with the fresh run we're about to spawn —
        // both write to the same task_id via persist_now, both emit events
        // on the same channel, and the older one's stale `messages` clone
        // can clobber the new one when its persist eventually lands.
        if let Some(prev_token) = agent.cancellation_tokens.get(&task_id) {
            prev_token.store(true, Ordering::SeqCst);
            tracing::warn!(
                target: "rustic::send_message",
                task = %task_id,
                "signalled cancel on previous run before starting new turn"
            );
        }
        // Create or refresh cancellation token for this run
        let cancel_token = Arc::new(AtomicBool::new(false));
        agent.cancellation_tokens.insert(task_id.clone(), Arc::clone(&cancel_token));

        // Build provider config
        let provider_entry = agent
            .ai_config
            .find_by_key(&task_provider_type)
            .cloned();

        if provider_entry.is_none() {
            return Err(format!(
                "The provider for this chat (\"{}\") is no longer configured. \
                 Please pick a different model from the model picker to continue.",
                task_provider_type
            ));
        }

        // In FullAuto mode the agent sees the full project tree (including
        // gitignored files). In every other mode gitignored files stay hidden
        // from the agent — the user still sees them in the file explorer.
        // (Bound earlier — used for the P0.6 first-message structure block too.)
        let is_global_scope = rustic_agent::is_global_project_id(&task_project_id);

        // Resolve the user-configured sub-agent override (cheaper/faster
        // model used when the orchestrator picks `model_tier: "fast"`). The
        // entry only counts if the referenced provider is still configured.
        let subagent_override: Option<(rustic_agent::SubagentConfig, rustic_agent::ProviderEntry)> =
            agent.ai_config.subagent.clone().and_then(|sub| {
                agent
                    .ai_config
                    .find_by_key(&sub.provider_key)
                    .cloned()
                    .map(|entry| (sub, entry))
            });
        let fast_subagent_model: Option<String> = subagent_override
            .as_ref()
            .map(|(sub, _)| sub.model.clone());

        let system_prompt = if is_global_scope {
            // Global orchestrator uses a dedicated prompt — no project tree,
            // no per-project rules, only the orchestrator tool reference.
            rustic_agent::build_orchestrator_prompt(&agent.ai_config.providers)
        } else {
            rustic_agent::build_system_prompt(
                &agent.ai_config.providers,
                &project_root,
                include_gitignored,
                &agent.tool_config,
                fast_subagent_model.as_deref(),
                agent.ai_config.budget.max_concurrent_subagents,
            )
        };

        // Skills and workflows are registered globally (user config dir), so
        // the Global orchestrator still gets them. Project-local skills /
        // workflows / rules are skipped in Global scope — project_root is
        // the Global internal dir and wouldn't contain any.
        let skills = discover_skills(&project_root);
        let skills_section = build_skills_system_section(&skills);
        let system_prompt = if skills_section.is_empty() {
            system_prompt
        } else {
            format!("{}{}", system_prompt, skills_section)
        };

        let workflows = discover_workflows(&project_root);
        let workflows_section = build_workflows_system_section(&workflows);
        let system_prompt = if workflows_section.is_empty() {
            system_prompt
        } else {
            format!("{}{}", system_prompt, workflows_section)
        };

        let system_prompt = if is_global_scope {
            // Skip per-project rules — in Global there is no project scope.
            system_prompt
        } else {
            let rules_section = build_user_rules_system_section(&project_root);
            if rules_section.is_empty() {
                system_prompt
            } else {
                format!("{}{}", system_prompt, rules_section)
            }
        };

        // P0.3: append the plan-mode addendum when the task is in plan mode.
        // The tool-partition step in task::executor already blocks every
        // write tool, but without this addendum the model only discovers
        // the restriction by hitting PERMISSION_DENIED — wastes a turn.
        // Stating it explicitly lets the model plan within the constraint
        // from the start.
        let system_prompt = if task_is_plan_mode {
            format!("{}{}", system_prompt, rustic_agent::plan_mode_addendum())
        } else {
            system_prompt
        };

        // P1.8: append the goal-mode addendum so the model knows the
        // outer loop semantics. The `message` parameter to this Tauri
        // call IS the goal text — the user typed `/goal <objective>` and
        // the frontend stripped the `/goal ` prefix before sending it.
        let system_prompt = if task_is_goal_mode {
            format!(
                "{}{}",
                system_prompt,
                rustic_agent::goal_mode_addendum(&message, task_goal_iteration_cap),
            )
        } else {
            system_prompt
        };

        // Resolve thinking budget with this precedence:
        //   1. Frontend-provided value for this message (explicit user choice)
        //   2. Per-provider user setting (custom_thinking_budget)
        //   3. Provider default (10k for Claude, 0 elsewhere)
        let thinking_budget_val = thinking_budget.unwrap_or_else(|| {
            let user_override = provider_entry
                .as_ref()
                .map(|p| p.custom_thinking_budget)
                .unwrap_or(0);
            if user_override > 0 {
                user_override
            } else if task_provider_type == "Claude" {
                10_000
            } else {
                0
            }
        });

        let custom_ctx = provider_entry
            .as_ref()
            .map(|p| p.custom_context_window)
            .unwrap_or(0);
        let context_window = rustic_agent::task::condense::get_context_window(&task_model, custom_ctx);

        // Auto-resolve max_tokens from the model registry.
        // For known models this returns the model's real max output limit.
        // For unknown models (Compatible provider) it falls back to the
        // user-configured value, or the ProviderEntry custom limit if set.
        let resolved_max_tokens = {
            let registry_val = rustic_agent::model_registry::max_output_tokens(&task_model, 0);
            if registry_val > 0 {
                registry_val
            } else if let Some(ref pe) = provider_entry {
                if pe.custom_max_output_tokens > 0 { pe.custom_max_output_tokens } else { max_tokens }
            } else {
                max_tokens
            }
        };

        let tool_config_snapshot = agent.tool_config.clone();
        // MCP-backed web_search overrides the built-in path — when the user
        // picked "Tavily MCP" as the backend, don't declare the server-side
        // tool either, so the MCP server's web_search tool takes over without
        // a name collision.
        let mcp_backs_web_search = matches!(
            tool_config_snapshot.web_search.backend,
            rustic_agent::WebSearchBackend::Mcp
        );
        let web_search_for_provider =
            tool_config_snapshot.web_search.enabled && !mcp_backs_web_search;
        let web_fetch_for_provider = tool_config_snapshot.web_fetch.enabled;

        let model_caps = agent.ai_config.capabilities_for(&task_model);
        let config = ProviderConfig {
            api_key: provider_entry
                .as_ref()
                .map(|p| p.api_key.clone())
                .unwrap_or_default(),
            model: task_model,
            max_tokens: resolved_max_tokens,
            temperature,
            base_url: provider_entry.as_ref().and_then(|p| p.base_url.clone()),
            system_prompt: Some(system_prompt),
            thinking_budget: thinking_budget_val,
            context_window,
            web_search_enabled: web_search_for_provider,
            web_fetch_enabled: web_fetch_for_provider,
            supports_temperature: model_caps.supports_temperature,
            supports_reasoning_effort: model_caps.supports_reasoning_effort,
            cancel_token: Some(Arc::clone(&cancel_token)),
        };

        // Load pre-approved paths from .rustic/allowed-files.txt
        let allowed_paths: Vec<String> = {
            let p = project_root.join(".rustic/allowed-files.txt");
            std::fs::read_to_string(&p)
                .unwrap_or_default()
                .lines()
                .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
                .map(|l| l.trim().to_string())
                .collect()
        };

        let broker = Arc::clone(&agent.permission_broker);
        // P0.2: clone the shared ask_user broker into a per-call handle so
        // ToolContext (and any sub-agent children) point at the same
        // instance the Tauri respond_to_ask_user command resolves against.
        let ask_user_broker = Arc::clone(&agent.ask_user_broker);
        // P0.4 fix #4: same pattern for the ceiling-breach broker.
        let ceiling_broker = Arc::clone(&agent.ceiling_broker);
        let mcp_arc = Arc::clone(&agent.mcp_manager);
        let ai_config = Arc::new(agent.ai_config.clone());
        let tool_config_arc = Arc::new(tool_config_snapshot);

        // Auto-load MCP configs from both scopes:
        //   user:    <app_data_dir>/mcp.json
        //   project: <project_root>/.mcp.json
        {
            let user_mcp_path = tauri::Manager::path(&app)
                .app_data_dir()
                .ok()
                .map(|d| d.join("mcp.json"));
            let project_mcp_path = project_root.join(".mcp.json");

            let mut mcp = mcp_arc.lock().unwrap();
            if let Some(p) = user_mcp_path {
                mcp.set_user_path(p.clone());
                let _ = mcp.load_scope(rustic_agent::McpScope::User, &p);
            }
            mcp.set_project_path(project_mcp_path.clone());
            let _ = mcp.load_scope(rustic_agent::McpScope::Project, &project_mcp_path);
        }

        (
            task_messages,
            project_root,
            task_permissions,
            task_sensitive_files_allowed,
            task_is_plan_mode,
            task_is_goal_mode,
            task_goal_iteration_cap,
            shared_perms,
            config,
            task_provider_type,
            cancel_token,
            broker,
            ask_user_broker,
            ceiling_broker,
            mcp_arc,
            ai_config,
            tool_config_arc,
            allowed_paths,
            task_project_id,
            subagent_override,
            fh_handle_opt,
            snapshot_message_id,
        )
    };

    let provider: Arc<dyn AiProvider> = if provider_type_str == "Claude" {
        Arc::new(ClaudeProvider::new())
    } else if provider_type_str == "OpenAi" {
        Arc::new(OpenAiProvider::new())
    } else if provider_type_str == "Gemini" {
        Arc::new(GeminiProvider::new())
    } else if provider_type_str == "Compatible"
        || provider_type_str.starts_with("Compatible:")
        || provider_type_str == "OpenRouter"
    {
        Arc::new(CompatibleProvider::new(provider_type_str.clone()))
    } else {
        Arc::new(ClaudeProvider::new())
    };

    let task_id_clone = task_id.clone();
    let app_clone = app.clone();

    // Clone shared maps for background thread
    let db_arc = Arc::clone(&state.db);
    let task_costs_arc = Arc::clone(&state.task_costs);
    let file_lock = Arc::clone(&state.file_lock);
    let subagent_registry = Arc::clone(&state.subagent_registry);
    let agent_arc = Arc::clone(&state.agent);
    // P1.7: recompute the values the background thread needs for the
    // deferred-tools directory. The `let (...) = { ... }` destructuring
    // earlier exports `task_project_id` and `subagent_override`; the
    // pre-thread block's internal bindings (`is_global_scope`,
    // `fast_subagent_model`) are out of scope here. Reconstitute them
    // from what IS in scope.
    let is_global_for_thread = rustic_agent::is_global_project_id(&task_project_id);
    let fast_subagent_model_for_thread: Option<String> = subagent_override
        .as_ref()
        .map(|(sub, _)| sub.model.clone());
    // P1.3: hand the WorkspaceServices registry to the background thread.
    // `state` itself can't cross the spawn boundary (its lifetime is the
    // command body), so capture the Arc here and look up the per-project
    // services from inside the thread once we know the canonical root.
    let workspace_services_registry = Arc::clone(&state.workspace_services);

    // Spawn async task for the agentic loop
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            // Emit Running status from inside the background thread so it is
            // guaranteed to arrive at the WebView before the Completed/Failed
            // events that this same thread emits later. Emitting from the main
            // command thread (before spawn) caused a race: fast providers
            // (GPT-5.4) could finish and emit Completed before the main
            // thread's Running event was delivered, leaving the UI stuck.
            let _ = app_clone.emit(
                "agent-task-status",
                AgentStatusEvent {
                    task_id: task_id_clone.clone(),
                    status: TaskStatus::Running,
                },
            );

            // Open the file-history snapshot on a dedicated blocking thread so
            // shadow.track()'s full worktree walk doesn't stall the Tauri
            // command handler. This must complete before the executor loop
            // starts so the pre-turn tree is captured before any tool edits.
            let fh_handle_opt = match fh_handle_opt {
                None => None,
                Some(handle) => {
                    let history_arc = Arc::clone(&handle.history);
                    let mid = snapshot_message_id.clone();
                    let tid = task_id_clone.clone();
                    match tokio::task::spawn_blocking(move || history_arc.open_snapshot(&mid, &tid))
                        .await
                    {
                        Ok(Ok(())) => {
                            if let Ok(db) = db_arc.lock() {
                                let current = db
                                    .get_task_todos(&task_id_clone)
                                    .ok()
                                    .flatten()
                                    .unwrap_or_else(|| "[]".to_string());
                                if let Err(e) = db.snapshot_todos_at_message(
                                    &task_id_clone,
                                    &snapshot_message_id,
                                    &current,
                                ) {
                                    tracing::warn!(task = %task_id_clone, ?e, "todo snapshot failed; revert won't restore the list for this turn");
                                }
                            }
                            Some(handle)
                        }
                        Ok(Err(e)) => {
                            tracing::warn!(task = %task_id_clone, ?e, "open_snapshot failed; tracker disabled for this turn");
                            None
                        }
                        Err(join_err) => {
                            tracing::warn!(task = %task_id_clone, ?join_err, "open_snapshot thread panicked; tracker disabled for this turn");
                            None
                        }
                    }
                }
            };

            // Connect MCP servers and gather tool defs in a blocking thread
            let mcp_arc_connect = Arc::clone(&mcp_manager_arc);
            let (mcp_tool_defs, mcp_system_section) =
                tokio::task::spawn_blocking(move || {
                    let mut mcp = mcp_arc_connect.lock().unwrap();
                    let _ = mcp.connect_all();
                    let tools = mcp.all_tools();
                    let section = build_mcp_system_section(&tools);
                    (tools, section)
                })
                .await
                .unwrap_or_default();

            // Append MCP section to system prompt if there are any tools
            let mut provider_config = provider_config;
            if !mcp_system_section.is_empty() {
                if let Some(ref mut sys) = provider_config.system_prompt {
                    sys.push_str(&mcp_system_section);
                }
            }

            // P1.7: append the deferred-tools directory. The model sees the
            // always-on tools by full schema and these by name + short blurb;
            // to call any of them it must first `tool_search` to fetch the
            // schema (which also marks the tool as loaded so subsequent turns
            // include the full schema in the request).
            //
            // We compute the FULL pool here once at task setup. MCP tools
            // that are added later in the session won't appear in this
            // catalog — acceptable for V1 since MCP connections are
            // typically stable across a task.
            {
                let tools = rustic_agent::BuiltinTools::new();
                let mut all_defs = tools
                    .definitions_for_host(&[], fast_subagent_model_for_thread.as_deref());
                // web_search/web_fetch and media tools (image/video/animate)
                // are dynamically constructed from tool_config in the
                // executor — they live in the always-on pool (web_search) or
                // are not in the deferred catalog (media). Skipping them
                // here is intentional; the catalog only lists deferred
                // tools the model needs to discover via `tool_search`.
                all_defs.extend(mcp_tool_defs.clone());
                // Respect the same orchestrator denylist the executor uses
                // when this is NOT a Global task — those tools never reach
                // the model in this scope, so they shouldn't be advertised.
                if !is_global_for_thread {
                    const ORCHESTRATOR_DENYLIST: &[&str] = &[
                        "list_projects",
                        "spawn_subtask",
                        "list_tasks_across_projects",
                        "read_task_history",
                    ];
                    all_defs.retain(|td| !ORCHESTRATOR_DENYLIST.contains(&td.name.as_str()));
                }
                let directory =
                    rustic_agent::build_deferred_tools_directory(&all_defs);
                if !directory.is_empty() {
                    if let Some(ref mut sys) = provider_config.system_prompt {
                        sys.push_str(&directory);
                    }
                }
            }

            let parent_provider_config = Arc::new(provider_config.clone());

            // Build the optional sub-agent provider config used when the
            // main agent picks `model_tier: "fast"`. Most fields just mirror
            // the parent (api_key/base_url/temperature/etc. come from the
            // referenced provider entry; max_tokens/context_window from the
            // model registry). Sub-agents always run with thinking disabled
            // — same as the regular sub-agent path in `subagent_tools.rs`.
            let subagent_provider_config: Option<Arc<rustic_agent::ProviderConfig>> =
                subagent_override.as_ref().map(|(sub, entry)| {
                    let caps = ai_config.capabilities_for(&sub.model);
                    let max_tokens_for_sub = {
                        let registry_val = rustic_agent::model_registry::max_output_tokens(&sub.model, 0);
                        if registry_val > 0 {
                            registry_val
                        } else if entry.custom_max_output_tokens > 0 {
                            entry.custom_max_output_tokens
                        } else {
                            ai_config.max_tokens
                        }
                    };
                    let ctx_for_sub = rustic_agent::task::condense::get_context_window(
                        &sub.model,
                        entry.custom_context_window,
                    );
                    Arc::new(rustic_agent::ProviderConfig {
                        api_key: entry.api_key.clone(),
                        model: sub.model.clone(),
                        max_tokens: max_tokens_for_sub,
                        temperature: ai_config.temperature,
                        base_url: entry.base_url.clone(),
                        // Sub-agent system prompt is supplied by
                        // `subagent_tools.rs` — we don't seed one here.
                        system_prompt: None,
                        thinking_budget: 0,
                        context_window: ctx_for_sub,
                        web_search_enabled: false,
                        web_fetch_enabled: false,
                        supports_temperature: caps.supports_temperature,
                        supports_reasoning_effort: caps.supports_reasoning_effort,
                        cancel_token: None,
                    })
                });

            let executor = TaskExecutor::new(provider, provider_config);

            // Create event channel before ToolContext so event_tx can be stored in context
            let (event_tx, mut event_rx) =
                tokio::sync::mpsc::channel::<TaskEvent>(rustic_agent::EVENT_CHANNEL_CAP);

            let is_global = rustic_agent::is_global_project_id(&task_project_id);
            let orchestrator_host: Option<Arc<dyn rustic_agent::OrchestratorHost>> = if is_global {
                Some(Arc::new(crate::commands::orchestrator_host::TauriOrchestratorHost::new(
                    app_clone.clone(),
                    Arc::clone(&agent_arc),
                    Arc::clone(&db_arc),
                )) as Arc<dyn rustic_agent::OrchestratorHost>)
            } else {
                None
            };

            // Build the incremental-persistence callback the executor calls
            // at every stable point during run_turn (top of each iteration,
            // after tool batches, before return). Wraps the same transactional
            // `replace_messages_for_task` used by the end-of-turn save so an
            // interruption after the callback fires leaves the DB in a
            // consistent state — the in-flight messages are already saved.
            let persist_db_arc = Arc::clone(&db_arc);
            let persist_task_id = task_id_clone.clone();
            let persist_messages_fn: rustic_agent::PersistMessagesFn = Arc::new(move |msgs: &[rustic_agent::Message]| {
                let persist_t0 = std::time::Instant::now();
                let Ok(db) = persist_db_arc.lock() else {
                    tracing::error!(
                        target: "rustic::persist",
                        task = %persist_task_id,
                        "persist callback: db lock failed — messages NOT saved"
                    );
                    return;
                };
                let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
                let mut rows = Vec::with_capacity(msgs.len());
                let mut skipped = 0usize;
                let mut total_bytes: usize = 0;
                for (i, msg) in msgs.iter().enumerate() {
                    let role = match &msg.role {
                        rustic_agent::Role::User => "user",
                        rustic_agent::Role::Assistant => "assistant",
                        rustic_agent::Role::System => "system",
                    };
                    let content_json = match serde_json::to_string(&msg.content) {
                        Ok(s) => s,
                        Err(e) => {
                            tracing::warn!(
                                target: "rustic::persist",
                                task = %persist_task_id,
                                idx = i,
                                error = %e,
                                "persist callback: skipped a message (content serialize failed)"
                            );
                            skipped += 1;
                            continue;
                        }
                    };
                    total_bytes += content_json.len();
                    rows.push(MessageRow {
                        id: format!("{}-{}", persist_task_id, i),
                        task_id: persist_task_id.clone(),
                        role: role.to_string(),
                        content_json,
                        created_at: now.clone(),
                        sort_order: i as i64,
                        // turn_usage is stamped onto the trailing user message
                        // only at end-of-turn (via the cumulative replace at
                        // line ~1080); the incremental saves omit it because
                        // we don't have the post-call turn_cost yet.
                        turn_usage_json: None,
                    });
                }
                let serialize_ms = persist_t0.elapsed().as_millis();
                let db_t0 = std::time::Instant::now();
                match db.replace_messages_for_task(&persist_task_id, &rows) {
                    Ok(()) => {
                        let db_ms = db_t0.elapsed().as_millis();
                        tracing::info!(
                            target: "rustic::persist",
                            task = %persist_task_id,
                            rows = rows.len(),
                            skipped,
                            bytes = total_bytes,
                            serialize_ms,
                            db_ms,
                            "persist callback: wrote messages"
                        );
                        // The persist path was the prime suspect for the
                        // freeze + disk thrash. A loud line above 200ms or
                        // 1 MB makes regressions easy to spot in logs.
                        if total_bytes > 1_000_000 || db_ms > 200 {
                            tracing::warn!(
                                target: "rustic::persist",
                                task = %persist_task_id,
                                rows = rows.len(),
                                bytes = total_bytes,
                                serialize_ms,
                                db_ms,
                                "persist callback: SLOW or LARGE write"
                            );
                        }
                    }
                    Err(e) => {
                        tracing::error!(
                            target: "rustic::persist",
                            task = %persist_task_id,
                            rows = rows.len(),
                            error = %e,
                            "persist callback: DB write FAILED"
                        );
                    }
                }
            });

            // P1.3: hand the executor the shared per-project WorkspaceServices.
            // Looking it up here (rather than once at project-open time) keeps
            // the registry the single source of truth even when send_message
            // is the first thing to touch the project after a restart. Sub-
            // agents inherit this Arc; multiple concurrent tasks in the same
            // project get the same instance.
            let workspace_services = workspace_services_registry
                .get_or_create(&PathBuf::from(&project_root));
            let context = ToolContext {
                project_root: PathBuf::from(&project_root),
                shared_permissions: shared_perms.clone(),
                persist_messages_fn: Some(persist_messages_fn),
                cancel_token: Some(Arc::clone(&cancel_token)),
                permission_broker,
                event_tx: event_tx.clone(),
                task_id: task_id_clone.clone(),
                file_lock: Arc::clone(&file_lock),
                file_read_registry: Arc::new(rustic_agent::FileReadRegistry::new()),
                mcp_manager: Some(Arc::clone(&mcp_manager_arc)),
                mcp_tool_defs,
                subagent_registry: Arc::clone(&subagent_registry),
                agent_depth: 0,
                ai_config: Arc::clone(&ai_config),
                tool_config: Arc::clone(&tool_config),
                allowed_paths,
                parent_provider_config: Some(parent_provider_config),
                subagent_provider_config: subagent_provider_config.clone(),
                write_scope: None, // main agent: unrestricted
                blocked_writes: Arc::new(std::sync::Mutex::new(Vec::new())),
                agent_terminals: Some(Arc::new(crate::commands::agent_terminals::TauriAgentTerminals::new(app_clone.clone())) as Arc<dyn rustic_agent::AgentTerminals>),
                is_global,
                // P0.3: plan-mode flag captured at send-message time. The
                // `set_task_plan_mode` Tauri command flips this on the task
                // record; next turn picks it up here. The executor gates
                // every non-read-only tool when true.
                is_plan_mode: task_is_plan_mode,
                // P0.4: global concurrency cap + daily cost ceiling. Pulled
                // from ai_config's BudgetSettings; an unrestricted Budget is
                // used if the user hasn't configured one. Hot-reloads from
                // `ai_config.budget` on every `send_message`.
                budget: rustic_agent::budget::Budget::new(&ai_config.budget),
                // P0.2: shared broker for the ask_user tool. Same handle
                // used across all tasks + their sub-agents so the dialog
                // flow is uniform regardless of which agent fired the call.
                ask_user_broker: ask_user_broker.clone(),
                // P0.4 fix #4: same shared-handle pattern for the
                // ceiling-breach broker.
                ceiling_broker: ceiling_broker.clone(),
                orchestrator_host,
                file_history: fh_handle_opt.as_ref().map(|h| h.history.clone()),
                sweep_worker: fh_handle_opt.as_ref().map(|h| h.sweep.clone()),
                current_user_message_id: fh_handle_opt.as_ref().map(|_| snapshot_message_id.clone()),
                // Drained by run_turn into TaskCost after each tool batch.
                tool_cost_sink: Arc::new(std::sync::Mutex::new(0.0)),
                // P1.3: shared per-project state, owned by the WorkspaceRegistry.
                workspace_services,
                // C3.4: pass the registry through so worktree-override
                // sub-agent spawns can mint their own per-worktree services
                // instead of inheriting the parent's.
                workspace_registry: Arc::clone(&workspace_services_registry),
                // P1.6: main agents are never sub-agents.
                subagent_self: None,
                // P1.7: per-task set of deferred tools the model has fetched
                // via `tool_search`. Starts empty; the executor reads this
                // at the top of every turn to know which tools (in addition
                // to `tool_search::ALWAYS_ON`) deserve their full schema in
                // the request. Sub-agents share this Arc with their parent.
                loaded_deferred_tools: Arc::new(std::sync::Mutex::new(
                    std::collections::HashSet::new(),
                )),
            };

            // Capture the cumulative cost from all previous turns for this task.
            // executor.rs resets task_cost to zero on each run_turn call, so CostUpdate
            // events only carry the per-turn incremental cost. Adding this baseline
            // at every CostUpdate gives the true cumulative total across all turns.
            let base_cost: TaskCost = {
                if let Ok(map) = task_costs_arc.lock() {
                    map.get(&task_id_clone).cloned().unwrap_or_default()
                } else {
                    TaskCost::default()
                }
            };

            // Forward events to Tauri
            let app_events = app_clone.clone();
            let cost_map = Arc::clone(&task_costs_arc);
            let cost_db = Arc::clone(&db_arc);
            // Track thinking start time so we can stamp duration_secs on thinking blocks
            let thinking_start: Arc<std::sync::Mutex<Option<std::time::Instant>>> =
                Arc::new(std::sync::Mutex::new(None));
            let thinking_durations: Arc<std::sync::Mutex<Vec<u64>>> =
                Arc::new(std::sync::Mutex::new(Vec::new()));
            let durations_for_persist = Arc::clone(&thinking_durations);
            // Per-sub-agent tool-call buffer. Mirrors the frontend's
            // `subagents.toolCalls` array: each entry is an object with
            // tool_use_id / tool_name / input / result / is_error. Updated on
            // every SubagentToolUse / SubagentToolResult event and serialized
            // to the `tool_calls_json` column so the activity panel can
            // restore the run after a restart.
            let mut subagent_tool_calls: std::collections::HashMap<(String, String), Vec<serde_json::Value>> =
                std::collections::HashMap::new();
            tokio::spawn(async move {
                while let Some(event) = event_rx.recv().await {
                    match event {
                        TaskEvent::TextDelta { task_id, text } => {
                            // A text delta means thinking is done — record duration
                            if let Ok(mut start) = thinking_start.lock() {
                                if let Some(t) = start.take() {
                                    let secs = t.elapsed().as_secs();
                                    if let Ok(mut durations) = thinking_durations.lock() {
                                        durations.push(secs);
                                    }
                                    let _ = app_events.emit("agent-thinking-done", AgentThinkingDoneEvent { task_id: task_id.clone(), duration_secs: secs });
                                }
                            }
                            let _ = app_events.emit("agent-stream", AgentStreamEvent { task_id, text });
                        }
                        TaskEvent::ThinkingDelta { task_id, text } => {
                            // Record start time on first thinking delta
                            if let Ok(mut start) = thinking_start.lock() {
                                if start.is_none() {
                                    *start = Some(std::time::Instant::now());
                                }
                            }
                            let _ = app_events.emit("agent-thinking-delta", AgentThinkingDeltaEvent { task_id, text });
                        }
                        TaskEvent::ToolUseStart { task_id, tool_use_id, tool_name } => {
                            // Same "thinking is done" semantics as ToolUse below —
                            // the model has stopped emitting reasoning and started
                            // calling tools. Stamp duration and notify UI.
                            if let Ok(mut start) = thinking_start.lock() {
                                if let Some(t) = start.take() {
                                    let secs = t.elapsed().as_secs();
                                    if let Ok(mut durations) = thinking_durations.lock() {
                                        durations.push(secs);
                                    }
                                    let _ = app_events.emit("agent-thinking-done", AgentThinkingDoneEvent { task_id: task_id.clone(), duration_secs: secs });
                                }
                            }
                            let _ = app_events.emit("agent-tool-use-start", AgentToolUseStartEvent { task_id, tool_use_id, tool_name });
                        }
                        TaskEvent::ToolUseInputDelta { task_id, tool_use_id, partial_json } => {
                            let _ = app_events.emit("agent-tool-use-input-delta", AgentToolUseInputDeltaEvent { task_id, tool_use_id, partial_json });
                        }
                        TaskEvent::ToolUseStop { task_id, tool_use_id } => {
                            let _ = app_events.emit("agent-tool-use-stop", AgentToolUseStopEvent { task_id, tool_use_id });
                        }
                        TaskEvent::ToolUse { task_id, tool_use_id, tool_name, tool_input } => {
                            // A tool use means thinking is done — record duration
                            if let Ok(mut start) = thinking_start.lock() {
                                if let Some(t) = start.take() {
                                    let secs = t.elapsed().as_secs();
                                    if let Ok(mut durations) = thinking_durations.lock() {
                                        durations.push(secs);
                                    }
                                    let _ = app_events.emit("agent-thinking-done", AgentThinkingDoneEvent { task_id: task_id.clone(), duration_secs: secs });
                                }
                            }
                            let _ = app_events.emit("agent-tool-use", AgentToolUseEvent { task_id, tool_use_id, tool_name, tool_input });
                        }
                        TaskEvent::ToolResult { task_id, tool_use_id, output, is_error } => {
                            let _ = app_events.emit("agent-tool-result", AgentToolResultEvent { task_id, tool_use_id, output, is_error });
                        }
                        TaskEvent::StatusChange { task_id, status } => {
                            let _ = app_events.emit("agent-task-status", AgentStatusEvent { task_id, status });
                        }
                        TaskEvent::TaskComplete { task_id, summary } => {
                            let _ = app_events.emit("agent-task-complete", AgentTaskCompleteEvent {
                                task_id,
                                summary,
                            });
                        }
                        TaskEvent::PermissionRequest { task_id, request_id, operation, description, preview } => {
                            let _ = app_events.emit("agent-permission-request", AgentPermissionRequestEvent {
                                task_id,
                                request_id,
                                operation,
                                description,
                                preview,
                            });
                        }
                        TaskEvent::AskUserRequest { task_id, request_id, questions } => {
                            // P0.2: forward to the frontend's tabbed dialog.
                            // Payload mirrors the tool input shape so the dialog
                            // just renders the questions verbatim.
                            let _ = app_events.emit(
                                "agent-ask-user-request",
                                serde_json::json!({
                                    "task_id": task_id,
                                    "request_id": request_id,
                                    "questions": questions,
                                }),
                            );
                        }
                        TaskEvent::CeilingBreached { task_id, request_id, ceiling_cents, spent_cents } => {
                            // P0.4 fix #4: the daily-cost ceiling tripped
                            // at the top of a turn. The executor has
                            // parked on the broker; the frontend renders
                            // a modal with "Raise ceiling to …" / "Stop"
                            // and responds via `respond_to_ceiling_breach`.
                            let _ = app_events.emit(
                                "agent-ceiling-breached",
                                serde_json::json!({
                                    "task_id": task_id,
                                    "request_id": request_id,
                                    "ceiling_cents": ceiling_cents,
                                    "spent_cents": spent_cents,
                                }),
                            );
                        }
                        TaskEvent::StreamRetry { task_id, attempt, max_attempts, waiting_ms } => {
                            // P0.1: surface the retry attempt so the UI can
                            // show "retrying in <waiting_ms>" instead of a
                            // frozen spinner.
                            let _ = app_events.emit(
                                "agent-stream-retry",
                                serde_json::json!({
                                    "task_id": task_id,
                                    "attempt": attempt,
                                    "max_attempts": max_attempts,
                                    "waiting_ms": waiting_ms,
                                }),
                            );
                        }
                        TaskEvent::SubagentParkTimeout { task_id, running_agents, parked_minutes } => {
                            // P1.9: the executor has been parked on a
                            // wait-for-sub-agent for `parked_minutes` minutes.
                            // Frontend renders a banner so the user can decide
                            // to keep waiting or stop the task. The executor
                            // does NOT block on a response — it just keeps
                            // waiting in 30-min cycles; user can stop via the
                            // existing cancel button.
                            let _ = app_events.emit(
                                "agent-subagent-park-timeout",
                                serde_json::json!({
                                    "task_id": task_id,
                                    "running_agents": running_agents,
                                    "parked_minutes": parked_minutes,
                                }),
                            );
                        }
                        TaskEvent::CostUpdate { task_id, cost } => {
                            // Combine the pre-turn baseline with the current turn's
                            // accumulated cost to produce a truly cumulative total.
                            let cumulative = TaskCost {
                                total_input_tokens: base_cost.total_input_tokens + cost.total_input_tokens,
                                total_output_tokens: base_cost.total_output_tokens + cost.total_output_tokens,
                                total_cache_read_tokens: base_cost.total_cache_read_tokens + cost.total_cache_read_tokens,
                                total_cache_write_tokens: base_cost.total_cache_write_tokens + cost.total_cache_write_tokens,
                                estimated_cost_usd: base_cost.estimated_cost_usd + cost.estimated_cost_usd,
                                turn_count: base_cost.turn_count + cost.turn_count,
                            };
                            if let Ok(mut map) = cost_map.lock() {
                                map.insert(task_id.clone(), cumulative.clone());
                            }
                            // Persist cumulative cost to DB
                            if let Ok(db) = cost_db.lock() {
                                let _ = db.update_task_cost(
                                    &task_id,
                                    cumulative.total_input_tokens as i64,
                                    cumulative.total_output_tokens as i64,
                                    cumulative.total_cache_read_tokens as i64,
                                    cumulative.estimated_cost_usd,
                                    cumulative.turn_count as i64,
                                );
                            }
                            let _ = app_events.emit("agent-cost-update", AgentCostUpdateEvent { task_id, cost: cumulative });
                        }
                        TaskEvent::RequestUsage { task_id, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, cost_usd } => {
                            let _ = app_events.emit("agent-request-usage", AgentRequestUsageEvent {
                                task_id,
                                input_tokens,
                                output_tokens,
                                cache_read_tokens,
                                cache_write_tokens,
                                cost_usd,
                            });
                        }
                        TaskEvent::MemoryUpdated { task_id } => {
                            let _ = app_events.emit("agent-memory-updated", AgentMemoryUpdatedEvent { task_id });
                        }
                        TaskEvent::FileTracked { task_id, message_id, kind, paths } => {
                            crate::commands::file_history::forward_file_tracked(
                                &app_events, task_id, message_id, kind, paths,
                            );
                        }
                        TaskEvent::SubagentSpawned { task_id, agent_id, model, prompt } => {
                            tracing::warn!("[tauri] subagent spawned: task={} agent={} model={}", task_id, agent_id, model);
                            // Persist the spawn so the card survives reload even
                            // if the sub-agent never completes (crash, cancel).
                            if let Ok(db) = cost_db.lock() {
                                let _ = db.upsert_subagent_spawn(&task_id, &agent_id, &model, &prompt);
                            }
                            let _ = app_events.emit("agent-subagent-spawned", AgentSubagentSpawnedEvent { task_id, agent_id, model, prompt });
                        }
                        TaskEvent::SubagentCompleted { task_id, agent_id, model, summary } => {
                            tracing::warn!("[tauri] subagent completed: task={} agent={} model={} summary_len={}", task_id, agent_id, model, summary.len());
                            if let Ok(db) = cost_db.lock() {
                                let _ = db.update_subagent_summary(&task_id, &agent_id, &summary);
                            }
                            let _ = app_events.emit("agent-subagent-completed", AgentSubagentCompletedEvent { task_id, agent_id, model, summary });
                        }
                        TaskEvent::SubagentFailed { task_id, agent_id, error } => {
                            tracing::warn!("[tauri] subagent failed: task={} agent={} error={}", task_id, agent_id, error);
                            if let Ok(db) = cost_db.lock() {
                                let _ = db.update_subagent_error(&task_id, &agent_id, &error);
                            }
                            let _ = app_events.emit("agent-subagent-failed", AgentSubagentFailedEvent { task_id, agent_id, error });
                        }
                        TaskEvent::SubagentTextDelta { task_id, agent_id, text } => {
                            // Append the streamed delta to the durable output
                            // buffer so the activity panel can replay this
                            // run after a restart.
                            if let Ok(db) = cost_db.lock() {
                                let _ = db.append_subagent_output(&task_id, &agent_id, &text);
                            }
                            let _ = app_events.emit("agent-subagent-text-delta", AgentSubagentTextDeltaEvent { task_id, agent_id, text });
                        }
                        TaskEvent::SubagentCostUpdate { task_id, agent_id, cost } => {
                            tracing::warn!("[tauri] subagent cost update: task={} agent={} in={} out={} usd={:.4}",
                                task_id, agent_id, cost.total_input_tokens, cost.total_output_tokens, cost.estimated_cost_usd);
                            if let Ok(db) = cost_db.lock() {
                                let _ = db.update_subagent_cost(
                                    &task_id,
                                    &agent_id,
                                    cost.total_input_tokens as i64,
                                    cost.total_output_tokens as i64,
                                    cost.total_cache_read_tokens as i64,
                                    cost.estimated_cost_usd,
                                );
                            }
                            let _ = app_events.emit("agent-subagent-cost-update", AgentSubagentCostUpdateEvent { task_id, agent_id, cost });
                        }
                        TaskEvent::SubagentToolUse { task_id, agent_id, tool_name, tool_use_id, input } => {
                            let key = (task_id.clone(), agent_id.clone());
                            let entry = serde_json::json!({
                                "tool_use_id": tool_use_id,
                                "tool_name": tool_name,
                                "input": input,
                                "result": serde_json::Value::Null,
                                "is_error": false,
                            });
                            let calls = subagent_tool_calls.entry(key.clone()).or_insert_with(Vec::new);
                            calls.push(entry);
                            if let Ok(s) = serde_json::to_string(calls) {
                                if let Ok(db) = cost_db.lock() {
                                    let _ = db.set_subagent_tool_calls(&task_id, &agent_id, &s);
                                }
                            }
                            let _ = app_events.emit("agent-subagent-tool-use", AgentSubagentToolUseEvent { task_id, agent_id, tool_name, tool_use_id, input });
                        }
                        TaskEvent::SubagentToolResult { task_id, agent_id, tool_use_id, content, is_error } => {
                            let key = (task_id.clone(), agent_id.clone());
                            if let Some(calls) = subagent_tool_calls.get_mut(&key) {
                                if let Some(entry) = calls.iter_mut().rev().find(|c| {
                                    c.get("tool_use_id").and_then(|v| v.as_str()) == Some(tool_use_id.as_str())
                                }) {
                                    if let Some(obj) = entry.as_object_mut() {
                                        obj.insert("result".to_string(), serde_json::Value::String(content.clone()));
                                        obj.insert("is_error".to_string(), serde_json::Value::Bool(is_error));
                                    }
                                }
                                if let Ok(s) = serde_json::to_string(calls) {
                                    if let Ok(db) = cost_db.lock() {
                                        let _ = db.set_subagent_tool_calls(&task_id, &agent_id, &s);
                                    }
                                }
                            }
                            let _ = app_events.emit("agent-subagent-tool-result", AgentSubagentToolResultEvent { task_id, agent_id, tool_use_id, content, is_error });
                        }
                        TaskEvent::TodoUpdated { task_id, todos } => {
                            // Persist the list so it survives app restarts and task switches.
                            // The list is small (a few items), so the encode-and-write cost
                            // per `todo_write` call is negligible.
                            if let Ok(json) = serde_json::to_string(&todos) {
                                if let Ok(db) = cost_db.lock() {
                                    let _ = db.set_task_todos(&task_id, &json);
                                }
                            }
                            let _ = app_events.emit("agent-todo-updated", AgentTodoUpdatedEvent { task_id, todos });
                        }
                        TaskEvent::ToolProgress { task_id, tool_use_id, progress_text } => {
                            let _ = app_events.emit("agent-tool-progress", AgentToolProgressEvent { task_id, tool_use_id, progress_text });
                        }
                        TaskEvent::ContextCondenseStarted { task_id } => {
                            let _ = app_events.emit("agent-context-condense-started", AgentContextCondenseStartedEvent { task_id });
                        }
                        TaskEvent::ContextCondenseCompleted { task_id, original_messages, condensed_to } => {
                            let _ = app_events.emit("agent-context-condense-completed", AgentContextCondenseCompletedEvent { task_id, original_messages, condensed_to });
                        }
                        _ => {}
                    }
                }
            });

            // P1.8: branch on goal-mode flag. Default path is a single
            // `run_turn`. In goal mode we hand control to `run_goal_loop`
            // which iterates `run_turn` internally until the model calls
            // `goal_complete`, the iteration cap fires, or the user cancels.
            let (result, turn_cost) = if task_is_goal_mode {
                use rustic_agent::task::goal_loop::{run_goal_loop, GoalTermination};
                match run_goal_loop(
                    &executor,
                    &mut messages,
                    &context,
                    task_goal_iteration_cap,
                )
                .await
                {
                    Ok(outcome) => {
                        // Whatever the termination, the goal-mode session for
                        // this user-message is done. Reset the task's flag so
                        // a subsequent plain `send_message` doesn't re-enter
                        // the loop. Persist the latest cap so the next /goal
                        // call can default-keep it if the frontend doesn't
                        // re-supply one.
                        if let Ok(mut agent) = agent_arc.lock() {
                            if let Some(t) = agent.tasks.get_mut(&task_id_clone) {
                                t.is_goal_mode = false;
                            }
                        }
                        tracing::warn!(
                            "[goal_loop] task={} iterations={} termination={:?}",
                            task_id_clone,
                            outcome.iterations,
                            outcome.termination,
                        );
                        let res = match outcome.termination {
                            GoalTermination::Errored(e) => {
                                Err(anyhow::anyhow!(e))
                            }
                            _ => Ok(()),
                        };
                        (res, outcome.task_cost)
                    }
                    Err(e) => (Err(e), TaskCost::default()),
                }
            } else {
                match executor.run_turn(&mut messages, &context).await {
                    Ok(cost) => (Ok(()), cost),
                    Err(e) => (Err(e), TaskCost::default()),
                }
            };

            // Stamp duration_secs on thinking blocks before persisting
            if let Ok(durations) = durations_for_persist.lock() {
                let mut dur_idx = 0;
                for msg in messages.iter_mut() {
                    for block in msg.content.iter_mut() {
                        if let ContentBlock::Thinking { duration_secs, .. } = block {
                            if dur_idx < durations.len() {
                                *duration_secs = Some(durations[dur_idx]);
                                dur_idx += 1;
                            }
                        }
                    }
                }
            }

            // Build turn_usage_json for the last user message in this turn.
            // Each run_turn call handles one user turn, so the returned turn_cost is
            // exactly the token/cost totals for the most recent user message.
            let last_user_idx = messages.iter().rposition(|m| matches!(m.role, Role::User));
            let turn_usage_json = serde_json::to_string(&TurnUsage {
                input: turn_cost.total_input_tokens as i64,
                output: turn_cost.total_output_tokens as i64,
                cache_read: turn_cost.total_cache_read_tokens as i64,
                cache_write: turn_cost.total_cache_write_tokens as i64,
                cost: turn_cost.estimated_cost_usd,
            }).ok();

            // Persist all messages to DB after the turn completes. Wrapped in
            // a single transaction (via `replace_messages_for_task`) so a
            // mid-write crash — most commonly: the user clicks Stop and
            // immediately closes the app while the worker thread is still
            // deleting the prior history — doesn't wipe the table. Before the
            // transaction the DELETE auto-committed and the OS could kill
            // the process before any INSERT ran, leaving the user with a
            // task row that had cost / turn-count metadata but zero messages.
            if let Ok(db) = db_arc.lock() {
                let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
                let mut rows = Vec::with_capacity(messages.len());
                for (i, msg) in messages.iter().enumerate() {
                    let role = match &msg.role {
                        Role::User => "user",
                        Role::Assistant => "assistant",
                        Role::System => "system",
                    };
                    let msg_turn_usage = if Some(i) == last_user_idx {
                        turn_usage_json.clone()
                    } else {
                        None
                    };
                    let content_json = match serde_json::to_string(&msg.content) {
                        Ok(s) => s,
                        Err(_) => continue,
                    };
                    rows.push(MessageRow {
                        id: format!("{}-{}", task_id_clone, i),
                        task_id: task_id_clone.clone(),
                        role: role.to_string(),
                        content_json,
                        created_at: now.clone(),
                        sort_order: i as i64,
                        turn_usage_json: msg_turn_usage,
                    });
                }
                let _ = db.replace_messages_for_task(&task_id_clone, &rows);
            }

            // Sync in-memory task messages with the executor's complete history.
            // Without this, the in-memory cache is stale (missing assistant responses,
            // tool calls, thinking blocks) and get_task_messages would return incomplete data.
            if let Ok(mut agent) = agent_arc.lock() {
                if let Some(task) = agent.tasks.get_mut(&task_id_clone) {
                    task.messages = messages.clone();
                }
            }

            // Snapshot the worktree state at the end of this turn. The Changed
            // Files panel compares against this oid for idle tasks instead of
            // live disk, so external edits made after the turn don't show up.
            if let Some(ref fh_handle) = fh_handle_opt {
                let h = fh_handle.history.clone();
                let tid = task_id_clone.clone();
                if let Err(e) = tokio::task::spawn_blocking(move || h.record_final_state(&tid))
                    .await
                    .unwrap_or_else(|e| Err(rustic_agent::FileHistoryError::Io(
                        std::io::Error::new(std::io::ErrorKind::Other, e.to_string())
                    )))
                {
                    tracing::warn!(task = %task_id_clone, ?e, "record_final_state failed; Changed Files may show external edits");
                }
            }

            // Check if the task was cancelled before deciding on final status
            let was_cancelled = cancel_token.load(Ordering::SeqCst);

            let final_status = if was_cancelled {
                TaskStatus::Cancelled
            } else {
                match result {
                    Ok(()) => TaskStatus::Completed,
                    Err(e) => {
                        let _ = app_clone.emit("agent-stream", AgentStreamEvent {
                            task_id: task_id_clone.clone(),
                            text: format!("\n\nError: {}", e),
                        });
                        TaskStatus::Failed
                    }
                }
            };

            // Emit agent-task-complete directly from the outer task on Completed.
            // The inner event-processor task also does this via TaskEvent::TaskComplete, but it may
            // be killed before draining the channel. Emitting here is guaranteed.
            // No `summary` field — the model now ends with a plain-text final message
            // which renders as a normal assistant bubble.
            if final_status == TaskStatus::Completed {
                let _ = app_clone.emit("agent-task-complete", AgentTaskCompleteEvent {
                    task_id: task_id_clone.clone(),
                    summary: None,
                });
            }

            // Persist the terminal status to the DB. Without this, a task that
            // completes in the UI still reads as "Running" after a restart
            // because the row was only updated to "Running" at task start and
            // never transitioned. The defensive code in list_tasks() used to
            // paper over this — we now fix it at the source.
            {
                let status_str = match final_status {
                    TaskStatus::Completed => "Completed",
                    TaskStatus::Failed => "Failed",
                    TaskStatus::Cancelled => "Cancelled",
                    TaskStatus::Running => "Running",
                };
                if let Ok(db) = db_arc.lock() {
                    let _ = db.update_task_status(&task_id_clone, status_str);
                }
                // Also update the in-memory task info so live UI state matches.
                if let Ok(mut agent) = agent_arc.lock() {
                    if let Some(t) = agent.tasks.get_mut(&task_id_clone) {
                        t.info.status = final_status.clone();
                    }
                }
            }

            let _ = app_clone.emit("agent-task-status", AgentStatusEvent {
                task_id: task_id_clone,
                status: final_status,
            });
        });
    });

    Ok(())
}

#[tauri::command]
pub fn list_tasks(
    state: State<'_, AppState>,
    project_id: Option<String>,
) -> Result<Vec<TaskInfo>, String> {
    let db = state.db.lock().unwrap();
    let rows = if let Some(ref pid) = project_id {
        db.list_tasks_for_project(pid).map_err(|e| e.to_string())?
    } else {
        // No project filter: load all in-memory tasks as fallback
        let agent = state.agent.lock().unwrap();
        return Ok(agent.tasks.values().map(|t| t.info.clone()).collect());
    };
    drop(db);

    // Hydrate into in-memory agent state so tasks are accessible for send_message.
    // Treat any DB-persisted "Running" tasks as Completed — they cannot be running
    // after a fresh app start (they were left over from a crashed session).
    let db2 = state.db.lock().unwrap();
    for row in &rows {
        // Diagnostic: log how many messages each task currently has in the DB.
        // If this comes back 0 for a task that the user remembers having
        // history, the messages were never persisted (incremental save didn't
        // fire OR persistence committed but got truncated by a later wipe).
        let msg_count = db2
            .get_messages_for_task(&row.id)
            .map(|v| v.len())
            .unwrap_or(0);
        tracing::info!(
            target: "rustic::list_tasks",
            task = %row.id,
            db_status = %row.status,
            msg_count,
            turn_count = row.turn_count,
            cost = row.estimated_cost_usd,
            "list_tasks: hydrating row"
        );
        if row.status == "Running" {
            tracing::warn!(
                target: "rustic::list_tasks",
                task = %row.id,
                "found stale Running task on startup — defensively marking Completed",
            );
            let _ = db2.update_task_status(&row.id, "Completed");
        }
    }
    drop(db2);

    let mut agent = state.agent.lock().unwrap();
    for row in &rows {
        if !agent.tasks.contains_key(&row.id) {
            let status = match row.status.as_str() {
                // "Running" is intentionally omitted — treated as Completed (see above)
                "Failed" => TaskStatus::Failed,
                "Cancelled" => TaskStatus::Cancelled,
                _ => TaskStatus::Completed,
            };
            let permissions = agent
                .project_permissions
                .get(&row.project_id)
                .cloned()
                .unwrap_or_default();
            let cost = TaskCost {
                total_input_tokens: row.total_input_tokens as u64,
                total_output_tokens: row.total_output_tokens as u64,
                total_cache_read_tokens: row.total_cache_read_tokens as u64,
                total_cache_write_tokens: 0,
                estimated_cost_usd: row.estimated_cost_usd,
                turn_count: row.turn_count as u32,
            };
            // Also populate the in-memory cost map so get_task_cost works
            if let Ok(mut cost_map) = state.task_costs.lock() {
                cost_map.entry(row.id.clone()).or_insert_with(|| cost.clone());
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
                    },
                    messages: Vec::new(),
                    permissions,
                    sensitive_files_allowed: false,
                    is_plan_mode: false,
                    is_goal_mode: false,
                    goal_iteration_cap: 0,
                    shared_permissions: None,
                    cost,
                },
            );
        }
    }

    Ok(rows
        .iter()
        .map(|row| {
            agent.tasks[&row.id].info.clone()
        })
        .collect())
}

/// Returns true for a text block that is a synthetic executor injection.
/// These text blocks appear inside user messages (sometimes alongside real
/// tool_result blocks) and must be stripped from DTOs sent to the frontend.
fn is_synthetic_text_block(block: &ContentBlock) -> bool {
    let text = match block {
        ContentBlock::Text { text } => text.as_str(),
        _ => return false,
    };
    text.starts_with("[Sub-agent '")
        || text.starts_with("[All sub-agents")
        || text.starts_with("[SYSTEM NUDGE")
        || text.starts_with("[Messages from orchestrator]")
        || text.starts_with("SYSTEM: one or more background terminals")
}

/// Strip synthetic injection text blocks from a user message's content.
/// Returns the cleaned content (tool_result blocks and real text kept).
/// Returns None only when the message becomes completely empty (was made up
/// entirely of synthetic text blocks).
///
/// Do NOT drop messages that become pure tool_result content — the frontend's
/// buildResultMap scans task.messages to resolve tool cards, and removing those
/// messages would make every tool card render as interrupted instead of
/// completed. The rendering pipeline already skips pure-tool-result user
/// messages as visible bubbles, so keeping them has no visual impact.
fn strip_synthetic_blocks(msg: &Message) -> Option<Vec<ContentBlock>> {
    if !matches!(msg.role, Role::User) {
        return Some(msg.content.clone());
    }
    let clean: Vec<ContentBlock> = msg.content.iter()
        .filter(|b| !is_synthetic_text_block(b))
        .cloned()
        .collect();
    if clean.is_empty() {
        return None; // was entirely synthetic text — drop
    }
    Some(clean)
}

#[tauri::command]
pub fn get_task_messages(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<Vec<MessageDto>, String> {
    // If task is in memory and has messages, return those (turn_usage not included
    // here — the frontend already has it from the live session's RequestUsage events).
    // Synthetic executor-injected user messages (sub-agent result blocks, terminal
    // exit notices, orchestrator nudges) are stripped from the DTO so the frontend
    // never renders them as visible chat bubbles — they were never emitted as stream
    // events during live execution so the UI didn't show them then either.
    {
        let agent = state.agent.lock().unwrap();
        if let Some(task) = agent.tasks.get(&task_id) {
            if !task.messages.is_empty() {
                let dtos = task.messages.iter()
                    .filter_map(|m| {
                        let clean = strip_synthetic_blocks(m)?;
                        Some(MessageDto {
                            role: match m.role {
                                Role::User => "user".to_string(),
                                Role::Assistant => "assistant".to_string(),
                                Role::System => "system".to_string(),
                            },
                            content: clean,
                            turn_usage: None,
                        })
                    }).collect();
                return Ok(dtos);
            }
        }
    }

    // Load from DB and deserialize
    let db = state.db.lock().unwrap();
    let rows = db.get_messages_for_task(&task_id).map_err(|e| e.to_string())?;
    drop(db);

    let mut dtos = Vec::new();
    let mut messages_for_cache = Vec::new();
    for row in &rows {
        let role = match row.role.as_str() {
            "user" => Role::User,
            _ => Role::Assistant,
        };
        let content: Vec<ContentBlock> = serde_json::from_str(&row.content_json)
            .unwrap_or_else(|e| {
                tracing::warn!("[get_task_messages] Failed to deserialize content_json for message {}: {}. Falling back to raw text.", row.id, e);
                vec![ContentBlock::Text { text: row.content_json.clone() }]
            });
        let turn_usage: Option<TurnUsage> = row.turn_usage_json.as_deref()
            .and_then(|j| serde_json::from_str(j).ok());
        let msg_for_cache = Message { role, content: content.clone() };
        // Keep synthetic injections in the executor cache (it needs them for
        // context on the next turn) but exclude them from the DTO sent to the
        // frontend so they don't render as visible chat bubbles.
        if let Some(clean_content) = strip_synthetic_blocks(&msg_for_cache) {
            dtos.push(MessageDto {
                role: row.role.clone(),
                content: clean_content,
                turn_usage,
            });
        }
        messages_for_cache.push(msg_for_cache);
    }

    // Hydrate into in-memory task if it exists
    if !rows.is_empty() {
        let mut agent = state.agent.lock().unwrap();
        if let Some(task) = agent.tasks.get_mut(&task_id) {
            task.messages = messages_for_cache;
        }
    }

    Ok(dtos)
}

/// Read the persisted todo list for a task. Returns an empty list when the
/// task hasn't written todos yet — the frontend treats that the same as no
/// list. The list is small so we deserialize on every call rather than caching.
#[tauri::command]
pub fn get_task_todos(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<Vec<TodoItem>, String> {
    let db = state.db.lock().unwrap();
    let json = db.get_task_todos(&task_id).map_err(|e| e.to_string())?;
    drop(db);
    let Some(json) = json else { return Ok(Vec::new()); };
    serde_json::from_str::<Vec<TodoItem>>(&json).map_err(|e| e.to_string())
}

/// Truncate a task's chat history to the first `keep_count` messages. Both
/// the in-memory `task.messages` vec and the persisted `messages` table are
/// trimmed in lockstep so the UI's chat re-renders to the truncated state and
/// the next turn the model sees won't include the discarded turns.
///
/// Used by the per-message revert: when the user picks "revert chat (only)"
/// from message N (zero-indexed), the frontend asks for `keep_count = N` so
/// message N and every later message are dropped — the user message text is
/// loaded back into the input box on the frontend side.
#[tauri::command]
pub fn truncate_task_messages(
    state: State<'_, AppState>,
    task_id: String,
    keep_count: usize,
) -> Result<(), String> {
    {
        let mut agent = state
            .agent
            .lock()
            .map_err(|e| format!("agent mutex poisoned: {e}"))?;
        if let Some(task) = agent.tasks.get_mut(&task_id) {
            if keep_count < task.messages.len() {
                task.messages.truncate(keep_count);
            }
        }
    }
    let db = state
        .db
        .lock()
        .map_err(|e| format!("db mutex poisoned: {e}"))?;
    db.truncate_messages_from(&task_id, keep_count as i64)
        .map_err(|e| format!("truncate_messages_from: {e}"))?;
    Ok(())
}

#[tauri::command]
pub fn delete_task(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<(), String> {
    // Tear down any live harness CLI process before removing the task row.
    // Without this, deleting a task tab while a `claude` child is mid-turn
    // would orphan the process (until app quit, where `shutdown_all` would
    // catch it). Best-effort; failure here doesn't block the DB delete.
    shutdown_harness_for_task(&state, &task_id);

    // Look up the task's project_root before we drop the in-memory record
    // so we can wipe any media (image_create / video_create / animate)
    // outputs the task wrote to disk. Without this the .rustic/generated_*
    // folders accumulate forever — one folder per deleted chat.
    let project_root_for_cleanup: Option<String> = {
        let agent = state.agent.lock().unwrap();
        let project_id = agent
            .tasks
            .get(&task_id)
            .map(|t| t.info.project_id.clone());
        drop(agent);
        project_id.and_then(|pid| {
            let ws = state.workspace.lock().ok()?;
            ws.list_projects()
                .into_iter()
                .find(|p| p.id.to_string() == pid)
                .map(|p| p.root_path.to_string_lossy().to_string())
        })
    };

    let mut agent = state.agent.lock().unwrap();
    agent.tasks.remove(&task_id);
    // Drop any session permission rules the user approved for this task —
    // a deleted task shouldn't carry "Allow for session" state forward
    // (plan §B.3). Cheap; no-op if the task never granted any.
    agent.permission_broker.clear_for_task(&task_id);
    drop(agent);
    let db = state.db.lock().unwrap();
    let _ = db.delete_messages_for_task(&task_id);
    let _ = db.delete_task(&task_id);
    drop(db);

    if let Some(root) = project_root_for_cleanup {
        wipe_task_media_dirs(&root, &task_id);
    }

    Ok(())
}

/// Remove `.rustic/generated_images/<task_id>/` and
/// `.rustic/generated_videos/<task_id>/` from a project root. Called when a
/// task is deleted so per-chat media is cleaned up alongside its messages.
/// Best-effort — log on failure, don't fail the delete.
fn wipe_task_media_dirs(project_root: &str, task_id: &str) {
    use std::path::PathBuf;
    // Defensive task_id sanitisation. Media tools already sanitise the same
    // way before writing; we mirror that here so we look in the right
    // directory.
    let safe_task: String = task_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }
        })
        .collect();
    let root = PathBuf::from(project_root);
    for sub in &[".rustic/generated_images", ".rustic/generated_videos"] {
        let dir = root.join(sub).join(&safe_task);
        if dir.exists() {
            if let Err(e) = std::fs::remove_dir_all(&dir) {
                tracing::warn!(
                    task = %task_id,
                    dir = %dir.display(),
                    error = %e,
                    "wipe_task_media_dirs: failed to remove"
                );
            }
        }
    }
}

/// Synchronously kill the harness session for `task_id` (if any). Spawns a
/// short-lived tokio runtime because the registry methods are async and the
/// Tauri command surface is sync.
fn shutdown_harness_for_task(state: &State<'_, AppState>, task_id: &str) {
    let registry = state.harness_registry.clone();
    let task_id = task_id.to_string();
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            tracing::error!(error = %e, "shutdown_harness_for_task: tokio init failed");
            return;
        }
    };
    rt.block_on(async move {
        if let Some(session) = registry.remove(&task_id).await {
            if let Err(e) = session.shutdown().await {
                tracing::warn!(task = %task_id, error = %e, "harness shutdown on delete failed");
            }
        }
    });
}

#[tauri::command]
pub fn delete_tasks_for_project(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<(), String> {
    // Snapshot the task ids before removing them from the in-memory map so
    // we can shut down their harness sessions (if any). Skipping this would
    // orphan `claude` processes for every project-wide delete.
    let project_task_ids: Vec<String> = {
        let agent = state.agent.lock().unwrap();
        agent
            .tasks
            .iter()
            .filter(|(_, t)| t.info.project_id == project_id)
            .map(|(id, _)| id.clone())
            .collect()
    };
    for tid in &project_task_ids {
        shutdown_harness_for_task(&state, tid);
    }

    // Capture project_root so we can wipe each task's media output dirs
    // alongside its DB rows.
    let project_root_for_cleanup: Option<String> = {
        let ws = state.workspace.lock().ok();
        ws.and_then(|w| {
            w.list_projects()
                .into_iter()
                .find(|p| p.id.to_string() == project_id)
                .map(|p| p.root_path.to_string_lossy().to_string())
        })
    };

    let mut agent = state.agent.lock().unwrap();
    agent.tasks.retain(|_, t| t.info.project_id != project_id);
    // Drop session permission rules for every task we just removed — same
    // rationale as in `delete_task`.
    for tid in &project_task_ids {
        agent.permission_broker.clear_for_task(tid);
    }
    drop(agent);
    let db = state.db.lock().unwrap();
    let _ = db.delete_tasks_for_project(&project_id);
    drop(db);

    if let Some(root) = project_root_for_cleanup {
        for tid in &project_task_ids {
            wipe_task_media_dirs(&root, tid);
        }
    }

    Ok(())
}

#[tauri::command]
pub fn rename_task(
    state: State<'_, AppState>,
    task_id: String,
    title: String,
) -> Result<(), String> {
    let mut agent = state.agent.lock().unwrap();
    if let Some(task) = agent.tasks.get_mut(&task_id) {
        task.info.title = title.clone();
    }
    drop(agent);
    let db = state.db.lock().unwrap();
    db.update_task_title(&task_id, &title).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_ai_provider(
    state: State<'_, AppState>,
    provider_type: String,
    api_key: String,
    model: String,
    base_url: Option<String>,
    large_context: Option<bool>,
    custom_max_output_tokens: Option<u32>,
    custom_input_cost: Option<f64>,
    custom_output_cost: Option<f64>,
    custom_cached_input_cost: Option<f64>,
    custom_cached_output_cost: Option<f64>,
    custom_context_window: Option<u32>,
    custom_thinking_budget: Option<u32>,
    name: Option<String>,
) -> Result<(), String> {
    let mut agent = state.agent.lock().unwrap();

    let pt = match provider_type.as_str() {
        "Claude" => ProviderType::Claude,
        "OpenAi" => ProviderType::OpenAi,
        "Gemini" => ProviderType::Gemini,
        "Compatible" => ProviderType::Compatible,
        "OpenRouter" => ProviderType::OpenRouter,
        "ClaudeCode" => ProviderType::ClaudeCode,
        "Codex" => ProviderType::Codex,
        _ => return Err(format!("Unknown provider type: {}", provider_type)),
    };

    let base_url = if matches!(pt, ProviderType::OpenRouter) {
        Some("https://openrouter.ai/api/v1".to_string())
    } else {
        base_url
    };

    // Normalize the instance name (only meaningful for Compatible)
    let entry_name: Option<String> = if matches!(pt, ProviderType::Compatible) {
        name.as_ref()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    } else {
        None
    };

    // Upsert key: for Compatible with a name, key by (type + provider_key) so
    // that a re-connect with display name "Groq" updates the same row that was
    // saved earlier as "groq" (frontend slugifies; backend now agrees).
    // Otherwise behave as before (one entry per provider type).
    let target_key: Option<String> = if matches!(pt, ProviderType::Compatible) {
        let probe = rustic_agent::ProviderEntry {
            provider_type: ProviderType::Compatible,
            api_key: String::new(),
            default_model: String::new(),
            base_url: None,
            enabled: false,
            large_context: false,
            custom_max_output_tokens: 0,
            custom_input_cost: 0.0,
            custom_output_cost: 0.0,
            custom_cached_input_cost: 0.0,
            custom_cached_output_cost: 0.0,
            custom_context_window: 0,
            custom_thinking_budget: 0,
            name: entry_name.clone(),
        };
        Some(probe.provider_key())
    } else {
        None
    };

    let matches_idx = agent.ai_config.providers.iter().position(|p| {
        if p.provider_type != pt {
            return false;
        }
        match &pt {
            ProviderType::Compatible => match &target_key {
                Some(k) => p.provider_key() == *k,
                None => p.name.is_none(),
            },
            _ => true,
        }
    });

    if let Some(idx) = matches_idx {
        let entry = &mut agent.ai_config.providers[idx];
        // Sentinel passed back from `get_ai_config` means "keep existing key".
        // The webview never sees the real key, so it can't echo it.
        if api_key != "__STORED__" {
            entry.api_key = api_key;
        }
        entry.default_model = model;
        entry.base_url = base_url;
        entry.enabled = true;
        entry.name = entry_name;
        if let Some(lc) = large_context { entry.large_context = lc; }
        if let Some(v) = custom_max_output_tokens { entry.custom_max_output_tokens = v; }
        if let Some(v) = custom_input_cost { entry.custom_input_cost = v; }
        if let Some(v) = custom_output_cost { entry.custom_output_cost = v; }
        if let Some(v) = custom_cached_input_cost { entry.custom_cached_input_cost = v; }
        if let Some(v) = custom_cached_output_cost { entry.custom_cached_output_cost = v; }
        if let Some(v) = custom_context_window { entry.custom_context_window = v; }
        if let Some(v) = custom_thinking_budget { entry.custom_thinking_budget = v; }
    } else {
        agent.ai_config.providers.push(rustic_agent::ProviderEntry {
            provider_type: pt.clone(),
            api_key,
            default_model: model,
            base_url,
            enabled: true,
            large_context: large_context.unwrap_or(false),
            custom_max_output_tokens: custom_max_output_tokens.unwrap_or(0),
            custom_input_cost: custom_input_cost.unwrap_or(0.0),
            custom_output_cost: custom_output_cost.unwrap_or(0.0),
            custom_cached_input_cost: custom_cached_input_cost.unwrap_or(0.0),
            custom_cached_output_cost: custom_cached_output_cost.unwrap_or(0.0),
            custom_context_window: custom_context_window.unwrap_or(0),
            custom_thinking_budget: custom_thinking_budget.unwrap_or(0),
            name: entry_name,
        });
    }

    if agent.ai_config.default_provider.is_none() {
        agent.ai_config.default_provider = Some(pt);
    }

    // Persist: write keys to OS keychain, then save the rest of the config
    // (with api_key fields blanked) to SQLite. The in-memory `ai_config`
    // keeps the real keys so the agent can read them directly.
    {
        let mut redacted = agent.ai_config.clone();
        tracing::info!(
            total_providers = redacted.providers.len(),
            "[secrets] set_ai_provider: persisting all providers to keychain"
        );
        for entry in redacted.providers.iter_mut() {
            if entry.api_key.is_empty() {
                continue;
            }
            let provider_str = entry.provider_type.as_str();
            let acct = crate::secrets::provider_account(provider_str, entry.name.as_deref());
            // Best-effort: if the keychain is unavailable, fall back to leaving
            // the key in SQLite for this run only and warn. Don't block the
            // user's save.
            match crate::secrets::set(&acct, &entry.api_key) {
                Ok(()) => {
                    entry.api_key.clear();
                    tracing::info!(account = %acct, "[secrets] keychain set OK");
                }
                Err(e) => {
                    // Don't clear — leave the key in the redacted clone so
                    // SQLite retains a fallback copy. Better plaintext than lost.
                    tracing::error!(account = %acct, error = %e, "[secrets] KEYCHAIN SET FAILED — falling back to plaintext in SQLite");
                }
            }
        }
        let config_json = serde_json::to_string(&redacted).map_err(|e| e.to_string())?;
        drop(agent);
        let db = state.db.lock().unwrap();
        db.set_setting("ai_config", &config_json).map_err(|e| e.to_string())?;
    }

    Ok(())
}

/// Update the per-model capability flags. Called by the frontend's "Register
/// model" / "Edit model" modal so a model that, say, rejects the
/// `temperature` field can have it suppressed without losing the model.
///
/// Each parameter is optional:
/// - `Some(value)` upserts the field on the existing entry (or creates one).
/// - `None` is treated as "leave that field alone."
/// - Passing `None` for *both* fields removes the entry entirely, reverting
///   the model to defaults — same shape the previous one-flag command used,
///   just generalised across multiple flags.
#[tauri::command]
pub fn set_model_capabilities(
    state: State<'_, AppState>,
    model_id: String,
    supports_temperature: Option<bool>,
    supports_reasoning_effort: Option<bool>,
) -> Result<(), String> {
    if model_id.trim().is_empty() {
        return Err("model_id is required".to_string());
    }

    let mut agent = state.agent.lock().unwrap();
    if supports_temperature.is_none() && supports_reasoning_effort.is_none() {
        // Nothing to apply — caller is asking us to drop any override on the
        // model so it picks up the defaults again.
        agent.ai_config.model_capabilities.remove(&model_id);
    } else {
        let entry = agent
            .ai_config
            .model_capabilities
            .entry(model_id.clone())
            .or_default();
        if let Some(v) = supports_temperature {
            entry.supports_temperature = v;
        }
        if let Some(v) = supports_reasoning_effort {
            entry.supports_reasoning_effort = v;
        }
    }

    // Persist alongside the rest of ai_config (same redacted-keys flow as
    // set_ai_provider — keys live in the keychain, not in the JSON).
    let mut redacted = agent.ai_config.clone();
    for entry in redacted.providers.iter_mut() {
        entry.api_key.clear();
    }
    let config_json = serde_json::to_string(&redacted).map_err(|e| e.to_string())?;
    drop(agent);
    let db = state.db.lock().unwrap();
    db.set_setting("ai_config", &config_json).map_err(|e| e.to_string())?;
    Ok(())
}

/// Return the per-model capabilities map, so the Edit-model modal can pre-
/// populate its toggle correctly.
#[tauri::command]
pub fn get_model_capabilities(
    state: State<'_, AppState>,
) -> Result<std::collections::HashMap<String, rustic_agent::ModelCapabilities>, String> {
    let agent = state.agent.lock().unwrap();
    Ok(agent.ai_config.model_capabilities.clone())
}

/// Configure the cheaper/faster sub-agent model. When set, the main agent's
/// `spawn_subagent` schema gains a `model_tier` parameter so it can route
/// individual sub-agents to either the main chat model or this one. Pass
/// `None` for both arguments via `clear_subagent_config` to remove the
/// override entirely.
///
/// The `provider_key` must match an existing entry's
/// `ProviderEntry::provider_key()` (e.g. `"Claude"`, `"Compatible:groq"`);
/// otherwise the override would point at nothing and silently fall back to
/// the main model on every call.
#[tauri::command]
pub fn set_subagent_config(
    state: State<'_, AppState>,
    provider_key: String,
    model: String,
) -> Result<(), String> {
    let provider_key = provider_key.trim().to_string();
    let model = model.trim().to_string();
    if provider_key.is_empty() || model.is_empty() {
        return Err("provider_key and model are required".to_string());
    }
    let mut agent = state.agent.lock().map_err(|e| e.to_string())?;
    if agent.ai_config.find_by_key(&provider_key).is_none() {
        return Err(format!(
            "No configured provider matches key \"{}\". Pick a model from a \
             provider that's already connected.",
            provider_key
        ));
    }
    agent.ai_config.subagent = Some(rustic_agent::SubagentConfig {
        provider_key,
        model,
    });
    persist_ai_config(&agent.ai_config, &state)?;
    Ok(())
}

/// Remove the configured cheaper sub-agent model. After this, the main
/// agent's `spawn_subagent` schema drops the `model_tier` parameter and
/// every spawn falls back to the main chat model.
#[tauri::command]
pub fn clear_subagent_config(state: State<'_, AppState>) -> Result<(), String> {
    let mut agent = state.agent.lock().map_err(|e| e.to_string())?;
    agent.ai_config.subagent = None;
    persist_ai_config(&agent.ai_config, &state)?;
    Ok(())
}

/// P0.4: persist the user's budget settings (concurrent-stream cap +
/// daily-cost-ceiling in cents). `None` on either field disables that
/// gate. Takes effect on the NEXT `send_message` — the running Budget
/// is constructed at ToolContext build time and doesn't hot-reload mid-
/// turn (matches how plan-mode and other ai-config settings behave).
#[tauri::command]
pub fn set_budget_settings(
    state: State<'_, AppState>,
    max_concurrent_streams: Option<usize>,
    daily_cost_ceiling_cents: Option<u64>,
) -> Result<(), String> {
    let mut agent = state.agent.lock().map_err(|e| e.to_string())?;
    // Preserve fields the Budget panel doesn't own — the sub-agent cap
    // lives in the Sub Agent settings panel, so a save from the Budget
    // panel must not clobber it.
    let preserved_subagents = agent.ai_config.budget.max_concurrent_subagents;
    agent.ai_config.budget = rustic_agent::budget::BudgetSettings {
        max_concurrent_streams,
        daily_cost_ceiling_cents,
        max_concurrent_subagents: preserved_subagents,
    };
    persist_ai_config(&agent.ai_config, &state)?;
    Ok(())
}

/// Set just the sub-agent concurrency cap. Lives in the Sub Agent settings
/// panel, separate from the Budget panel — the user's mental model is that
/// "how many sub-agents can fan out per task" is a sub-agent concern, not
/// a cross-task budget. Persisted in `BudgetSettings.max_concurrent_subagents`
/// for storage convenience (same struct gets serialised in one place).
/// `None` disables the gate (uncapped).
#[tauri::command]
pub fn set_subagent_concurrency_cap(
    state: State<'_, AppState>,
    cap: Option<usize>,
) -> Result<(), String> {
    let mut agent = state.agent.lock().map_err(|e| e.to_string())?;
    agent.ai_config.budget.max_concurrent_subagents = cap;
    persist_ai_config(&agent.ai_config, &state)?;
    Ok(())
}

/// Read the sub-agent concurrency cap. `None` → uncapped. The Sub Agent
/// settings panel calls this on mount to populate the toggle + input.
#[tauri::command]
pub fn get_subagent_concurrency_cap(
    state: State<'_, AppState>,
) -> Result<Option<usize>, String> {
    let agent = state.agent.lock().map_err(|e| e.to_string())?;
    Ok(agent.ai_config.budget.max_concurrent_subagents)
}

/// P0.4: read the current budget settings. UI uses this to populate the
/// form fields on settings open. Mirrors `get_ai_config` but only returns
/// the budget slice so the frontend doesn't have to parse a giant blob.
#[tauri::command]
pub fn get_budget_settings(
    state: State<'_, AppState>,
) -> Result<rustic_agent::budget::BudgetSettings, String> {
    let agent = state.agent.lock().map_err(|e| e.to_string())?;
    Ok(agent.ai_config.budget.clone())
}

/// Persist `ai_config` to SQLite with API keys redacted (the keychain owns
/// the real secrets). Used by `set_subagent_config` / `clear_subagent_config`
/// so they don't have to duplicate the redact-and-write dance.
fn persist_ai_config(cfg: &AiConfig, state: &State<'_, AppState>) -> Result<(), String> {
    let mut redacted = cfg.clone();
    for entry in redacted.providers.iter_mut() {
        entry.api_key.clear();
    }
    let json = serde_json::to_string(&redacted).map_err(|e| e.to_string())?;
    let db = state.db.lock().map_err(|e| e.to_string())?;
    db.set_setting("ai_config", &json).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn get_ai_config(state: State<'_, AppState>) -> Result<AiConfig, String> {
    // Return a redacted copy: api_key fields are replaced with a marker so
    // the webview can render "Configured / Not configured" without seeing the
    // raw secret. The agent loop reads keys directly from `state.agent`.
    let agent = state.agent.lock().unwrap();
    let mut config = agent.ai_config.clone();
    for entry in config.providers.iter_mut() {
        if !entry.api_key.is_empty() {
            entry.api_key = "__STORED__".to_string();
        }
    }
    Ok(config)
}

#[tauri::command]
pub fn get_tool_config(state: State<'_, AppState>) -> Result<ToolConfig, String> {
    let agent = state.agent.lock().unwrap();
    Ok(agent.tool_config.clone())
}

#[tauri::command]
pub fn set_tool_config(
    state: State<'_, AppState>,
    config: ToolConfig,
) -> Result<(), String> {
    let mut agent = state.agent.lock().unwrap();
    agent.tool_config = config;
    let json = serde_json::to_string(&agent.tool_config).map_err(|e| e.to_string())?;
    drop(agent);
    let db = state.db.lock().unwrap();
    db.set_setting("tool_config", &json).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn remove_ai_provider(
    state: State<'_, AppState>,
    provider_key: String,
) -> Result<(), String> {
    let mut agent = state.agent.lock().unwrap();

    // Snapshot the matching entry's (provider_type, name) before removal so
    // we can wipe its keychain secret too — otherwise the keychain
    // accumulates orphaned API keys for providers the user has deleted from
    // the UI.
    let removed_account: Option<String> = agent
        .ai_config
        .providers
        .iter()
        .find(|p| p.provider_key() == provider_key)
        .map(|entry| {
            let provider_str = entry.provider_type.as_str();
            crate::secrets::provider_account(provider_str, entry.name.as_deref())
        });

    let before = agent.ai_config.providers.len();
    agent
        .ai_config
        .providers
        .retain(|p| p.provider_key() != provider_key);

    if agent.ai_config.providers.len() == before {
        return Err(format!("Provider not found: {}", provider_key));
    }

    // Best-effort keychain cleanup. Failure to delete is logged but not
    // surfaced — the provider is already gone from the user's POV; a stale
    // keychain entry just wastes a slot and is harmless.
    if let Some(acct) = removed_account {
        if let Err(e) = crate::secrets::delete(&acct) {
            tracing::warn!(account = %acct, error = %e, "keychain cleanup failed");
        }
    }

    // If the deleted entry was the default, pick a new default if anything remains
    if let Some(ref def) = agent.ai_config.default_provider {
        let still_present = agent
            .ai_config
            .providers
            .iter()
            .any(|p| &p.provider_type == def);
        if !still_present {
            agent.ai_config.default_provider = agent
                .ai_config
                .providers
                .first()
                .map(|p| p.provider_type.clone());
        }
    }

    let config_json = serde_json::to_string(&agent.ai_config).map_err(|e| e.to_string())?;
    drop(agent);
    let db = state.db.lock().unwrap();
    db.set_setting("ai_config", &config_json).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn set_permissions(
    state: State<'_, AppState>,
    project_id: Option<String>,
    level: String,
) -> Result<(), String> {
    let perm = parse_permission_level(&level)?;
    let mut agent = state.agent.lock().unwrap();
    if let Some(pid) = project_id {
        agent.project_permissions.insert(pid, perm);
    }
    Ok(())
}

#[tauri::command]
pub fn set_task_permissions(
    state: State<'_, AppState>,
    task_id: String,
    level: String,
) -> Result<(), String> {
    let perm = parse_permission_level(&level)?;
    let mut agent = state.agent.lock().unwrap();
    let task = agent
        .tasks
        .get_mut(&task_id)
        .ok_or_else(|| format!("Task not found: {}", task_id))?;
    task.permissions = perm.clone();
    // Update shared permissions so the running executor sees the change immediately
    if let Some(ref shared) = task.shared_permissions {
        shared.set_level(perm);
    }
    Ok(())
}

fn parse_permission_level(level: &str) -> Result<PermissionLevel, String> {
    match level {
        "Chat" => Ok(PermissionLevel::Chat),
        "ManualEdit" => Ok(PermissionLevel::ManualEdit),
        "AutoEdit" => Ok(PermissionLevel::AutoEdit),
        "FullAuto" => Ok(PermissionLevel::FullAuto),
        _ => Err(format!("Unknown permission level: {}. Valid values: Chat, ManualEdit, AutoEdit, FullAuto", level)),
    }
}

// Model-list fetching extracted to ./models.rs

// MCP commands extracted to ./mcp.rs
// Runtime commands extracted to ./runtime.rs
// Memory commands (get_memory/clear_memory) extracted to ./memory.rs
// Project defaults commands extracted to ./project_defaults.rs

/// Build the MCP tools section to append to the system prompt.
/// Verbosity is scaled by total tool count to keep context usage reasonable.
fn build_mcp_system_section(tools: &[ToolDef]) -> String {
    if tools.is_empty() {
        return String::new();
    }
    let mut section = String::from("\n\n## MCP tools\n");
    if tools.len() < 20 {
        // Full names + descriptions
        for t in tools {
            section.push_str(&format!("- **{}**: {}\n", t.name, t.description));
        }
    } else if tools.len() <= 100 {
        // Names only
        section.push_str("Available tools: ");
        let names: Vec<_> = tools.iter().map(|t| t.name.as_str()).collect();
        section.push_str(&names.join(", "));
        section.push('\n');
    } else {
        // Just report the count
        section.push_str(&format!(
            "{} MCP tools available from external servers.\n",
            tools.len()
        ));
    }
    section
}


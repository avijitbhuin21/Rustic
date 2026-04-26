use crate::state::{AgentTask, AppState};
use rustic_agent::{
    AiConfig, AiProvider, ContentBlock, McpConnectResult, McpScope, McpServerWithStatus, Message,
    PermissionLevel, ProviderConfig, ProviderType, Role, SharedPermissions, TaskCost, TaskDiff,
    TodoItem, TaskEvent, TaskExecutor, TaskInfo, TaskStatus, ToolConfig, ToolContext, ToolDef,
    checkpoint_ops, build_skills_system_section, discover_skills,
    build_workflows_system_section, discover_workflows,
    build_user_rules_system_section,
};
use rustic_agent::tools::{ComputeDiffFn, SnapshotFn};
use rustic_db::{MessageRow, SubagentRecord, TaskRow};
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
struct AgentStreamEvent {
    task_id: String,
    text: String,
}

#[derive(Clone, Serialize)]
struct AgentToolUseEvent {
    task_id: String,
    tool_use_id: String,
    tool_name: String,
    tool_input: serde_json::Value,
}

#[derive(Clone, Serialize)]
struct AgentToolResultEvent {
    task_id: String,
    tool_use_id: String,
    output: String,
    is_error: bool,
}

#[derive(Clone, Serialize)]
struct AgentToolProgressEvent {
    task_id: String,
    tool_use_id: String,
    progress_text: String,
}

#[derive(Clone, Serialize)]
struct AgentStatusEvent {
    task_id: String,
    status: TaskStatus,
}

#[derive(Clone, Serialize)]
struct AgentTaskCompleteEvent {
    task_id: String,
    diff: TaskDiff,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<String>,
}

#[derive(Clone, Serialize)]
struct AgentPermissionRequestEvent {
    task_id: String,
    request_id: String,
    operation: String,
    description: String,
    preview: Option<String>,
}

#[derive(Clone, Serialize)]
struct AgentCostUpdateEvent {
    task_id: String,
    cost: TaskCost,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentRequestUsageEvent {
    task_id: String,
    input_tokens: u32,
    output_tokens: u32,
    cache_read_tokens: u32,
    cache_write_tokens: u32,
    cost_usd: f64,
}

#[derive(Clone, Serialize)]
struct AgentMemoryUpdatedEvent {
    task_id: String,
}

#[derive(Clone, Serialize)]
struct AgentModelSwitchedEvent {
    task_id: String,
    from_model: String,
    to_model: String,
    provider_type: String,
}

#[derive(Clone, Serialize)]
struct AgentSubagentSpawnedEvent {
    task_id: String,
    agent_id: String,
    model: String,
    prompt: String,
}

#[derive(Clone, Serialize)]
struct AgentSubagentCompletedEvent {
    task_id: String,
    agent_id: String,
    summary: String,
}

#[derive(Clone, Serialize)]
struct AgentSubagentFailedEvent {
    task_id: String,
    agent_id: String,
    error: String,
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
struct AgentThinkingDeltaEvent {
    task_id: String,
    text: String,
}

#[derive(Clone, Serialize)]
struct AgentThinkingDoneEvent {
    task_id: String,
    duration_secs: u64,
}

#[derive(Clone, Serialize)]
struct AgentQuestionRequestEvent {
    task_id: String,
    request_id: String,
    question: String,
}

#[derive(Clone, Serialize)]
struct AgentTodoUpdatedEvent {
    task_id: String,
    todos: Vec<TodoItem>,
}

#[derive(Clone, Serialize)]
struct AgentTitleChangedEvent {
    task_id: String,
    title: String,
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
    let (mut messages, project_root, _permissions, _sensitive_files_allowed, shared_perms, provider_config, provider_type_str, checkpoint_id, cancel_token, permission_broker, question_broker, mcp_manager_arc, ai_config, tool_config, allowed_paths, task_project_id) = {
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
            let raw: String = message.chars().take(70).collect();
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

        // Create or reuse shared permissions — the executor reads from this Arc in real-time.
        let shared_perms = task.shared_permissions.get_or_insert_with(|| {
            SharedPermissions::new(task_permissions.clone(), task_sensitive_files_allowed)
        }).clone();
        // Sync current values into shared permissions (user may have changed mode between messages)
        shared_perms.set_level(task_permissions.clone());
        shared_perms.set_sensitive_files_allowed(task_sensitive_files_allowed);

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
            }).map_err(|e| format!("Failed to persist task: {}", e))?;
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

        // Add user message (text + optional images)
        let mut user_content = vec![ContentBlock::Text { text: message }];
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

        let message_index = task.messages.len() as i64 - 1;
        let task_messages = task.messages.clone();

        // Create or refresh cancellation token for this run
        let cancel_token = Arc::new(AtomicBool::new(false));
        agent.cancellation_tokens.insert(task_id.clone(), Arc::clone(&cancel_token));

        // Create checkpoint for this user message. This both records the DB
        // row AND copies the project directory (minus heavy build dirs) into
        // the app's snapshot_root so a later revert can restore it wholesale.
        let db = state.db.lock().unwrap();
        let checkpoint_id = checkpoint_ops::create_checkpoint(
            &db,
            &task_id,
            message_index,
            &project_root,
            &state.snapshot_root,
        ).map_err(|e| e.to_string())?;

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
        let include_gitignored = matches!(task_permissions, PermissionLevel::FullAuto);
        let is_global_scope = rustic_agent::is_global_project_id(&task_project_id);
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
        let question_broker = Arc::clone(&agent.question_broker);
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
            shared_perms,
            config,
            task_provider_type,
            checkpoint_id,
            cancel_token,
            broker,
            question_broker,
            mcp_arc,
            ai_config,
            tool_config_arc,
            allowed_paths,
            task_project_id,
        )
    };

    let provider: Arc<dyn AiProvider> = if provider_type_str == "Claude" {
        Arc::new(ClaudeProvider::new())
    } else if provider_type_str == "OpenAi" {
        Arc::new(OpenAiProvider::new())
    } else if provider_type_str == "Gemini" {
        Arc::new(GeminiProvider::new())
    } else if provider_type_str == "Compatible" || provider_type_str.starts_with("Compatible:") {
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

            let parent_provider_config = Arc::new(provider_config.clone());
            let executor = TaskExecutor::new(provider, provider_config);

            // Build snapshot closure that captures DB and checkpoint ID
            let db_for_snapshot = Arc::clone(&db_arc);
            let cp_id = checkpoint_id.clone();
            let snapshot_fn: SnapshotFn = Arc::new(move |path: &std::path::Path| {
                if let Ok(db) = db_for_snapshot.lock() {
                    let _ = checkpoint_ops::snapshot_file(&db, &cp_id, path);
                }
            });

            // Build compute_diff closure that captures DB and task ID
            let db_for_diff = Arc::clone(&db_arc);
            let diff_task_id = task_id_clone.clone();
            let compute_diff_fn: ComputeDiffFn = Arc::new(move || {
                if let Ok(db) = db_for_diff.lock() {
                    checkpoint_ops::compute_task_diff(&db, &diff_task_id)
                        .unwrap_or_else(|_| TaskDiff {
                            files: Vec::new(),
                            total_insertions: 0,
                            total_deletions: 0,
                        })
                } else {
                    TaskDiff {
                        files: Vec::new(),
                        total_insertions: 0,
                        total_deletions: 0,
                    }
                }
            });

            // Create event channel before ToolContext so event_tx can be stored in context
            let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel::<TaskEvent>();

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

            let context = ToolContext {
                project_root: PathBuf::from(&project_root),
                shared_permissions: shared_perms.clone(),
                snapshot_fn: Some(snapshot_fn),
                compute_diff_fn: Some(compute_diff_fn),
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
                question_broker: Arc::clone(&question_broker),
                parent_provider_config: Some(parent_provider_config),
                completion_summary: Arc::new(std::sync::Mutex::new(None)),
                write_scope: None, // main agent: unrestricted
                blocked_writes: Arc::new(std::sync::Mutex::new(Vec::new())),
                agent_terminals: Some(Arc::new(crate::commands::agent_terminals::TauriAgentTerminals::new(app_clone.clone())) as Arc<dyn rustic_agent::AgentTerminals>),
                is_global,
                orchestrator_host,
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
                        TaskEvent::TaskComplete { task_id, diff, summary } => {
                            let _ = app_events.emit("agent-task-complete", AgentTaskCompleteEvent {
                                task_id,
                                diff,
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
                        TaskEvent::SubagentSpawned { task_id, agent_id, model, prompt } => {
                            eprintln!("[tauri] subagent spawned: task={} agent={} model={}", task_id, agent_id, model);
                            // Persist the spawn so the card survives reload even
                            // if the sub-agent never completes (crash, cancel).
                            if let Ok(db) = cost_db.lock() {
                                let _ = db.upsert_subagent_spawn(&task_id, &agent_id, &model, &prompt);
                            }
                            let _ = app_events.emit("agent-subagent-spawned", AgentSubagentSpawnedEvent { task_id, agent_id, model, prompt });
                        }
                        TaskEvent::SubagentCompleted { task_id, agent_id, summary } => {
                            eprintln!("[tauri] subagent completed: task={} agent={} summary_len={}", task_id, agent_id, summary.len());
                            if let Ok(db) = cost_db.lock() {
                                let _ = db.update_subagent_summary(&task_id, &agent_id, &summary);
                            }
                            let _ = app_events.emit("agent-subagent-completed", AgentSubagentCompletedEvent { task_id, agent_id, summary });
                        }
                        TaskEvent::SubagentFailed { task_id, agent_id, error } => {
                            eprintln!("[tauri] subagent failed: task={} agent={} error={}", task_id, agent_id, error);
                            if let Ok(db) = cost_db.lock() {
                                let _ = db.update_subagent_error(&task_id, &agent_id, &error);
                            }
                            let _ = app_events.emit("agent-subagent-failed", AgentSubagentFailedEvent { task_id, agent_id, error });
                        }
                        TaskEvent::SubagentTextDelta { task_id, agent_id, text } => {
                            let _ = app_events.emit("agent-subagent-text-delta", AgentSubagentTextDeltaEvent { task_id, agent_id, text });
                        }
                        TaskEvent::SubagentCostUpdate { task_id, agent_id, cost } => {
                            eprintln!("[tauri] subagent cost update: task={} agent={} in={} out={} usd={:.4}",
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
                        TaskEvent::UserQuestionRequest { task_id, request_id, question } => {
                            let _ = app_events.emit("agent-question-request", AgentQuestionRequestEvent { task_id, request_id, question });
                        }
                        TaskEvent::TodoUpdated { task_id, todos } => {
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

            let (result, turn_cost) = match executor.run_turn(&mut messages, &context).await {
                Ok(cost) => (Ok(()), cost),
                Err(e) => (Err(e), TaskCost::default()),
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

            // Persist all messages to DB after the turn completes
            if let Ok(db) = db_arc.lock() {
                let _ = db.delete_messages_for_task(&task_id_clone);
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
                    if let Ok(content_json) = serde_json::to_string(&msg.content) {
                        let _ = db.insert_message(&MessageRow {
                            id: format!("{}-{}", task_id_clone, i),
                            task_id: task_id_clone.clone(),
                            role: role.to_string(),
                            content_json,
                            created_at: chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string(),
                            sort_order: i as i64,
                            turn_usage_json: msg_turn_usage,
                        });
                    }
                }
            }

            // Sync in-memory task messages with the executor's complete history.
            // Without this, the in-memory cache is stale (missing assistant responses,
            // tool calls, thinking blocks) and get_task_messages would return incomplete data.
            if let Ok(mut agent) = agent_arc.lock() {
                if let Some(task) = agent.tasks.get_mut(&task_id_clone) {
                    task.messages = messages.clone();
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
            // The frontend guards against duplicate task_complete messages.
            if final_status == TaskStatus::Completed {
                let diff = context
                    .compute_diff_fn
                    .as_ref()
                    .map(|f| f())
                    .unwrap_or_else(|| TaskDiff {
                        files: Vec::new(),
                        total_insertions: 0,
                        total_deletions: 0,
                    });
                let summary = context
                    .completion_summary
                    .lock()
                    .ok()
                    .and_then(|s| s.clone());
                let _ = app_clone.emit("agent-task-complete", AgentTaskCompleteEvent {
                    task_id: task_id_clone.clone(),
                    diff,
                    summary,
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
        if row.status == "Running" {
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

#[tauri::command]
pub fn get_task_messages(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<Vec<MessageDto>, String> {
    // If task is in memory and has messages, return those (turn_usage not included
    // here — the frontend already has it from the live session's RequestUsage events)
    {
        let agent = state.agent.lock().unwrap();
        if let Some(task) = agent.tasks.get(&task_id) {
            if !task.messages.is_empty() {
                let dtos = task.messages.iter().map(|m| MessageDto {
                    role: match m.role {
                        Role::User => "user".to_string(),
                        Role::Assistant => "assistant".to_string(),
                        Role::System => "system".to_string(),
                    },
                    content: m.content.clone(),
                    turn_usage: None,
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
                eprintln!("[get_task_messages] Failed to deserialize content_json for message {}: {}. Falling back to raw text.", row.id, e);
                vec![ContentBlock::Text { text: row.content_json.clone() }]
            });
        let turn_usage: Option<TurnUsage> = row.turn_usage_json.as_deref()
            .and_then(|j| serde_json::from_str(j).ok());
        dtos.push(MessageDto {
            role: row.role.clone(),
            content: content.clone(),
            turn_usage,
        });
        messages_for_cache.push(Message { role, content });
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

#[tauri::command]
pub fn delete_task(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<(), String> {
    let mut agent = state.agent.lock().unwrap();
    agent.tasks.remove(&task_id);
    drop(agent);
    let db = state.db.lock().unwrap();
    let _ = db.delete_messages_for_task(&task_id);
    let _ = db.delete_task(&task_id);
    drop(db);
    // Blow away this task's checkpoint snapshots. They're keyed by task_id so
    // one remove_dir_all covers every checkpoint for this task.
    let task_snap_dir = state.snapshot_root.join(&task_id);
    let _ = std::fs::remove_dir_all(&task_snap_dir);
    Ok(())
}

#[tauri::command]
pub fn delete_tasks_for_project(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<(), String> {
    let mut agent = state.agent.lock().unwrap();
    agent.tasks.retain(|_, t| t.info.project_id != project_id);
    drop(agent);
    let db = state.db.lock().unwrap();
    // Collect task ids first so we can clean up their snapshots after the DB
    // cascade removes the rows.
    let task_ids: Vec<String> = db
        .list_tasks_for_project(&project_id)
        .unwrap_or_default()
        .into_iter()
        .map(|t| t.id)
        .collect();
    let _ = db.delete_tasks_for_project(&project_id);
    drop(db);
    for tid in task_ids {
        let _ = std::fs::remove_dir_all(state.snapshot_root.join(&tid));
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
        _ => return Err(format!("Unknown provider type: {}", provider_type)),
    };

    // Normalize the instance name (only meaningful for Compatible)
    let entry_name: Option<String> = if matches!(pt, ProviderType::Compatible) {
        name.as_ref()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    } else {
        None
    };

    // Upsert key: for Compatible with a name, key by (type + name); otherwise
    // behave as before (one entry per provider type).
    let matches_idx = agent.ai_config.providers.iter().position(|p| {
        if p.provider_type != pt {
            return false;
        }
        match (&pt, &entry_name, &p.name) {
            (ProviderType::Compatible, Some(n), Some(existing)) => existing == n,
            (ProviderType::Compatible, None, None) => true,
            (ProviderType::Compatible, _, _) => false,
            _ => true,
        }
    });

    if let Some(idx) = matches_idx {
        let entry = &mut agent.ai_config.providers[idx];
        entry.api_key = api_key;
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

    // Persist so the key survives restarts
    let config_json = serde_json::to_string(&agent.ai_config).map_err(|e| e.to_string())?;
    drop(agent);
    let db = state.db.lock().unwrap();
    db.set_setting("ai_config", &config_json).map_err(|e| e.to_string())?;

    Ok(())
}

#[tauri::command]
pub fn get_ai_config(state: State<'_, AppState>) -> Result<AiConfig, String> {
    let agent = state.agent.lock().unwrap();
    Ok(agent.ai_config.clone())
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
    let before = agent.ai_config.providers.len();
    agent
        .ai_config
        .providers
        .retain(|p| p.provider_key() != provider_key);

    if agent.ai_config.providers.len() == before {
        return Err(format!("Provider not found: {}", provider_key));
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

// === Model fetching ===
//
// Per-provider list endpoints. Results are cached in-memory for MODEL_CACHE_TTL
// to avoid hammering the provider every time the UI refreshes the dropdown.
// The selected model id is persisted by the frontend; the fetched list itself
// is treated as live data.

const MODEL_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(5 * 60);

type ModelCacheKey = (String, u64, String); // (provider, api_key hash, base_url)
type ModelCacheEntry = (Vec<String>, std::time::Instant);

static MODEL_CACHE: std::sync::OnceLock<
    tokio::sync::Mutex<std::collections::HashMap<ModelCacheKey, ModelCacheEntry>>,
> = std::sync::OnceLock::new();

fn model_cache() -> &'static tokio::sync::Mutex<std::collections::HashMap<ModelCacheKey, ModelCacheEntry>> {
    MODEL_CACHE.get_or_init(|| tokio::sync::Mutex::new(std::collections::HashMap::new()))
}

fn hash_key(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

/// Shared non-text model keywords. Anything containing one of these in its id
/// is not a chat model and should be hidden from the picker.
const NON_CHAT_KEYWORDS: &[&str] = &[
    "tts", "whisper", "dall-e", "embedding", "moderation",
    "speech", "audio", "image-gen", "transcri", "realtime",
];

#[tauri::command]
pub async fn fetch_ai_models(
    provider_type: String,
    api_key: String,
    base_url: Option<String>,
    force_refresh: Option<bool>,
) -> Result<Vec<String>, String> {
    let cache_key: ModelCacheKey = (
        provider_type.clone(),
        hash_key(&api_key),
        base_url.clone().unwrap_or_default(),
    );

    if !force_refresh.unwrap_or(false) {
        let cache = model_cache().lock().await;
        if let Some((models, fetched_at)) = cache.get(&cache_key) {
            if fetched_at.elapsed() < MODEL_CACHE_TTL {
                eprintln!(
                    "[fetch_ai_models] provider={} CACHE_HIT age={}s count={}",
                    provider_type,
                    fetched_at.elapsed().as_secs(),
                    models.len()
                );
                return Ok(models.clone());
            }
        }
    }
    eprintln!(
        "[fetch_ai_models] provider={} CACHE_MISS force={} base_url={:?}",
        provider_type,
        force_refresh.unwrap_or(false),
        base_url
    );

    let client = reqwest::Client::new();

    let models = match provider_type.as_str() {
        "Claude" => {
            let res = client
                .get("https://api.anthropic.com/v1/models")
                .header("x-api-key", &api_key)
                .header("anthropic-version", "2023-06-01")
                .query(&[("limit", "1000")])
                .send()
                .await
                .map_err(|e| e.to_string())?;
            if !res.status().is_success() {
                let status = res.status();
                let body = res.text().await.unwrap_or_default();
                eprintln!("[fetch_ai_models] Claude HTTP error status={} body={}", status, body);
                return Err(format!("HTTP {}: {}", status, body));
            }
            let data: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;

            // Dump every id the API returned so we can tell real filter bugs
            // from API-side omissions.
            let raw_ids: Vec<String> = data["data"]
                .as_array()
                .unwrap_or(&vec![])
                .iter()
                .filter_map(|m| m["id"].as_str().map(|s| s.to_string()))
                .collect();
            eprintln!(
                "[fetch_ai_models] Claude raw ids ({}): {:?}",
                raw_ids.len(),
                raw_ids
            );

            let mut models: Vec<String> = raw_ids
                .into_iter()
                .filter(|id| {
                    let is_claude = id.starts_with("claude-");
                    let has_tier = id.contains("haiku") || id.contains("sonnet") || id.contains("opus");
                    let keep = is_claude && has_tier;
                    if !keep {
                        eprintln!("[fetch_ai_models] Claude DROP id={}", id);
                    }
                    keep
                })
                .collect();
            models.sort_by(|a, b| b.cmp(a));
            eprintln!(
                "[fetch_ai_models] Claude kept ({}): {:?}",
                models.len(),
                models
            );
            models
        }
        "OpenAi" => {
            let res = client
                .get("https://api.openai.com/v1/models")
                .header("Authorization", format!("Bearer {}", api_key))
                .send()
                .await
                .map_err(|e| e.to_string())?;
            if !res.status().is_success() {
                return Err(format!("HTTP {}", res.status()));
            }
            let data: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;
            // Chat-capable families only: gpt-*, chatgpt-*, and reasoning o-series
            // (o1, o3, o4, o5...). Excludes embeddings, tts, whisper, etc.
            let mut models: Vec<String> = data["data"]
                .as_array()
                .unwrap_or(&vec![])
                .iter()
                .filter_map(|m| m["id"].as_str().map(|s| s.to_string()))
                .filter(|id| {
                    let id_lower = id.to_lowercase();
                    let is_chat_family = id_lower.starts_with("gpt-")
                        || id_lower.starts_with("chatgpt-")
                        || (id_lower.starts_with('o')
                            && id_lower.chars().nth(1).map_or(false, |c| c.is_ascii_digit()));
                    is_chat_family
                        && !NON_CHAT_KEYWORDS.iter().any(|kw| id_lower.contains(kw))
                        // "search" models are retrieval helpers, not chat
                        && !id_lower.contains("search")
                })
                .collect();
            models.sort_by(|a, b| b.cmp(a));
            models
        }
        "Gemini" => {
            let res = client
                .get("https://generativelanguage.googleapis.com/v1beta/models")
                .query(&[("key", api_key.as_str()), ("pageSize", "200")])
                .send()
                .await
                .map_err(|e| e.to_string())?;
            if !res.status().is_success() {
                return Err(format!("HTTP {}", res.status()));
            }
            let data: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;
            let generate_content = serde_json::json!("generateContent");
            // Any model that supports generateContent and isn't an embedding
            // surface. Includes both dated variants and -latest aliases.
            let mut models: Vec<String> = data["models"]
                .as_array()
                .unwrap_or(&vec![])
                .iter()
                .filter(|m| {
                    m["supportedGenerationMethods"]
                        .as_array()
                        .map(|methods| methods.contains(&generate_content))
                        .unwrap_or(false)
                })
                .filter_map(|m| m["name"].as_str().map(|s| s.replace("models/", "")))
                .filter(|id| {
                    let id_lower = id.to_lowercase();
                    !NON_CHAT_KEYWORDS.iter().any(|kw| id_lower.contains(kw))
                        // Gecko / aqa / text-bison-style legacy embeddings
                        && !id_lower.contains("gecko")
                        && !id_lower.contains("aqa")
                })
                .collect();
            models.sort_by(|a, b| b.cmp(a));
            models
        }
        "Compatible" => {
            let raw = base_url.clone().unwrap_or_default();
            let base = raw
                .trim_end_matches('/')
                .trim_end_matches("/chat/completions")
                .trim_end_matches("/completions")
                .trim_end_matches('/')
                .to_string();
            let url = format!("{}/models", base);
            let res = client
                .get(&url)
                .header("Authorization", format!("Bearer {}", api_key))
                .send()
                .await
                .map_err(|e| format!("Request to {} failed: {}", url, e))?;
            if !res.status().is_success() {
                let status = res.status();
                let body = res.text().await.unwrap_or_default();
                return Err(format!("HTTP {} from {}: {}", status, url, body));
            }
            let data: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;
            let mut models: Vec<String> = data["data"]
                .as_array()
                .or_else(|| data["models"].as_array())
                .unwrap_or(&vec![])
                .iter()
                .filter_map(|m| {
                    m["id"]
                        .as_str()
                        .or_else(|| m["name"].as_str())
                        .map(|s| s.to_string())
                })
                .filter(|id| {
                    let id_lower = id.to_lowercase();
                    !NON_CHAT_KEYWORDS.iter().any(|kw| id_lower.contains(kw))
                })
                .collect();
            models.sort_by(|a, b| b.cmp(a));
            models
        }
        _ => return Err(format!("Unknown provider type: {}", provider_type)),
    };

    // Cache the fresh list.
    {
        let mut cache = model_cache().lock().await;
        cache.insert(cache_key, (models.clone(), std::time::Instant::now()));
    }

    Ok(models)
}

// === MCP commands ===
//
// Config model (matches Claude Code): servers live in JSON files, not SQLite.
//   - User scope:    <app_data_dir>/mcp.json       — shared across projects
//   - Project scope: <project_root>/.mcp.json      — committed to source
//
// The frontend edits the raw JSON via a modal. On save, the backend validates,
// writes atomically, reloads the scope into McpManager, and tries to connect
// each server so the UI can show per-server success/failure inline.

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpSaveResult {
    pub name: String,
    pub connected: bool,
    pub tool_count: usize,
    pub error: Option<String>,
}

impl From<McpConnectResult> for McpSaveResult {
    fn from(r: McpConnectResult) -> Self {
        Self {
            name: r.name,
            connected: r.connected,
            tool_count: r.tool_count,
            error: r.error,
        }
    }
}

fn parse_scope(s: &str) -> Result<McpScope, String> {
    match s {
        "user" => Ok(McpScope::User),
        "project" => Ok(McpScope::Project),
        other => Err(format!("Unknown MCP scope: {}. Valid values: user, project", other)),
    }
}

/// Resolve the on-disk path for a given scope.
/// User scope uses the Tauri app data dir; project scope needs `project_id`.
fn resolve_scope_path(
    app: &AppHandle,
    state: &State<'_, AppState>,
    scope: McpScope,
    project_id: Option<&str>,
) -> Result<PathBuf, String> {
    match scope {
        McpScope::User => {
            let dir = tauri::Manager::path(app)
                .app_data_dir()
                .map_err(|e| format!("Failed to resolve app data dir: {}", e))?;
            Ok(dir.join("mcp.json"))
        }
        McpScope::Project => {
            let pid = project_id
                .ok_or_else(|| "project_id is required for project scope".to_string())?;
            let workspace = state.workspace.lock().unwrap();
            let project = workspace
                .list_projects()
                .into_iter()
                .find(|p| p.id.to_string() == pid)
                .ok_or_else(|| format!("Project not found: {}", pid))?;
            Ok(project.root_path.join(".mcp.json"))
        }
    }
}

/// Return the current JSON text for a scope. If the file does not exist yet,
/// returns a blank template so the user has something to edit.
#[tauri::command]
pub fn read_mcp_json(
    app: AppHandle,
    state: State<'_, AppState>,
    scope: String,
    project_id: Option<String>,
) -> Result<String, String> {
    let scope = parse_scope(&scope)?;
    let path = resolve_scope_path(&app, &state, scope, project_id.as_deref())?;
    if path.exists() {
        std::fs::read_to_string(&path).map_err(|e| e.to_string())
    } else {
        Ok("{\n  \"mcpServers\": {}\n}\n".to_string())
    }
}

/// Validate + write raw JSON content for a scope, reload it into the manager,
/// and try to connect each server. Returns per-server `{name, connected, error}`.
#[tauri::command]
pub fn save_mcp_json(
    app: AppHandle,
    state: State<'_, AppState>,
    scope: String,
    project_id: Option<String>,
    content: String,
) -> Result<Vec<McpSaveResult>, String> {
    let scope = parse_scope(&scope)?;
    let path = resolve_scope_path(&app, &state, scope, project_id.as_deref())?;

    let mcp_arc = Arc::clone(&state.agent.lock().unwrap().mcp_manager);
    let mut mcp = mcp_arc.lock().unwrap();

    // Make sure the manager knows where to write.
    match scope {
        McpScope::User => mcp.set_user_path(path.clone()),
        McpScope::Project => mcp.set_project_path(path.clone()),
    }

    mcp.save_scope_raw(scope, &content)
        .map_err(|e| e.to_string())?;

    Ok(mcp
        .test_scope(scope)
        .into_iter()
        .map(McpSaveResult::from)
        .collect())
}

#[tauri::command]
pub fn list_mcp_servers(
    app: AppHandle,
    state: State<'_, AppState>,
    project_id: Option<String>,
) -> Result<Vec<McpServerWithStatus>, String> {
    let user_path = resolve_scope_path(&app, &state, McpScope::User, None).ok();
    let project_path = project_id
        .as_deref()
        .and_then(|pid| resolve_scope_path(&app, &state, McpScope::Project, Some(pid)).ok());

    let mcp_arc = Arc::clone(&state.agent.lock().unwrap().mcp_manager);
    let mut mcp = mcp_arc.lock().unwrap();

    if let Some(p) = user_path {
        let _ = mcp.load_scope(McpScope::User, &p);
    }
    if let Some(p) = project_path {
        let _ = mcp.load_scope(McpScope::Project, &p);
    }

    // Populate status for any server that hasn't been tested yet. Already-connected
    // servers are skipped, so repeated panel opens are fast.
    let _ = mcp.connect_all();

    Ok(mcp.list_servers_with_status())
}

#[tauri::command]
pub fn test_mcp_server(state: State<'_, AppState>, id: String) -> Result<Vec<ToolDef>, String> {
    let mcp_arc = Arc::clone(&state.agent.lock().unwrap().mcp_manager);
    let result = mcp_arc.lock().unwrap().test_server(&id).map_err(|e| e.to_string());
    result
}

#[tauri::command]
pub fn remove_mcp_server(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let mcp_arc = Arc::clone(&state.agent.lock().unwrap().mcp_manager);
    let result = mcp_arc.lock().unwrap().remove_server(&id).map_err(|e| e.to_string());
    result
}

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

#[tauri::command]
pub fn get_memory(state: State<'_, AppState>, project_id: String) -> Result<String, String> {
    let workspace = state.workspace.lock().unwrap();
    let project = workspace
        .list_projects()
        .into_iter()
        .find(|p| p.id.to_string() == project_id)
        .ok_or_else(|| "Project not found".to_string())?;
    let memory_path = project.root_path.join(".rustic/memory.md");
    drop(workspace);
    // Create the file (and parent dir) if it doesn't exist yet
    if !memory_path.exists() {
        if let Some(parent) = memory_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&memory_path, "");
    }
    Ok(std::fs::read_to_string(&memory_path).unwrap_or_default())
}

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

#[tauri::command]
pub fn clear_memory(state: State<'_, AppState>, project_id: String) -> Result<(), String> {
    let workspace = state.workspace.lock().unwrap();
    let project = workspace
        .list_projects()
        .into_iter()
        .find(|p| p.id.to_string() == project_id)
        .ok_or_else(|| "Project not found".to_string())?;
    let memory_path = project.root_path.join(".rustic/memory.md");
    drop(workspace);
    if memory_path.exists() {
        std::fs::write(&memory_path, "").map_err(|e| e.to_string())
    } else {
        Ok(())
    }
}

// ── Project defaults ────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProjectDefaults {
    pub model: Option<String>,
    pub provider_type: Option<String>,
    pub permission_level: Option<String>,
    pub thinking_effort: Option<String>,
}

#[tauri::command]
pub fn get_project_defaults(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<ProjectDefaults, String> {
    let db = state.db.lock().unwrap();
    if let Ok(Some(project)) = db.get_project(&project_id) {
        if let Some(json) = &project.settings_json {
            if let Ok(defaults) = serde_json::from_str::<ProjectDefaults>(json) {
                return Ok(defaults);
            }
        }
    }
    Ok(ProjectDefaults::default())
}

#[tauri::command]
pub fn save_project_defaults(
    state: State<'_, AppState>,
    project_id: String,
    defaults: ProjectDefaults,
) -> Result<(), String> {
    let json = serde_json::to_string(&defaults).map_err(|e| e.to_string())?;
    let db = state.db.lock().unwrap();
    db.update_project_settings(&project_id, Some(&json))
        .map_err(|e| e.to_string())
}

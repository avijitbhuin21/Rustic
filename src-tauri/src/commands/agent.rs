use crate::state::{AgentTask, AppState};
use rustic_agent::{
    AiConfig, AiProvider, ContentBlock, McpSource, McpTransport, Message, PermissionLevel,
    ProviderConfig, ProviderType, Role, ServerConfig, SharedPermissions, TaskCost, TaskDiff,
    TodoItem, TaskEvent, TaskExecutor, TaskInfo, TaskStatus, ToolContext, ToolDef, TurnBudget,
    checkpoint_ops, build_skills_system_section, discover_skills,
};
use rustic_agent::tools::{ComputeDiffFn, SnapshotFn};
use rustic_db::{MessageRow, TaskRow};
use std::sync::atomic::{AtomicBool, Ordering};
use rustic_agent::provider::claude::ClaudeProvider;
use rustic_agent::provider::openai::OpenAiProvider;
use rustic_agent::provider::compatible::CompatibleProvider;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};

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
    summary: String,
    notes: Option<String>,
    diff: TaskDiff,
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
struct AgentTurnBudgetWarningEvent {
    task_id: String,
    turns_remaining: u32,
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

#[tauri::command]
pub fn create_task(
    state: State<'_, AppState>,
    project_id: String,
    _project_name: String,
    _project_root: String,
    title: String,
) -> Result<TaskInfo, String> {
    let mut agent = state.agent.lock().unwrap();

    let task_id = uuid::Uuid::new_v4().to_string();
    let provider_type = agent
        .ai_config
        .default_provider
        .clone()
        .unwrap_or(ProviderType::Claude);
    let model = agent
        .ai_config
        .providers
        .iter()
        .find(|p| p.provider_type == provider_type)
        .map(|p| p.default_model.clone())
        .unwrap_or_else(|| "claude-sonnet-4-20250514".to_string());

    let info = TaskInfo {
        id: task_id.clone(),
        project_id: project_id.clone(),
        title,
        status: TaskStatus::Completed, // idle until a message is sent
        provider_type: format!("{:?}", provider_type),
        model,
    };

    let permissions = agent
        .project_permissions
        .get(&project_id)
        .cloned()
        .unwrap_or_default();

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

#[tauri::command]
pub fn send_message(
    app: AppHandle,
    state: State<'_, AppState>,
    task_id: String,
    message: String,
    thinking_budget: Option<u32>,
) -> Result<(), String> {
    let (mut messages, project_root, _permissions, _sensitive_files_allowed, shared_perms, provider_config, provider_type_str, checkpoint_id, cancel_token, permission_broker, question_broker, turn_budget_max, mcp_manager_arc, ai_config, allowed_paths) = {
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

        // Add user message
        task.messages.push(Message {
            role: Role::User,
            content: vec![ContentBlock::Text { text: message }],
        });
        task.info.status = TaskStatus::Running;

        let message_index = task.messages.len() as i64 - 1;
        let task_messages = task.messages.clone();

        // Create or refresh cancellation token for this run
        let cancel_token = Arc::new(AtomicBool::new(false));
        agent.cancellation_tokens.insert(task_id.clone(), Arc::clone(&cancel_token));

        // Create checkpoint for this user message
        let db = state.db.lock().unwrap();
        let checkpoint_id = checkpoint_ops::create_checkpoint(&db, &task_id, message_index)
            .map_err(|e| e.to_string())?;

        // Build provider config
        let provider_entry = agent
            .ai_config
            .providers
            .iter()
            .find(|p| format!("{:?}", p.provider_type) == task_provider_type)
            .cloned();

        let system_prompt = rustic_agent::build_system_prompt(&agent.ai_config.providers, &project_root);

        // Discover skills and append to system prompt
        let skills = discover_skills(&project_root);
        let skills_section = build_skills_system_section(&skills);
        let system_prompt = if skills_section.is_empty() {
            system_prompt
        } else {
            format!("{}{}", system_prompt, skills_section)
        };

        // Use frontend-provided thinking budget, or default per provider
        let thinking_budget_val = thinking_budget.unwrap_or_else(|| {
            if task_provider_type == "Claude" { 10000 } else { 0 }
        });

        let large_context = provider_entry.as_ref().map(|p| p.large_context).unwrap_or(false);
        let context_window = rustic_agent::task::condense::get_context_window(&task_model, large_context);

        let config = ProviderConfig {
            api_key: provider_entry
                .as_ref()
                .map(|p| p.api_key.clone())
                .unwrap_or_default(),
            model: task_model,
            max_tokens,
            temperature,
            base_url: provider_entry.as_ref().and_then(|p| p.base_url.clone()),
            system_prompt: Some(system_prompt),
            thinking_budget: thinking_budget_val,
            context_window,
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
        let turn_budget_max = agent.default_turn_budget;
        let mcp_arc = Arc::clone(&agent.mcp_manager);
        let ai_config = Arc::new(agent.ai_config.clone());

        // Auto-load .mcp.json from the project root into the MCP manager
        let mcp_json_path = project_root.join(".mcp.json");
        if mcp_json_path.exists() {
            let _ = mcp_arc.lock().unwrap().load_from_json_file(&mcp_json_path);
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
            turn_budget_max,
            mcp_arc,
            ai_config,
            allowed_paths,
        )
    };

    let provider: Arc<dyn AiProvider> = match provider_type_str.as_str() {
        "Claude" => Arc::new(ClaudeProvider::new()),
        "OpenAi" => Arc::new(OpenAiProvider::new()),
        "Compatible" => Arc::new(CompatibleProvider::new("Compatible".to_string())),
        _ => Arc::new(ClaudeProvider::new()),
    };

    let task_id_clone = task_id.clone();
    let app_clone = app.clone();

    // Emit status change
    let _ = app.emit(
        "agent-task-status",
        AgentStatusEvent {
            task_id: task_id.clone(),
            status: TaskStatus::Running,
        },
    );

    // Clone shared maps for background thread
    let db_arc = Arc::clone(&state.db);
    let task_costs_arc = Arc::clone(&state.task_costs);
    let turn_budgets_arc = Arc::clone(&state.turn_budgets);
    let file_lock = Arc::clone(&state.file_lock);
    let subagent_registry = Arc::clone(&state.subagent_registry);

    // Create turn budget for this run and store in the shared map so extend_turn_budget can find it
    let turn_budget = TurnBudget::new(turn_budget_max);
    {
        let mut map = turn_budgets_arc.lock().unwrap();
        map.insert(task_id.clone(), Arc::clone(&turn_budget));
    }

    // Spawn async task for the agentic loop
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
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

            let context = ToolContext {
                project_root: PathBuf::from(&project_root),
                shared_permissions: shared_perms.clone(),
                snapshot_fn: Some(snapshot_fn),
                compute_diff_fn: Some(compute_diff_fn),
                cancel_token: Some(Arc::clone(&cancel_token)),
                permission_broker,
                event_tx: event_tx.clone(),
                task_id: task_id_clone.clone(),
                turn_budget,
                file_lock: Arc::clone(&file_lock),
                mcp_manager: Some(Arc::clone(&mcp_manager_arc)),
                mcp_tool_defs,
                subagent_registry: Arc::clone(&subagent_registry),
                agent_depth: 0,
                ai_config: Arc::clone(&ai_config),
                allowed_paths,
                question_broker: Arc::clone(&question_broker),
                parent_provider_config: Some(parent_provider_config),
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
                        TaskEvent::TaskComplete { task_id, summary, notes, diff } => {
                            let _ = app_events.emit("agent-task-complete", AgentTaskCompleteEvent {
                                task_id,
                                summary,
                                notes,
                                diff,
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
                            if let Ok(mut map) = cost_map.lock() {
                                map.insert(task_id.clone(), cost.clone());
                            }
                            // Persist cost to DB
                            if let Ok(db) = cost_db.lock() {
                                let _ = db.update_task_cost(
                                    &task_id,
                                    cost.total_input_tokens as i64,
                                    cost.total_output_tokens as i64,
                                    cost.total_cache_read_tokens as i64,
                                    cost.estimated_cost_usd,
                                    cost.turn_count as i64,
                                );
                            }
                            let _ = app_events.emit("agent-cost-update", AgentCostUpdateEvent { task_id, cost });
                        }
                        TaskEvent::TurnBudgetWarning { task_id, turns_remaining } => {
                            let _ = app_events.emit("agent-turn-budget-warning", AgentTurnBudgetWarningEvent { task_id, turns_remaining });
                        }
                        TaskEvent::MemoryUpdated { task_id } => {
                            let _ = app_events.emit("agent-memory-updated", AgentMemoryUpdatedEvent { task_id });
                        }
                        TaskEvent::SubagentSpawned { task_id, agent_id, model, prompt } => {
                            eprintln!("[tauri] subagent spawned: task={} agent={} model={}", task_id, agent_id, model);
                            let _ = app_events.emit("agent-subagent-spawned", AgentSubagentSpawnedEvent { task_id, agent_id, model, prompt });
                        }
                        TaskEvent::SubagentCompleted { task_id, agent_id, summary } => {
                            eprintln!("[tauri] subagent completed: task={} agent={} summary_len={}", task_id, agent_id, summary.len());
                            let _ = app_events.emit("agent-subagent-completed", AgentSubagentCompletedEvent { task_id, agent_id, summary });
                        }
                        TaskEvent::SubagentFailed { task_id, agent_id, error } => {
                            eprintln!("[tauri] subagent failed: task={} agent={} error={}", task_id, agent_id, error);
                            let _ = app_events.emit("agent-subagent-failed", AgentSubagentFailedEvent { task_id, agent_id, error });
                        }
                        TaskEvent::SubagentTextDelta { task_id, agent_id, text } => {
                            let _ = app_events.emit("agent-subagent-text-delta", AgentSubagentTextDeltaEvent { task_id, agent_id, text });
                        }
                        TaskEvent::SubagentCostUpdate { task_id, agent_id, cost } => {
                            eprintln!("[tauri] subagent cost update: task={} agent={} in={} out={} usd={:.4}",
                                task_id, agent_id, cost.total_input_tokens, cost.total_output_tokens, cost.estimated_cost_usd);
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
                        _ => {}
                    }
                }
            });

            let result = executor.run_turn(&mut messages, &context).await;

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

            // Persist all messages to DB after the turn completes
            if let Ok(db) = db_arc.lock() {
                let _ = db.delete_messages_for_task(&task_id_clone);
                for (i, msg) in messages.iter().enumerate() {
                    let role = match &msg.role {
                        Role::User => "user",
                        Role::Assistant => "assistant",
                        Role::System => "system",
                    };
                    if let Ok(content_json) = serde_json::to_string(&msg.content) {
                        let _ = db.insert_message(&MessageRow {
                            id: format!("{}-{}", task_id_clone, i),
                            task_id: task_id_clone.clone(),
                            role: role.to_string(),
                            content_json,
                            created_at: chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string(),
                            sort_order: i as i64,
                        });
                    }
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
) -> Result<Vec<Message>, String> {
    // If task is in memory and has messages, return those
    {
        let agent = state.agent.lock().unwrap();
        if let Some(task) = agent.tasks.get(&task_id) {
            if !task.messages.is_empty() {
                return Ok(task.messages.clone());
            }
        }
    }

    // Load from DB and deserialize
    let db = state.db.lock().unwrap();
    let rows = db.get_messages_for_task(&task_id).map_err(|e| e.to_string())?;
    drop(db);

    let mut messages = Vec::new();
    for row in &rows {
        let role = match row.role.as_str() {
            "user" => Role::User,
            _ => Role::Assistant,
        };
        let content: Vec<ContentBlock> = serde_json::from_str(&row.content_json)
            .unwrap_or_else(|_| vec![ContentBlock::Text { text: row.content_json.clone() }]);
        messages.push(Message { role, content });
    }

    // Hydrate into in-memory task if it exists
    if !rows.is_empty() {
        let mut agent = state.agent.lock().unwrap();
        if let Some(task) = agent.tasks.get_mut(&task_id) {
            task.messages = messages.clone();
        }
    }

    Ok(messages)
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
    let _ = db.delete_tasks_for_project(&project_id);
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
) -> Result<(), String> {
    let mut agent = state.agent.lock().unwrap();

    let pt = match provider_type.as_str() {
        "Claude" => ProviderType::Claude,
        "OpenAi" => ProviderType::OpenAi,
        "Gemini" => ProviderType::Gemini,
        "Compatible" => ProviderType::Compatible,
        _ => return Err(format!("Unknown provider type: {}", provider_type)),
    };

    // Update or add provider entry
    if let Some(entry) = agent.ai_config.providers.iter_mut().find(|p| p.provider_type == pt) {
        entry.api_key = api_key;
        entry.default_model = model;
        entry.base_url = base_url;
        entry.enabled = true;
        if let Some(lc) = large_context { entry.large_context = lc; }
    } else {
        agent.ai_config.providers.push(rustic_agent::ProviderEntry {
            provider_type: pt.clone(),
            api_key,
            default_model: model,
            base_url,
            enabled: true,
            large_context: large_context.unwrap_or(false),
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

#[tauri::command]
pub async fn fetch_ai_models(
    provider_type: String,
    api_key: String,
    base_url: Option<String>,
) -> Result<Vec<String>, String> {
    let client = reqwest::Client::new();

    match provider_type.as_str() {
        "Claude" => {
            let res = client
                .get("https://api.anthropic.com/v1/models")
                .header("x-api-key", &api_key)
                .header("anthropic-version", "2023-06-01")
                .send()
                .await
                .map_err(|e| e.to_string())?;
            if !res.status().is_success() {
                return Err(format!("HTTP {}", res.status()));
            }
            let data: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;
            // Keep clean alias names (haiku/sonnet/opus, all tiers).
            // Dated snapshots end with an 8-digit date segment (e.g. -20250514) — skip those.
            let mut models: Vec<String> = data["data"]
                .as_array()
                .unwrap_or(&vec![])
                .iter()
                .filter_map(|m| m["id"].as_str().map(|s| s.to_string()))
                .filter(|id| {
                    let is_claude = id.starts_with("claude-");
                    let has_tier = id.contains("haiku") || id.contains("sonnet") || id.contains("opus");
                    // Skip dated snapshots: last dash-segment is 8 digits
                    let is_dated = id.split('-').last()
                        .map(|s| s.len() == 8 && s.chars().all(|c| c.is_ascii_digit()))
                        .unwrap_or(false);
                    is_claude && has_tier && !is_dated
                })
                .collect();
            models.sort();
            Ok(models)
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
            // GPT-5 and chatgpt-5 only; no GPT-4, no o-series, no noise
            let allowed_prefixes = ["gpt-5", "chatgpt-5"];
            let excluded_keywords = ["audio", "realtime", "tts", "whisper", "dall-e",
                                     "embedding", "moderation", "search", "transcri"];
            let mut models: Vec<String> = data["data"]
                .as_array()
                .unwrap_or(&vec![])
                .iter()
                .filter_map(|m| m["id"].as_str().map(|s| s.to_string()))
                .filter(|id| allowed_prefixes.iter().any(|p| id.starts_with(p)))
                .filter(|id| !excluded_keywords.iter().any(|kw| id.contains(kw)))
                .collect();
            models.sort();
            Ok(models)
        }
        "Gemini" => {
            let res = client
                .get("https://generativelanguage.googleapis.com/v1beta/models")
                .query(&[("key", api_key.as_str()), ("pageSize", "100")])
                .send()
                .await
                .map_err(|e| e.to_string())?;
            if !res.status().is_success() {
                return Err(format!("HTTP {}", res.status()));
            }
            let data: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;
            let generate_content = serde_json::json!("generateContent");
            // Only the stable "latest" alias pointers — one per model family
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
                .filter(|id| id.ends_with("-latest"))
                .collect();
            models.sort();
            Ok(models)
        }
        "Compatible" => {
            // Strip accidental full-path suffixes — users often paste the chat endpoint
            let raw = base_url.unwrap_or_default();
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
                // Exclude audio-only / non-text models
                .filter(|id| {
                    let id_lower = id.to_lowercase();
                    !["tts", "whisper", "dall-e", "embedding", "moderation",
                      "speech", "audio", "image-gen", "transcri"]
                        .iter()
                        .any(|kw| id_lower.contains(kw))
                })
                .collect();
            models.sort();
            Ok(models)
        }
        _ => Err(format!("Unknown provider type: {}", provider_type)),
    }
}

// === MCP commands ===

#[tauri::command]
pub fn add_mcp_server(
    state: State<'_, AppState>,
    name: String,
    transport_type: String,
    command: Option<String>,
    args: Option<Vec<String>>,
    url: Option<String>,
) -> Result<ServerConfig, String> {
    let id = uuid::Uuid::new_v4().to_string();

    let transport = match transport_type.as_str() {
        "stdio" => McpTransport::Stdio {
            command: command.unwrap_or_default(),
            args: args.unwrap_or_default(),
            env: std::collections::HashMap::new(),
        },
        "sse" => McpTransport::Sse {
            url: url.unwrap_or_default(),
            headers: std::collections::HashMap::new(),
        },
        _ => return Err(format!("Unknown transport type: {}", transport_type)),
    };

    let config = ServerConfig {
        id: id.clone(),
        name,
        transport,
        enabled: true,
        source: McpSource::Manual,
    };

    let agent = state.agent.lock().unwrap();
    agent.mcp_manager.lock().unwrap().add_server(config.clone());
    Ok(config)
}

#[tauri::command]
pub fn remove_mcp_server(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let agent = state.agent.lock().unwrap();
    agent.mcp_manager.lock().unwrap().remove_server(&id);
    Ok(())
}

#[tauri::command]
pub fn list_mcp_servers(state: State<'_, AppState>) -> Result<Vec<ServerConfig>, String> {
    let mcp_arc = Arc::clone(&state.agent.lock().unwrap().mcp_manager);
    let servers = mcp_arc.lock().unwrap().list_servers();
    Ok(servers)
}

#[tauri::command]
pub fn test_mcp_server(state: State<'_, AppState>, id: String) -> Result<Vec<ToolDef>, String> {
    let mcp_arc = Arc::clone(&state.agent.lock().unwrap().mcp_manager);
    let result = mcp_arc.lock().unwrap().test_server(&id).map_err(|e| e.to_string());
    result
}

#[tauri::command]
pub fn extend_turn_budget(
    state: State<'_, AppState>,
    task_id: String,
    additional: u32,
) -> Result<(), String> {
    let map = state.turn_budgets.lock().map_err(|e| e.to_string())?;
    if let Some(budget) = map.get(&task_id) {
        let mut b = budget.lock().map_err(|e| e.to_string())?;
        b.max += additional;
        Ok(())
    } else {
        // Task not currently running — update the default for the next run
        let mut agent = state.agent.lock().map_err(|e| e.to_string())?;
        agent.default_turn_budget += additional;
        Ok(())
    }
}

#[tauri::command]
pub fn get_task_cost(state: State<'_, AppState>, task_id: String) -> Result<TaskCost, String> {
    let map = state.task_costs.lock().map_err(|e| e.to_string())?;
    Ok(map.get(&task_id).cloned().unwrap_or_default())
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
pub fn import_mcp_json(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<usize, String> {
    let path = {
        let workspace = state.workspace.lock().unwrap();
        let project = workspace
            .list_projects()
            .into_iter()
            .find(|p| p.id.to_string() == project_id)
            .ok_or_else(|| "Project not found".to_string())?;
        project.root_path.join(".mcp.json")
    };
    if !path.exists() {
        return Err(".mcp.json not found in project root".to_string());
    }
    let mcp_arc = Arc::clone(&state.agent.lock().unwrap().mcp_manager);
    let result = mcp_arc
        .lock()
        .unwrap()
        .load_from_json_file(&path)
        .map_err(|e| e.to_string());
    result
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

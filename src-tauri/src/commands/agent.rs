use crate::state::{AgentTask, AppState};
use rustic_agent::{
    AiConfig, AiProvider, ContentBlock, McpTransport, Message, PermissionLevel, ProviderConfig,
    ProviderType, Role, ServerConfig, TaskEvent, TaskExecutor, TaskInfo, TaskStatus, ToolContext,
    ToolDef, checkpoint_ops,
};
use rustic_agent::tools::SnapshotFn;
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
struct AgentStatusEvent {
    task_id: String,
    status: TaskStatus,
}

#[tauri::command]
pub fn create_task(
    state: State<'_, AppState>,
    project_id: String,
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
        task_id,
        AgentTask {
            info: info.clone(),
            messages: Vec::new(),
            permissions,
        },
    );

    Ok(info)
}

#[tauri::command]
pub fn send_message(
    app: AppHandle,
    state: State<'_, AppState>,
    task_id: String,
    message: String,
) -> Result<(), String> {
    let (mut messages, project_root, permissions, provider_config, provider_type_str, checkpoint_id) = {
        let mut agent = state.agent.lock().unwrap();

        // Read config values first (immutable access)
        let max_tokens = agent.ai_config.max_tokens;
        let temperature = agent.ai_config.temperature;

        let task = agent
            .tasks
            .get_mut(&task_id)
            .ok_or_else(|| format!("Task not found: {}", task_id))?;

        // Add user message
        task.messages.push(Message {
            role: Role::User,
            content: vec![ContentBlock::Text { text: message }],
        });
        task.info.status = TaskStatus::Running;

        let message_index = task.messages.len() as i64 - 1;
        let task_provider_type = task.info.provider_type.clone();
        let task_model = task.info.model.clone();
        let task_project_id = task.info.project_id.clone();
        let task_messages = task.messages.clone();
        let task_permissions = task.permissions.clone();

        // Get project root
        let workspace = state.workspace.lock().unwrap();
        let project = workspace
            .list_projects()
            .into_iter()
            .find(|p| p.id.to_string() == task_project_id)
            .ok_or_else(|| "Project not found".to_string())?;
        let project_root = project.root_path.clone();

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

        let config = ProviderConfig {
            api_key: provider_entry
                .as_ref()
                .map(|p| p.api_key.clone())
                .unwrap_or_default(),
            model: task_model,
            max_tokens,
            temperature,
            base_url: provider_entry.as_ref().and_then(|p| p.base_url.clone()),
        };

        (
            task_messages,
            project_root,
            task_permissions,
            config,
            task_provider_type,
            checkpoint_id,
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

    // Clone DB Arc for background thread
    let db_arc = Arc::clone(&state.db);

    // Spawn async task for the agentic loop
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let executor = TaskExecutor::new(provider, provider_config);

            // Build snapshot closure that captures DB and checkpoint ID
            let db_for_snapshot = Arc::clone(&db_arc);
            let cp_id = checkpoint_id.clone();
            let snapshot_fn: SnapshotFn = Arc::new(move |path: &std::path::Path| {
                if let Ok(db) = db_for_snapshot.lock() {
                    let _ = checkpoint_ops::snapshot_file(&db, &cp_id, path);
                }
            });

            let context = ToolContext {
                project_root: PathBuf::from(&project_root),
                permissions,
                snapshot_fn: Some(snapshot_fn),
            };

            let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();

            // Forward events to Tauri
            let app_events = app_clone.clone();
            let _task_id_events = task_id_clone.clone();
            tokio::spawn(async move {
                while let Some(event) = event_rx.recv().await {
                    match event {
                        TaskEvent::TextDelta { task_id, text } => {
                            let _ = app_events.emit("agent-stream", AgentStreamEvent { task_id, text });
                        }
                        TaskEvent::ToolUse { task_id, tool_name, tool_input } => {
                            let _ = app_events.emit("agent-tool-use", AgentToolUseEvent { task_id, tool_name, tool_input });
                        }
                        TaskEvent::ToolResult { task_id, tool_use_id, output, is_error } => {
                            let _ = app_events.emit("agent-tool-result", AgentToolResultEvent { task_id, tool_use_id, output, is_error });
                        }
                        TaskEvent::StatusChange { task_id, status } => {
                            let _ = app_events.emit("agent-task-status", AgentStatusEvent { task_id, status });
                        }
                        _ => {}
                    }
                }
            });

            let result = executor.run_turn(&task_id_clone, &mut messages, &context, &event_tx).await;

            let final_status = match result {
                Ok(()) => TaskStatus::Completed,
                Err(e) => {
                    let _ = app_clone.emit("agent-stream", AgentStreamEvent {
                        task_id: task_id_clone.clone(),
                        text: format!("\n\nError: {}", e),
                    });
                    TaskStatus::Failed
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
    let agent = state.agent.lock().unwrap();
    let tasks: Vec<TaskInfo> = agent
        .tasks
        .values()
        .filter(|t| {
            project_id
                .as_ref()
                .map(|pid| &t.info.project_id == pid)
                .unwrap_or(true)
        })
        .map(|t| t.info.clone())
        .collect();
    Ok(tasks)
}

#[tauri::command]
pub fn get_task_messages(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<Vec<Message>, String> {
    let agent = state.agent.lock().unwrap();
    let task = agent
        .tasks
        .get(&task_id)
        .ok_or_else(|| format!("Task not found: {}", task_id))?;
    Ok(task.messages.clone())
}

#[tauri::command]
pub fn delete_task(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<(), String> {
    let mut agent = state.agent.lock().unwrap();
    agent.tasks.remove(&task_id);
    Ok(())
}

#[tauri::command]
pub fn set_ai_provider(
    state: State<'_, AppState>,
    provider_type: String,
    api_key: String,
    model: String,
    base_url: Option<String>,
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
    } else {
        agent.ai_config.providers.push(rustic_agent::ProviderEntry {
            provider_type: pt.clone(),
            api_key,
            default_model: model,
            base_url,
            enabled: true,
        });
    }

    if agent.ai_config.default_provider.is_none() {
        agent.ai_config.default_provider = Some(pt);
    }

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
    let perm = match level.as_str() {
        "Admin" => PermissionLevel::Admin,
        "ReadWrite" => PermissionLevel::ReadWrite,
        "ReadOnly" => PermissionLevel::ReadOnly,
        _ => return Err(format!("Unknown permission level: {}", level)),
    };

    let mut agent = state.agent.lock().unwrap();
    if let Some(pid) = project_id {
        agent.project_permissions.insert(pid, perm);
    }
    Ok(())
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
            let mut models: Vec<String> = data["data"]
                .as_array()
                .unwrap_or(&vec![])
                .iter()
                .filter_map(|m| m["id"].as_str().map(|s| s.to_string()))
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
            let chat_prefixes = ["gpt-", "o1", "o3", "o4", "chatgpt"];
            let mut models: Vec<String> = data["data"]
                .as_array()
                .unwrap_or(&vec![])
                .iter()
                .filter_map(|m| m["id"].as_str().map(|s| s.to_string()))
                .filter(|id| chat_prefixes.iter().any(|p| id.starts_with(p)))
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
                .collect();
            models.sort();
            Ok(models)
        }
        "Compatible" => {
            let base = base_url
                .unwrap_or_default()
                .trim_end_matches('/')
                .to_string();
            let url = format!("{}/models", base);
            let res = client
                .get(&url)
                .header("Authorization", format!("Bearer {}", api_key))
                .send()
                .await
                .map_err(|e| e.to_string())?;
            if !res.status().is_success() {
                return Err(format!("HTTP {}", res.status()));
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
    };

    let mut agent = state.agent.lock().unwrap();
    agent.mcp_manager.add_server(config.clone());
    Ok(config)
}

#[tauri::command]
pub fn remove_mcp_server(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let mut agent = state.agent.lock().unwrap();
    agent.mcp_manager.remove_server(&id);
    Ok(())
}

#[tauri::command]
pub fn list_mcp_servers(state: State<'_, AppState>) -> Result<Vec<ServerConfig>, String> {
    let agent = state.agent.lock().unwrap();
    Ok(agent.mcp_manager.list_servers())
}

#[tauri::command]
pub fn test_mcp_server(state: State<'_, AppState>, id: String) -> Result<Vec<ToolDef>, String> {
    let mut agent = state.agent.lock().unwrap();
    agent.mcp_manager.test_server(&id).map_err(|e| e.to_string())
}

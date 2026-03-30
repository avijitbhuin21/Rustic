use anyhow::Result;
use serde_json::{json, Value};
use std::sync::Arc;
use crate::config::ProviderType;
use crate::provider::{AiProvider, ProviderConfig};
use crate::provider::claude::ClaudeProvider;
use crate::provider::openai::OpenAiProvider;
use crate::provider::compatible::CompatibleProvider;
use crate::task::TaskEvent;
use crate::task::subagent::SubagentResult;
use crate::tools::{ToolContext, ToolOutput};
use crate::provider::ToolDef;

pub fn definitions() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "spawn_subagent".to_string(),
            description: "Spawn a parallel sub-agent to handle a task concurrently. The sub-agent runs independently and you can continue other work. Use wait_for_all_agents or wait for reactive injection to get results. Only available to the main agent (not sub-agents).".to_string(),
            parameters: json!({
                "type": "object",
                "required": ["agent_id", "task", "model"],
                "properties": {
                    "agent_id": { "type": "string", "description": "Unique ID for this sub-agent (e.g. 'auth-refactor', 'test-writer')" },
                    "task": { "type": "string", "description": "Full task description for the sub-agent" },
                    "model": { "type": "string", "description": "Model to use for this sub-agent (e.g. 'claude-haiku-4-5-20251001', 'claude-sonnet-4-20250514')" },
                    "files": { "type": "array", "items": { "type": "string" }, "description": "Optional: files this sub-agent will work with (for conflict avoidance documentation)" }
                }
            }),
        },
        ToolDef {
            name: "wait_for_all_agents".to_string(),
            description: "Block until all specified sub-agents complete. Returns their results.".to_string(),
            parameters: json!({
                "type": "object",
                "required": ["agent_ids"],
                "properties": {
                    "agent_ids": { "type": "array", "items": { "type": "string" }, "description": "List of agent IDs to wait for" }
                }
            }),
        },
        ToolDef {
            name: "list_active_agents".to_string(),
            description: "List all currently running sub-agents and their status.".to_string(),
            parameters: json!({ "type": "object", "properties": {} }),
        },
        ToolDef {
            name: "cancel_agent".to_string(),
            description: "Cancel a running sub-agent by ID.".to_string(),
            parameters: json!({
                "type": "object",
                "required": ["agent_id"],
                "properties": {
                    "agent_id": { "type": "string" }
                }
            }),
        },
    ]
}

pub async fn execute(name: &str, params: Value, context: &ToolContext) -> Result<ToolOutput> {
    match name {
        "spawn_subagent" => spawn_subagent(params, context).await,
        "wait_for_all_agents" => wait_for_all_agents(params, context).await,
        "list_active_agents" => list_active_agents(context).await,
        "cancel_agent" => cancel_agent(params, context).await,
        _ => Ok(ToolOutput { content: format!("Unknown tool: {}", name), is_error: true }),
    }
}

async fn spawn_subagent(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    if context.agent_depth >= 1 {
        return Ok(ToolOutput {
            content: "PERMISSION_DENIED: Sub-agents cannot spawn further sub-agents (max depth 1).".to_string(),
            is_error: true,
        });
    }

    let agent_id = params["agent_id"].as_str().unwrap_or("").to_string();
    let task_desc = params["task"].as_str().unwrap_or("").to_string();
    let model = params["model"].as_str().unwrap_or("").to_string();

    if agent_id.is_empty() || task_desc.is_empty() || model.is_empty() {
        return Ok(ToolOutput {
            content: "Missing required parameters: agent_id, task, model".to_string(),
            is_error: true,
        });
    }

    // Find provider config for the specified model
    let provider_entry = context.ai_config.providers.iter().find(|p| {
        let model_lower = model.to_lowercase();
        match p.provider_type {
            ProviderType::Claude => model_lower.starts_with("claude"),
            ProviderType::OpenAi => model_lower.starts_with("gpt") || model_lower.starts_with("o1") || model_lower.starts_with("o3"),
            ProviderType::Gemini => model_lower.starts_with("gemini"),
            ProviderType::Compatible => true, // fallback
        }
    });

    let Some(entry) = provider_entry else {
        return Ok(ToolOutput {
            content: format!("No configured provider found for model: {}", model),
            is_error: true,
        });
    };

    let provider: Arc<dyn AiProvider> = match entry.provider_type {
        ProviderType::Claude => Arc::new(ClaudeProvider::new()),
        ProviderType::OpenAi => Arc::new(OpenAiProvider::new()),
        ProviderType::Compatible => Arc::new(CompatibleProvider::new("Compatible".to_string())),
        ProviderType::Gemini => Arc::new(OpenAiProvider::new()), // fallback
    };

    let sub_config = ProviderConfig {
        api_key: entry.api_key.clone(),
        model: model.clone(),
        max_tokens: context.ai_config.max_tokens,
        temperature: context.ai_config.temperature,
        base_url: entry.base_url.clone(),
        system_prompt: Some(format!(
            "You are a sub-agent performing a specific task. Complete the task and call task_complete when done.\n\
             Shell environment: {}\n\
             When your task is fully complete, call task_complete immediately.",
            if cfg!(target_os = "windows") { "PowerShell on Windows" }
            else if cfg!(target_os = "macos") { "bash on macOS" }
            else { "bash on Linux" }
        )),
    };

    // Register sub-agent
    context.subagent_registry.register(&context.task_id, &agent_id, &model);

    // Emit spawned event
    let _ = context.event_tx.send(TaskEvent::SubagentSpawned {
        task_id: context.task_id.clone(),
        agent_id: agent_id.clone(),
        model: model.clone(),
    });

    // Clone values for the spawned task
    let parent_task_id = context.task_id.clone();
    let agent_id_clone = agent_id.clone();
    let registry = Arc::clone(&context.subagent_registry);
    let parent_event_tx = context.event_tx.clone();
    let child_project_root = context.project_root.clone();
    let child_permissions = context.permissions.clone();
    let child_snapshot_fn = context.snapshot_fn.clone();
    let child_compute_diff_fn = context.compute_diff_fn.clone();
    let child_file_lock = Arc::clone(&context.file_lock);
    let child_permission_broker = Arc::clone(&context.permission_broker);
    let child_turn_budget = crate::task::TurnBudget::new(30); // sub-agents get 30 turns
    let child_ai_config = Arc::clone(&context.ai_config);
    let child_mcp_manager = context.mcp_manager.clone();
    let child_mcp_tool_defs = context.mcp_tool_defs.clone();
    let child_subagent_registry = Arc::clone(&context.subagent_registry);
    let child_allowed_paths = context.allowed_paths.clone();

    tokio::spawn(async move {
        use crate::task::executor::TaskExecutor;
        use crate::provider::{Message, Role, ContentBlock};

        // Create a child event channel that forwards text events to parent as SubagentTextDelta
        let (child_event_tx, mut child_event_rx) = tokio::sync::mpsc::unbounded_channel::<TaskEvent>();

        // Forward sub-agent events to parent
        let fwd_parent_tx = parent_event_tx.clone();
        let fwd_task_id = parent_task_id.clone();
        let fwd_agent_id = agent_id_clone.clone();
        tokio::spawn(async move {
            while let Some(event) = child_event_rx.recv().await {
                match event {
                    TaskEvent::TextDelta { text, .. } => {
                        let _ = fwd_parent_tx.send(TaskEvent::SubagentTextDelta {
                            task_id: fwd_task_id.clone(),
                            agent_id: fwd_agent_id.clone(),
                            text,
                        });
                    }
                    _ => {} // Sub-agent other events are not forwarded
                }
            }
        });

        let child_context = ToolContext {
            project_root: child_project_root,
            permissions: child_permissions,
            snapshot_fn: child_snapshot_fn,
            compute_diff_fn: child_compute_diff_fn.clone(),
            cancel_token: None,
            sensitive_files_allowed: false,
            permission_broker: child_permission_broker,
            event_tx: child_event_tx,
            task_id: format!("{}/{}", parent_task_id, agent_id_clone),
            turn_budget: child_turn_budget,
            file_lock: child_file_lock,
            mcp_manager: child_mcp_manager,
            mcp_tool_defs: child_mcp_tool_defs,
            subagent_registry: child_subagent_registry,
            agent_depth: 1,
            ai_config: child_ai_config,
            allowed_paths: child_allowed_paths,
        };

        let executor = TaskExecutor::new(provider, sub_config);
        let mut messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text { text: task_desc }],
        }];

        let result = executor.run_turn(&mut messages, &child_context).await;

        let (summary, diff) = match result {
            Ok(()) => {
                let diff = child_context.compute_diff_fn
                    .as_ref()
                    .map(|f| f())
                    .unwrap_or_else(|| crate::checkpoint::TaskDiff {
                        files: Vec::new(),
                        total_insertions: 0,
                        total_deletions: 0,
                    });
                // Extract summary from the last assistant message
                let summary = messages.iter().rev()
                    .find(|m| matches!(m.role, Role::Assistant))
                    .and_then(|m| m.content.iter().find_map(|b| {
                        if let ContentBlock::Text { text } = b { Some(text.clone()) } else { None }
                    }))
                    .unwrap_or_else(|| "Sub-agent completed.".to_string());
                // Trim to 500 chars if very long
                let summary = if summary.len() > 500 {
                    format!("{}…", &summary[..500])
                } else {
                    summary
                };
                (summary, diff)
            }
            Err(e) => {
                let err = format!("Sub-agent error: {}", e);
                registry.fail(&parent_task_id, &agent_id_clone, err.clone());
                let _ = parent_event_tx.send(TaskEvent::SubagentFailed {
                    task_id: parent_task_id.clone(),
                    agent_id: agent_id_clone.clone(),
                    error: err,
                });
                return;
            }
        };

        let sub_result = SubagentResult {
            agent_id: agent_id_clone.clone(),
            model: String::new(),
            summary: summary.clone(),
            notes: None,
            diff,
        };
        registry.complete(&parent_task_id, sub_result);
        let _ = parent_event_tx.send(TaskEvent::SubagentCompleted {
            task_id: parent_task_id,
            agent_id: agent_id_clone,
            summary,
        });
    });

    Ok(ToolOutput {
        content: format!("Sub-agent '{}' spawned with model {}. It will run in parallel.", agent_id, model),
        is_error: false,
    })
}

async fn wait_for_all_agents(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    let agent_ids: Vec<String> = params["agent_ids"]
        .as_array()
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();

    if agent_ids.is_empty() {
        return Ok(ToolOutput {
            content: "No agent IDs specified.".to_string(),
            is_error: false,
        });
    }

    let mut results = Vec::new();
    let mut remaining: std::collections::HashSet<String> = agent_ids.into_iter().collect();

    while !remaining.is_empty() {
        match context.subagent_registry.wait_for_any(&context.task_id).await {
            None => break,
            Some(crate::task::subagent::SubagentCompletionEvent::Completed(r)) => {
                if remaining.remove(&r.agent_id) {
                    results.push(format!("Agent '{}' completed: {}", r.agent_id, r.summary));
                }
            }
            Some(crate::task::subagent::SubagentCompletionEvent::Failed { agent_id, error }) => {
                if remaining.remove(&agent_id) {
                    results.push(format!("Agent '{}' FAILED: {}", agent_id, error));
                }
            }
        }
    }

    Ok(ToolOutput {
        content: if results.is_empty() {
            "No matching agents found or all already completed.".to_string()
        } else {
            results.join("\n")
        },
        is_error: false,
    })
}

async fn list_active_agents(context: &ToolContext) -> Result<ToolOutput> {
    let agents = context.subagent_registry.all_for_task(&context.task_id);
    if agents.is_empty() {
        return Ok(ToolOutput {
            content: "No sub-agents for this task.".to_string(),
            is_error: false,
        });
    }
    let lines: Vec<String> = agents.iter().map(|a| {
        let status = match a.status {
            crate::task::subagent::SubagentStatus::Running => "Running",
            crate::task::subagent::SubagentStatus::Completed => "Completed",
            crate::task::subagent::SubagentStatus::Failed => "Failed",
        };
        format!("- {} [{}] — {}", a.agent_id, a.model, status)
    }).collect();
    Ok(ToolOutput {
        content: lines.join("\n"),
        is_error: false,
    })
}

async fn cancel_agent(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    let agent_id = params["agent_id"].as_str().unwrap_or("").to_string();
    if agent_id.is_empty() {
        return Ok(ToolOutput { content: "agent_id required".to_string(), is_error: true });
    }
    // Best-effort: mark as failed in registry (no hard cancellation without cancel token per agent)
    context.subagent_registry.fail(
        &context.task_id,
        &agent_id,
        "Cancelled by orchestrator.".to_string(),
    );
    Ok(ToolOutput {
        content: format!("Sub-agent '{}' cancellation requested.", agent_id),
        is_error: false,
    })
}

use anyhow::Result;
use serde_json::{json, Value};
use std::sync::Arc;
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
            description: "Launch a sub-agent to handle a task in parallel. The sub-agent inherits \
                          the same model, tools, and system prompt as the main agent. It runs \
                          independently and results are automatically injected when ready. \
                          Use this aggressively for independent tasks to maximize parallelism. \
                          Only the main agent can spawn sub-agents (depth limit: 1).".to_string(),
            parameters: json!({
                "type": "object",
                "required": ["prompt", "description"],
                "properties": {
                    "prompt": {
                        "type": "string",
                        "description": "The full task description and instructions for the sub-agent. \
                                        Be specific and self-contained — include all context the sub-agent \
                                        needs to complete the task independently."
                    },
                    "description": {
                        "type": "string",
                        "description": "A short (3-5 word) summary of what this agent will do. \
                                        Used as the agent's display name. E.g. 'refactor auth module', \
                                        'write unit tests', 'fix login bug'."
                    }
                }
            }),
        },
        ToolDef {
            name: "list_active_agents".to_string(),
            description: "List all sub-agents and their current status (running, completed, failed).".to_string(),
            parameters: json!({ "type": "object", "properties": {} }),
        },
    ]
}

pub async fn execute(name: &str, params: Value, context: &ToolContext) -> Result<ToolOutput> {
    match name {
        "spawn_subagent" => spawn_subagent(params, context).await,
        "list_active_agents" => list_active_agents(context).await,
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

    let prompt = params["prompt"].as_str().unwrap_or("").to_string();
    let description = params["description"].as_str().unwrap_or("").to_string();

    if prompt.is_empty() {
        return Ok(ToolOutput {
            content: "Missing required parameter: prompt".to_string(),
            is_error: true,
        });
    }

    // Generate a short agent ID from the description (or a random suffix)
    let agent_id = if description.is_empty() {
        format!("agent-{}", &uuid::Uuid::new_v4().to_string()[..8])
    } else {
        // Sanitize description into a slug-like ID
        let slug: String = description
            .to_lowercase()
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '-' })
            .collect::<String>()
            .trim_matches('-')
            .to_string();
        let slug = if slug.len() > 30 { slug[..30].to_string() } else { slug };
        if slug.is_empty() {
            format!("agent-{}", &uuid::Uuid::new_v4().to_string()[..8])
        } else {
            slug
        }
    };

    // Use the same model & provider as the main agent
    let parent_config = context.parent_provider_config.as_ref().ok_or_else(|| {
        anyhow::anyhow!("No parent provider config available for sub-agent spawning")
    })?;
    let model = parent_config.model.clone();

    // Determine provider type from model name
    let model_lower = model.to_lowercase();
    let provider: Arc<dyn AiProvider> = if model_lower.starts_with("claude") {
        Arc::new(ClaudeProvider::new())
    } else if model_lower.starts_with("gpt") || model_lower.starts_with("o1") || model_lower.starts_with("o3") {
        Arc::new(OpenAiProvider::new())
    } else if model_lower.starts_with("gemini") {
        Arc::new(OpenAiProvider::new())
    } else {
        Arc::new(CompatibleProvider::new("Compatible".to_string()))
    };

    // Sub-agents inherit the parent's full system prompt
    let sub_system_prompt = parent_config.system_prompt.clone()
        .unwrap_or_else(|| crate::system_prompt::build_subagent_prompt());

    let sub_config = ProviderConfig {
        api_key: parent_config.api_key.clone(),
        model: model.clone(),
        max_tokens: parent_config.max_tokens,
        temperature: parent_config.temperature,
        base_url: parent_config.base_url.clone(),
        system_prompt: Some(sub_system_prompt),
        thinking_budget: parent_config.thinking_budget,
        context_window: parent_config.context_window,
        cancel_token: context.cancel_token.clone(),
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
    let child_shared_permissions = context.shared_permissions.clone();
    let child_snapshot_fn = context.snapshot_fn.clone();
    let child_compute_diff_fn = context.compute_diff_fn.clone();
    let child_file_lock = Arc::clone(&context.file_lock);
    let child_permission_broker = Arc::clone(&context.permission_broker);
    let child_turn_budget = crate::task::TurnBudget::new(30);
    let child_ai_config = Arc::clone(&context.ai_config);
    let child_mcp_manager = context.mcp_manager.clone();
    let child_mcp_tool_defs = context.mcp_tool_defs.clone();
    let child_subagent_registry = Arc::clone(&context.subagent_registry);
    let child_allowed_paths = context.allowed_paths.clone();
    let child_question_broker = Arc::clone(&context.question_broker);

    tokio::spawn(async move {
        use crate::task::executor::TaskExecutor;
        use crate::provider::{Message, Role, ContentBlock};

        // Create a child event channel that forwards text events to parent
        let (child_event_tx, mut child_event_rx) = tokio::sync::mpsc::unbounded_channel::<TaskEvent>();

        // Forward sub-agent text deltas to parent
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
                    _ => {}
                }
            }
        });

        let child_context = ToolContext {
            project_root: child_project_root,
            shared_permissions: child_shared_permissions,
            snapshot_fn: child_snapshot_fn,
            compute_diff_fn: child_compute_diff_fn.clone(),
            cancel_token: None,
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
            question_broker: child_question_broker,
            parent_provider_config: None, // sub-agents cannot spawn further sub-agents
        };

        let executor = TaskExecutor::new(provider, sub_config);
        let mut messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text { text: prompt }],
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
        content: format!(
            "Sub-agent '{}' spawned (model: {}). It will run in parallel and results will be \
             injected automatically when complete.",
            agent_id, model
        ),
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

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
                          independently and can read files, search code, and generate content on its own. \
                          IMPORTANT: Delegate the TASK, not the solution — tell the sub-agent WHAT to \
                          accomplish, not the exact content to write. Do NOT pre-read files or generate \
                          content yourself to pass in the prompt. The sub-agent has full tool access. \
                          Use `wait_for_subagents` to wait for results. \
                          Only the main agent can spawn sub-agents (depth limit: 1). \
                          Declare `writes` for any files the sub-agent will modify — spawning a \
                          sub-agent whose writes collide with an already-running one is rejected. \
                          Max concurrent sub-agents: 4.".to_string(),
            parameters: json!({
                "type": "object",
                "required": ["name", "prompt"],
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "A short (3-5 word) name for this sub-agent. Used as the \
                                        agent's display name and ID. E.g. 'refactor auth module', \
                                        'write unit tests', 'fix login bug'."
                    },
                    "prompt": {
                        "type": "string",
                        "description": "The task description for the sub-agent. Describe WHAT to do and \
                                        WHERE (file paths, directories), but do NOT include file contents \
                                        or pre-generated code. The sub-agent will read files and generate \
                                        content itself using its tools."
                    },
                    "writes": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "File or directory paths (repo-relative) this sub-agent will \
                                        create, edit, or delete. Used to detect collisions with other \
                                        running sub-agents. Leave empty for read-only tasks (research, \
                                        analysis, summarization). Directory entries cover everything \
                                        beneath them. Be tight — over-declaring serializes agents that \
                                        could have run in parallel."
                    },
                    "reads": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional: file or directory paths the sub-agent will read. \
                                        Informational only; reads never cause collisions."
                    }
                }
            }),
        },
        ToolDef {
            name: "list_active_agents".to_string(),
            description: "List all sub-agents and their current status (running, completed, failed). \
                          This is a non-blocking status check. If you want to wait for sub-agents \
                          to finish, call `wait_for_subagents` instead.".to_string(),
            parameters: json!({ "type": "object", "properties": {} }),
        },
        ToolDef {
            name: "wait_for_subagents".to_string(),
            description: "Block until at least one running sub-agent completes or fails, then return \
                          its result. Use this when you have spawned sub-agents and need to wait for \
                          their results before proceeding. You will be notified each time a sub-agent \
                          finishes — call this tool again if more sub-agents are still running and you \
                          need their results too. Do NOT poll with list_active_agents — use this instead.".to_string(),
            parameters: json!({ "type": "object", "properties": {} }),
        },
        ToolDef {
            name: "report_blocked_write".to_string(),
            description: "SUB-AGENT ONLY. Call this when you hit a WRITE_SCOPE_VIOLATION — to record \
                          that you needed to write a file outside your declared `writes` scope. The \
                          orchestrator will see the list of blocked writes in your final result and \
                          decide whether to do them itself, re-dispatch with expanded scope, or spawn \
                          a follow-up sub-agent. After calling this for every blocked write, call \
                          `complete_task` with a summary of what you DID finish. This tool does not \
                          end the task on its own — it only records. No-op when called by the main agent.".to_string(),
            parameters: json!({
                "type": "object",
                "required": ["path", "reason"],
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Repo-relative path of the file the sub-agent needed to write \
                                        but couldn't (outside declared `writes`)."
                    },
                    "reason": {
                        "type": "string",
                        "description": "Short (1-2 sentence) explanation of why this write was needed. \
                                        The orchestrator uses this to decide how to handle it."
                    }
                }
            }),
        },
    ]
}

pub async fn execute(name: &str, params: Value, context: &ToolContext) -> Result<ToolOutput> {
    // In the Global orchestrator, same-task sub-agents don't make sense —
    // the project_root they'd inherit is the Global internal scope, not a
    // real project. Direct the model to `spawn_subtask` instead, which
    // creates a top-level task in a chosen project.
    if context.is_global && name == "spawn_subagent" {
        return Ok(ToolOutput {
            content: "PERMISSION_DENIED: `spawn_subagent` is blocked in the \
                      Global scope. Use `spawn_subtask(project_id, prompt)` \
                      instead — it creates a top-level chat inside a specific \
                      project that the user can see in the agent panel."
                .into(),
            is_error: true,
        });
    }
    match name {
        "spawn_subagent" => spawn_subagent(params, context).await,
        "list_active_agents" => list_active_agents(context).await,
        "wait_for_subagents" => wait_for_subagents(context).await,
        "report_blocked_write" => report_blocked_write(params, context).await,
        _ => Ok(ToolOutput { content: format!("Unknown tool: {}", name), is_error: true }),
    }
}

async fn report_blocked_write(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    // Main agent calling this is a no-op — there's no orchestrator above it to
    // forward the report to. Keep the tool callable from any context so a model
    // mis-firing it doesn't explode, but explain what happened.
    if context.write_scope.is_none() {
        return Ok(ToolOutput {
            content: "report_blocked_write has no effect for the main agent — you have unrestricted \
                      write scope. If you hit a genuine permission failure, handle it directly."
                .to_string(),
            is_error: false,
        });
    }

    let path = params["path"].as_str().unwrap_or("").trim();
    let reason = params["reason"].as_str().unwrap_or("").trim();
    if path.is_empty() || reason.is_empty() {
        return Ok(ToolOutput {
            content: "report_blocked_write requires both `path` and `reason` to be non-empty strings."
                .to_string(),
            is_error: true,
        });
    }

    if let Ok(mut writes) = context.blocked_writes.lock() {
        writes.push(crate::task::subagent::BlockedWrite {
            path: path.to_string(),
            reason: reason.to_string(),
        });
    }

    Ok(ToolOutput {
        content: format!(
            "Recorded blocked write: '{}'. Call `complete_task` once you've finished what you can \
             and the orchestrator will see this in your result.",
            path
        ),
        is_error: false,
    })
}

async fn spawn_subagent(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    if context.agent_depth >= 1 {
        return Ok(ToolOutput {
            content: "PERMISSION_DENIED: Sub-agents cannot spawn further sub-agents (max depth 1).".to_string(),
            is_error: true,
        });
    }

    let name = params["name"].as_str().unwrap_or("").to_string();
    let prompt = params["prompt"].as_str().unwrap_or("").to_string();
    let writes: Vec<String> = params
        .get("writes")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_default();

    if prompt.is_empty() {
        return Ok(ToolOutput {
            content: "Missing required parameter: prompt".to_string(),
            is_error: true,
        });
    }

    // Concurrency cap — prevents rate-limit thrash and bounds resource use.
    {
        let running = context.subagent_registry.running_count(&context.task_id);
        if running >= crate::task::subagent::MAX_CONCURRENT_SUBAGENTS {
            return Ok(ToolOutput {
                content: format!(
                    "SPAWN_REJECTED: {} sub-agents already running (max = {}). \
                     Call `wait_for_subagents` to let at least one finish before spawning more.",
                    running,
                    crate::task::subagent::MAX_CONCURRENT_SUBAGENTS
                ),
                is_error: true,
            });
        }
    }

    // Collision check — reject if an active sibling already declared writes to
    // the same path. The model should either serialize (wait for the existing
    // agent) or redesign the work so writes are disjoint.
    if let Some(conflicting) = context
        .subagent_registry
        .find_write_collision(&context.task_id, &writes)
    {
        return Ok(ToolOutput {
            content: format!(
                "SPAWN_REJECTED: write collision with running sub-agent '{}'. \
                 Its declared writes overlap with yours. Either wait for '{}' to \
                 finish (`wait_for_subagents`) before spawning this one, or narrow \
                 your `writes` list so it doesn't overlap.",
                conflicting, conflicting
            ),
            is_error: true,
        });
    }

    // Generate a short agent ID from the name (or a random suffix)
    let agent_id = if name.is_empty() {
        format!("agent-{}", &uuid::Uuid::new_v4().to_string()[..8])
    } else {
        // Sanitize name into a slug-like ID
        let slug: String = name
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
        web_search_enabled: parent_config.web_search_enabled,
        web_fetch_enabled: parent_config.web_fetch_enabled,
        cancel_token: context.cancel_token.clone(),
    };

    // Register sub-agent (with declared writes for future collision checks)
    context.subagent_registry.register(&context.task_id, &agent_id, &model, writes.clone());
    eprintln!("[subagent] Registered '{}' under task '{}' with model '{}'", agent_id, context.task_id, model);

    // Emit spawned event
    let _ = context.event_tx.send(TaskEvent::SubagentSpawned {
        task_id: context.task_id.clone(),
        agent_id: agent_id.clone(),
        model: model.clone(),
        prompt: prompt.clone(),
    });

    // Clone values for the spawned task
    let parent_task_id = context.task_id.clone();
    let agent_id_clone = agent_id.clone();
    let registry = Arc::clone(&context.subagent_registry);
    let parent_event_tx = context.event_tx.clone();
    let child_project_root = context.project_root.clone();
    // Sub-agents always run in FullAuto without sensitive-file access,
    // regardless of the parent's level. A sub-agent that pauses on a
    // permission prompt would stall the parent waiting on its completion;
    // sensitive reads stay gated because the sub-agent can't escalate its
    // own trust boundary. `write_scope` (passed separately as
    // `child_write_scope`) still constrains exactly which files it can
    // touch.
    let child_shared_permissions = crate::task::permissions::SharedPermissions::new(
        crate::PermissionLevel::FullAuto,
        false,
    );
    let child_snapshot_fn = context.snapshot_fn.clone();
    let child_compute_diff_fn = context.compute_diff_fn.clone();
    let child_file_lock = Arc::clone(&context.file_lock);
    let child_permission_broker = Arc::clone(&context.permission_broker);
    let child_ai_config = Arc::clone(&context.ai_config);
    let child_tool_config = Arc::clone(&context.tool_config);
    let child_mcp_manager = context.mcp_manager.clone();
    let child_mcp_tool_defs = context.mcp_tool_defs.clone();
    let child_subagent_registry = Arc::clone(&context.subagent_registry);
    let child_allowed_paths = context.allowed_paths.clone();
    let child_question_broker = Arc::clone(&context.question_broker);
    let child_write_scope = writes.clone();
    let child_blocked_writes: Arc<std::sync::Mutex<Vec<crate::task::subagent::BlockedWrite>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let blocked_writes_for_result = Arc::clone(&child_blocked_writes);
    let child_agent_terminals = context.agent_terminals.clone();
    let child_is_global = context.is_global;
    let child_orchestrator_host = context.orchestrator_host.clone();

    tokio::spawn(async move {
        use crate::task::executor::TaskExecutor;
        use crate::provider::{Message, Role, ContentBlock};

        // Create a child event channel that forwards text events to parent
        let (child_event_tx, mut child_event_rx) = tokio::sync::mpsc::unbounded_channel::<TaskEvent>();

        // Forward sub-agent text/thinking deltas to parent as SubagentTextDelta
        let fwd_parent_tx = parent_event_tx.clone();
        let fwd_task_id = parent_task_id.clone();
        let fwd_agent_id = agent_id_clone.clone();
        eprintln!("[subagent] Starting event forwarder for '{}'", fwd_agent_id);
        tokio::spawn(async move {
            let mut event_count = 0u64;
            while let Some(event) = child_event_rx.recv().await {
                event_count += 1;
                if event_count <= 5 || event_count % 50 == 0 {
                    eprintln!("[subagent] '{}' event #{}: {:?}", fwd_agent_id, event_count,
                        match &event {
                            TaskEvent::TextDelta { .. } => "TextDelta",
                            TaskEvent::ThinkingDelta { .. } => "ThinkingDelta",
                            TaskEvent::ToolUse { tool_name, .. } => tool_name.as_str(),
                            TaskEvent::ToolResult { .. } => "ToolResult",
                            TaskEvent::CostUpdate { .. } => "CostUpdate",
                            TaskEvent::StatusChange { .. } => "StatusChange",
                            _ => "Other",
                        }
                    );
                }
                match event {
                    TaskEvent::TextDelta { text, .. } => {
                        let _ = fwd_parent_tx.send(TaskEvent::SubagentTextDelta {
                            task_id: fwd_task_id.clone(),
                            agent_id: fwd_agent_id.clone(),
                            text,
                        });
                    }
                    TaskEvent::ThinkingDelta { text, .. } => {
                        let _ = fwd_parent_tx.send(TaskEvent::SubagentTextDelta {
                            task_id: fwd_task_id.clone(),
                            agent_id: fwd_agent_id.clone(),
                            text: format!("[thinking] {}", text),
                        });
                    }
                    TaskEvent::ToolUse { tool_name, .. } => {
                        let _ = fwd_parent_tx.send(TaskEvent::SubagentTextDelta {
                            task_id: fwd_task_id.clone(),
                            agent_id: fwd_agent_id.clone(),
                            text: format!("\n[tool: {}]\n", tool_name),
                        });
                    }
                    TaskEvent::CostUpdate { cost, .. } => {
                        let _ = fwd_parent_tx.send(TaskEvent::SubagentCostUpdate {
                            task_id: fwd_task_id.clone(),
                            agent_id: fwd_agent_id.clone(),
                            cost,
                        });
                    }
                    // Forward permission requests to parent so the user sees popups
                    // for sub-agent operations. We use the parent's task_id so the
                    // UI can match them to the active task, and prefix the description
                    // with the sub-agent name so the user knows which agent is asking.
                    TaskEvent::PermissionRequest { request_id, operation, description, preview, .. } => {
                        let _ = fwd_parent_tx.send(TaskEvent::PermissionRequest {
                            task_id: fwd_task_id.clone(),
                            request_id,
                            operation,
                            description: format!("[Sub-agent '{}'] {}", fwd_agent_id, description),
                            preview,
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
            file_lock: child_file_lock,
            file_read_registry: std::sync::Arc::new(crate::tools::FileReadRegistry::new()),
            mcp_manager: child_mcp_manager,
            mcp_tool_defs: child_mcp_tool_defs,
            subagent_registry: child_subagent_registry,
            agent_depth: 1,
            ai_config: child_ai_config,
            tool_config: child_tool_config,
            allowed_paths: child_allowed_paths,
            question_broker: child_question_broker,
            parent_provider_config: None, // sub-agents cannot spawn further sub-agents
            completion_summary: Arc::new(std::sync::Mutex::new(None)),
            write_scope: Some(child_write_scope),
            blocked_writes: child_blocked_writes,
            agent_terminals: child_agent_terminals,
            is_global: child_is_global,
            orchestrator_host: child_orchestrator_host,
        };

        let executor = TaskExecutor::new(provider, sub_config);
        let mut messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text { text: prompt }],
        }];

        eprintln!("[subagent] '{}' starting run_turn...", agent_id_clone);
        let result = executor.run_turn(&mut messages, &child_context).await;
        eprintln!("[subagent] '{}' run_turn finished: {}", agent_id_clone, if result.is_ok() { "OK" } else { "ERROR" });

        let (summary, diff) = match result {
            Ok(_) => {
                let diff = child_context.compute_diff_fn
                    .as_ref()
                    .map(|f| f())
                    .unwrap_or_else(|| crate::checkpoint::TaskDiff {
                        files: Vec::new(),
                        total_insertions: 0,
                        total_deletions: 0,
                    });
                // Prefer the explicit summary from `complete_task` when the
                // sub-agent called it. Falls back to the last assistant text
                // block for compatibility when the model ends via plain text.
                let explicit = child_context
                    .completion_summary
                    .lock()
                    .ok()
                    .and_then(|s| s.clone());
                let summary = explicit.unwrap_or_else(|| {
                    messages.iter().rev()
                        .find(|m| matches!(m.role, Role::Assistant))
                        .and_then(|m| m.content.iter().find_map(|b| {
                            if let ContentBlock::Text { text } = b { Some(text.clone()) } else { None }
                        }))
                        .unwrap_or_else(|| "Sub-agent completed.".to_string())
                });
                // Cap runaway summaries; complete_task is expected to stay concise.
                let summary = if summary.len() > 2000 {
                    format!("{}…", &summary[..2000])
                } else {
                    summary
                };
                (summary, diff)
            }
            Err(e) => {
                let err = format!("Sub-agent error: {}", e);
                eprintln!("[subagent] '{}' FAILED: {}", agent_id_clone, err);
                registry.fail(&parent_task_id, &agent_id_clone, err.clone());
                let _ = parent_event_tx.send(TaskEvent::SubagentFailed {
                    task_id: parent_task_id.clone(),
                    agent_id: agent_id_clone.clone(),
                    error: err,
                });
                return;
            }
        };

        // Drain the sub-agent's recorded blocked writes — this is what the
        // orchestrator will use to decide how to recover from out-of-scope writes.
        let blocked_on = blocked_writes_for_result
            .lock()
            .map(|mut v| std::mem::take(&mut *v))
            .unwrap_or_default();

        let sub_result = SubagentResult {
            agent_id: agent_id_clone.clone(),
            model: String::new(),
            summary: summary.clone(),
            notes: None,
            diff,
            blocked_on,
        };
        eprintln!("[subagent] '{}' completed successfully, summary len={}", agent_id_clone, summary.len());
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
    let mut lines: Vec<String> = agents.iter().map(|a| {
        let status = match a.status {
            crate::task::subagent::SubagentStatus::Running => "Running",
            crate::task::subagent::SubagentStatus::Completed => "Completed",
            crate::task::subagent::SubagentStatus::Failed => "Failed",
        };
        format!("- {} [{}] — {}", a.agent_id, a.model, status)
    }).collect();
    let running_count = agents.iter().filter(|a| a.status == crate::task::subagent::SubagentStatus::Running).count();
    if running_count > 0 {
        lines.push(format!(
            "\n{} sub-agent(s) still running. Call `wait_for_subagents` to block until one finishes.",
            running_count
        ));
    }
    Ok(ToolOutput {
        content: lines.join("\n"),
        is_error: false,
    })
}

async fn wait_for_subagents(context: &ToolContext) -> Result<ToolOutput> {
    let active = context.subagent_registry.active_for_task(&context.task_id);
    if active.is_empty() {
        // No running sub-agents — check if any completed results are pending in the queue
        let agents = context.subagent_registry.all_for_task(&context.task_id);
        if agents.is_empty() {
            return Ok(ToolOutput {
                content: "No sub-agents for this task.".to_string(),
                is_error: false,
            });
        }
        return Ok(ToolOutput {
            content: "All sub-agents have already finished.".to_string(),
            is_error: false,
        });
    }

    // Block until one sub-agent completes or fails
    match context.subagent_registry.wait_for_any(&context.task_id).await {
        None => Ok(ToolOutput {
            content: "All sub-agents have finished.".to_string(),
            is_error: false,
        }),
        Some(crate::task::subagent::SubagentCompletionEvent::Completed(result)) => {
            let still_active = context.subagent_registry.active_for_task(&context.task_id);
            let mut output = result.format_completion_block();
            if still_active.is_empty() {
                output.push_str("\n\n[All sub-agents have finished]");
            } else {
                let names: Vec<String> = still_active.iter().map(|a| a.agent_id.clone()).collect();
                output.push_str(&format!(
                    "\n\n[{} still running: {}] — call `wait_for_subagents` again to wait for the next one.",
                    still_active.len(),
                    names.join(", ")
                ));
            }
            Ok(ToolOutput {
                content: output,
                is_error: false,
            })
        }
        Some(crate::task::subagent::SubagentCompletionEvent::Failed { agent_id, error }) => {
            let still_active = context.subagent_registry.active_for_task(&context.task_id);
            let mut output = format!(
                "[Sub-agent '{}' FAILED: {}]",
                agent_id, error
            );
            if still_active.is_empty() {
                output.push_str("\n\n[All sub-agents have finished]");
            } else {
                let names: Vec<String> = still_active.iter().map(|a| a.agent_id.clone()).collect();
                output.push_str(&format!(
                    "\n\n[{} still running: {}] — call `wait_for_subagents` again to wait for the next one.",
                    still_active.len(),
                    names.join(", ")
                ));
            }
            Ok(ToolOutput {
                content: output,
                is_error: false,
            })
        }
    }
}

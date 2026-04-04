use crate::checkpoint::TaskDiff;
use crate::provider::{AiProvider, ContentBlock, Message, ProviderConfig, ProviderStreamEvent, Role, StreamCallback};
use crate::task::condense;
use crate::task::cost::TaskCost;
use crate::task::{TaskEvent, TaskStatus};
use crate::tools::{BuiltinTools, ToolContext, ToolExecutor, ToolOutput};
use anyhow::Result;
use futures::future::join_all;
use std::sync::atomic::Ordering;
use std::sync::Arc;

pub struct TaskExecutor {
    provider: Arc<dyn AiProvider>,
    tools: BuiltinTools,
    config: ProviderConfig,
}

impl TaskExecutor {
    pub fn new(
        provider: Arc<dyn AiProvider>,
        config: ProviderConfig,
    ) -> Self {
        Self {
            provider,
            tools: BuiltinTools::new(),
            config,
        }
    }

    /// Run one turn of the agentic loop: send messages, handle tool calls, repeat until text-only response.
    pub async fn run_turn(
        &self,
        messages: &mut Vec<Message>,
        context: &ToolContext,
    ) -> Result<()> {
        // Combine builtin tool defs with pre-fetched MCP tool defs
        let mut tool_defs = self.tools.definitions();
        tool_defs.extend(context.mcp_tool_defs.clone());
        let task_id = &context.task_id;
        let event_tx = &context.event_tx;
        let model = &self.config.model;
        let mut task_cost = TaskCost::default();

        loop {
            // Check cancellation before every provider call
            if let Some(token) = &context.cancel_token {
                if token.load(Ordering::SeqCst) {
                    let _ = event_tx.send(TaskEvent::StatusChange {
                        task_id: task_id.clone(),
                        status: TaskStatus::Cancelled,
                    });
                    break;
                }
            }

            // Check and increment turn budget
            let turns_remaining = {
                let mut b = context.turn_budget.lock().unwrap();
                if b.used >= b.max {
                    let _ = event_tx.send(TaskEvent::TurnBudgetWarning {
                        task_id: task_id.clone(),
                        turns_remaining: 0,
                    });
                    let _ = event_tx.send(TaskEvent::StatusChange {
                        task_id: task_id.clone(),
                        status: TaskStatus::TurnLimitReached,
                    });
                    break;
                }
                b.used += 1;
                b.max - b.used
            };

            // Strip UI-only ModelSwitch markers and redact thinking text before sending to the API.
            // Thinking blocks must be echoed back with their signature for the API to accept them,
            // but the thinking text itself can be cleared to avoid bloating context.
            // Also remove any messages that become empty after stripping.
            let api_messages: Vec<Message> = messages
                .iter()
                .map(|msg| Message {
                    role: msg.role.clone(),
                    content: msg
                        .content
                        .iter()
                        .filter(|b| !matches!(b, ContentBlock::ModelSwitch { .. }))
                        .map(|b| match b {
                            ContentBlock::Thinking { signature, .. } => ContentBlock::Thinking {
                                thinking: String::new(),
                                signature: signature.clone(),
                            },
                            other => other.clone(),
                        })
                        .collect(),
                })
                .filter(|msg| !msg.content.is_empty())
                .collect();

            // Build streaming callback that forwards token deltas to the event channel
            let stream_task_id = task_id.clone();
            let stream_event_tx = event_tx.clone();
            let stream_cb: StreamCallback = Arc::new(move |event| {
                match event {
                    ProviderStreamEvent::TextDelta(text) => {
                        let _ = stream_event_tx.send(TaskEvent::TextDelta {
                            task_id: stream_task_id.clone(),
                            text,
                        });
                    }
                    ProviderStreamEvent::ThinkingDelta(text) => {
                        let _ = stream_event_tx.send(TaskEvent::ThinkingDelta {
                            task_id: stream_task_id.clone(),
                            text,
                        });
                    }
                }
            });

            // Send to provider (streaming)
            let response = match self
                .provider
                .chat(api_messages, tool_defs.clone(), &self.config, Some(stream_cb))
                .await
            {
                Ok(resp) => resp,
                Err(e) if e.to_string().contains("Task cancelled") => {
                    let _ = event_tx.send(TaskEvent::StatusChange {
                        task_id: task_id.clone(),
                        status: TaskStatus::Cancelled,
                    });
                    return Ok(());
                }
                Err(e) => return Err(e),
            };

            // Accumulate cost and emit update
            task_cost.add_turn(model, &response.usage);
            let _ = event_tx.send(TaskEvent::CostUpdate {
                task_id: task_id.clone(),
                cost: task_cost.clone(),
            });

            // Build assistant message from response
            let assistant_msg = Message {
                role: Role::Assistant,
                content: response.content.clone(),
            };
            messages.push(assistant_msg.clone());

            let _ = event_tx.send(TaskEvent::MessageComplete {
                task_id: task_id.clone(),
                message: assistant_msg,
            });

            // Check if tool use is needed
            let tool_uses: Vec<_> = response
                .content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::ToolUse { id, name, input } => {
                        Some((id.clone(), name.clone(), input.clone()))
                    }
                    _ => None,
                })
                .collect();

            if tool_uses.is_empty() {
                // Check for active sub-agents before breaking
                let active = context.subagent_registry.active_for_task(task_id);
                if active.is_empty() {
                    break; // No tool calls and no sub-agents — turn complete
                }

                // Wait for any sub-agent to finish, then inject result and loop
                match context.subagent_registry.wait_for_any(task_id).await {
                    None => break, // No more agents
                    Some(crate::task::subagent::SubagentCompletionEvent::Completed(result)) => {
                        let still_active = context.subagent_registry.active_for_task(task_id);
                        let still_running_list: Vec<String> = still_active.iter().map(|a| a.agent_id.clone()).collect();

                        let injection = if still_running_list.is_empty() {
                            format!(
                                "[Sub-agent '{}' completed]\n{}\n[All sub-agents have finished]",
                                result.agent_id, result.summary
                            )
                        } else {
                            format!(
                                "[Sub-agent '{}' completed]\n{}\n[{} still running: {}]",
                                result.agent_id,
                                result.summary,
                                still_running_list.len(),
                                still_running_list.join(", ")
                            )
                        };

                        messages.push(Message {
                            role: Role::User,
                            content: vec![ContentBlock::Text { text: injection }],
                        });
                        // Loop back — main model processes the result
                    }
                    Some(crate::task::subagent::SubagentCompletionEvent::Failed { agent_id, error }) => {
                        let still_active = context.subagent_registry.active_for_task(task_id);
                        let still_running_list: Vec<String> = still_active.iter().map(|a| a.agent_id.clone()).collect();

                        let injection = if still_running_list.is_empty() {
                            format!(
                                "[Sub-agent '{}' FAILED: {}]\n[All sub-agents have finished]",
                                agent_id, error
                            )
                        } else {
                            format!(
                                "[Sub-agent '{}' FAILED: {}]\n[{} still running: {}]",
                                agent_id,
                                error,
                                still_running_list.len(),
                                still_running_list.join(", ")
                            )
                        };

                        messages.push(Message {
                            role: Role::User,
                            content: vec![ContentBlock::Text { text: injection }],
                        });
                    }
                }
                continue; // Loop back for next provider call
            }

            // Check for task_complete before executing tools
            let mut task_completed = false;
            for (_, tool_name, tool_input) in &tool_uses {
                if tool_name == "task_complete" {
                    let summary = tool_input
                        .get("summary")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let notes = tool_input
                        .get("notes")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());

                    let diff = context
                        .compute_diff_fn
                        .as_ref()
                        .map(|f| f())
                        .unwrap_or_else(|| TaskDiff {
                            files: Vec::new(),
                            total_insertions: 0,
                            total_deletions: 0,
                        });

                    let _ = event_tx.send(TaskEvent::TaskComplete {
                        task_id: task_id.clone(),
                        summary,
                        notes,
                        diff,
                    });
                    task_completed = true;
                    break;
                }
            }

            if task_completed {
                break;
            }

            // Check cancellation once before executing the tool batch
            if let Some(token) = &context.cancel_token {
                if token.load(Ordering::SeqCst) {
                    let _ = event_tx.send(TaskEvent::StatusChange {
                        task_id: task_id.clone(),
                        status: TaskStatus::Cancelled,
                    });
                    return Ok(());
                }
            }

            // Emit all ToolUse events upfront so the UI shows them immediately
            for (tool_id, tool_name, tool_input) in &tool_uses {
                let _ = event_tx.send(TaskEvent::ToolUse {
                    task_id: task_id.clone(),
                    tool_use_id: tool_id.clone(),
                    tool_name: tool_name.clone(),
                    tool_input: tool_input.clone(),
                });
            }

            // Give the frontend time to render the "pending" tool cards before results arrive.
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;

            // Smart concurrency: read-only tools run in parallel, write tools run sequentially after.
            // This prevents race conditions on writes while maximizing throughput for reads.
            let (read_only_tools, write_tools): (Vec<_>, Vec<_>) = tool_uses
                .iter()
                .partition(|(_, name, _)| BuiltinTools::is_read_only(name));

            let mut results: Vec<(String, ToolOutput)> = Vec::new();

            // Phase 1: Execute all read-only tools in parallel
            if !read_only_tools.is_empty() {
                let read_futures: Vec<_> = read_only_tools
                    .iter()
                    .map(|(tool_id, tool_name, tool_input)| {
                        let tool_id = tool_id.clone();
                        let tool_name = tool_name.clone();
                        let tool_input = tool_input.clone();
                        async move {
                            let result = if BuiltinTools::is_builtin(&tool_name) {
                                self.tools
                                    .execute(&tool_name, &tool_id, tool_input, context)
                                    .await
                                    .unwrap_or_else(|e| ToolOutput {
                                        content: format!("Tool error: {}", e),
                                        is_error: true,
                                    })
                            } else if let Some(mcp) = &context.mcp_manager {
                                let mcp_clone = Arc::clone(mcp);
                                let name = tool_name.clone();
                                match tokio::task::spawn_blocking(move || {
                                    mcp_clone.lock().unwrap().call_tool(&name, tool_input)
                                })
                                .await
                                {
                                    Ok(Ok(val)) => ToolOutput {
                                        content: val.to_string(),
                                        is_error: false,
                                    },
                                    Ok(Err(e)) => ToolOutput {
                                        content: format!("MCP tool error: {}", e),
                                        is_error: true,
                                    },
                                    Err(e) => ToolOutput {
                                        content: format!("MCP call panicked: {}", e),
                                        is_error: true,
                                    },
                                }
                            } else {
                                ToolOutput {
                                    content: format!("Unknown tool: {}", tool_name),
                                    is_error: true,
                                }
                            };
                            (tool_id, result)
                        }
                    })
                    .collect();

                results.extend(join_all(read_futures).await);
            }

            // Phase 2: Execute write/execute tools sequentially
            for (tool_id, tool_name, tool_input) in &write_tools {
                let result = if BuiltinTools::is_builtin(tool_name) {
                    self.tools
                        .execute(tool_name, tool_id, tool_input.clone(), context)
                        .await
                        .unwrap_or_else(|e| ToolOutput {
                            content: format!("Tool error: {}", e),
                            is_error: true,
                        })
                } else if let Some(mcp) = &context.mcp_manager {
                    let mcp_clone = Arc::clone(mcp);
                    let name = tool_name.clone();
                    let input = tool_input.clone();
                    match tokio::task::spawn_blocking(move || {
                        mcp_clone.lock().unwrap().call_tool(&name, input)
                    })
                    .await
                    {
                        Ok(Ok(val)) => ToolOutput {
                            content: val.to_string(),
                            is_error: false,
                        },
                        Ok(Err(e)) => ToolOutput {
                            content: format!("MCP tool error: {}", e),
                            is_error: true,
                        },
                        Err(e) => ToolOutput {
                            content: format!("MCP call panicked: {}", e),
                            is_error: true,
                        },
                    }
                } else {
                    ToolOutput {
                        content: format!("Unknown tool: {}", tool_name),
                        is_error: true,
                    }
                };
                results.push((tool_id.clone(), result));
            }

            // Emit results and collect for the next message.
            // Large results are budgeted: the UI gets the full output, but the API
            // context gets a truncated preview to save tokens.
            const MAX_RESULT_CHARS: usize = 50_000;
            const PREVIEW_CHARS: usize = 2_000;

            let mut tool_results = Vec::new();
            for (tool_id, result) in results {
                // UI always gets the full result
                let _ = event_tx.send(TaskEvent::ToolResult {
                    task_id: task_id.clone(),
                    tool_use_id: tool_id.clone(),
                    output: result.content.clone(),
                    is_error: result.is_error,
                });

                // Budget: if the result is too large, truncate what goes into the API context
                let api_content = if result.content.len() > MAX_RESULT_CHARS {
                    let preview_end = result
                        .content
                        .char_indices()
                        .nth(PREVIEW_CHARS)
                        .map(|(i, _)| i)
                        .unwrap_or(result.content.len());
                    format!(
                        "{}\n\n[... truncated — full output was {} chars. Only the first {} chars are shown to save context.]",
                        &result.content[..preview_end],
                        result.content.len(),
                        PREVIEW_CHARS
                    )
                } else {
                    result.content
                };

                tool_results.push(ContentBlock::ToolResult {
                    tool_use_id: tool_id,
                    content: api_content,
                    is_error: result.is_error,
                });
            }

            // Inject turn budget warning into tool results message when 5 turns remain.
            // The model sees this on the next turn and begins wrapping up.
            if turns_remaining == 5 {
                tool_results.push(ContentBlock::Text {
                    text: "[Turn budget: 5 turns remaining — please wrap up and call task_complete soon.]"
                        .to_string(),
                });
                let _ = event_tx.send(TaskEvent::TurnBudgetWarning {
                    task_id: task_id.clone(),
                    turns_remaining: 5,
                });
            }

            // Add tool results as a user message (Claude expects this)
            messages.push(Message {
                role: Role::User,
                content: tool_results,
            });

            // Check if context needs condensing before the next provider call
            if self.config.context_window > 0
                && condense::should_condense(
                    response.usage.input_tokens,
                    self.config.context_window,
                    self.config.max_tokens,
                    self.config.thinking_budget,
                )
            {
                let _ = event_tx.send(TaskEvent::ContextCondenseStarted {
                    task_id: task_id.clone(),
                });
                let original_count = messages.len() as u32;

                match condense::condense_context(&self.provider, &self.config, messages).await {
                    Ok(condensed) => {
                        *messages = condensed;
                    }
                    Err(_e) => {
                        *messages = condense::sliding_window_fallback(messages);
                    }
                }

                let _ = event_tx.send(TaskEvent::ContextCondenseCompleted {
                    task_id: task_id.clone(),
                    original_messages: original_count,
                    condensed_to: messages.len() as u32,
                });
            }

            // Loop back for next provider call
        }

        Ok(())
    }
}

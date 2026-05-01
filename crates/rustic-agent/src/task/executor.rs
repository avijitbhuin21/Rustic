use crate::checkpoint::TaskDiff;
use crate::provider::{AiProvider, ContentBlock, Message, ProviderConfig, ProviderStreamEvent, Role, StopReason, StreamCallback};
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
    ) -> Result<TaskCost> {
        // Combine builtin tool defs with pre-fetched MCP tool defs.
        // web_search / web_fetch are conditional on the user's tool config
        // (and on whether an MCP server is providing them instead), so they
        // come from a separate entry point rather than being unconditionally
        // included in `BuiltinTools::definitions()`.
        //
        // Pass the host's actual shell list so `run_command`'s `shell` enum
        // only advertises interpreters that will successfully spawn.
        let available_shells = context
            .agent_terminals
            .as_ref()
            .map(|b| b.available_shells())
            .unwrap_or_default();
        let mut tool_defs = self.tools.definitions_for_host(&available_shells);
        tool_defs.extend(crate::tools::web_tools::definitions_for(&context.tool_config));
        tool_defs.extend(context.mcp_tool_defs.clone());
        let task_id = &context.task_id;
        let event_tx = &context.event_tx;
        let model = &self.config.model;
        let mut task_cost = TaskCost::default();
        // Tracks the input token count from the most recent API response.
        // Used to decide whether to condense before the next API call.
        // Starts at 0 (unknown); on the first call we fall back to a size estimate.
        let mut last_input_tokens: u32 = 0;
        // Set to true right after condensing so we skip the check on the very next
        // iteration and avoid an infinite condense loop.
        let mut just_condensed = false;
        // Buffer of streaming assistant text for the current iteration. Lets
        // the cancel branch persist whatever the model said before the user
        // hit "Stop & send" — without this, the partial response is shown to
        // the user via deltas but discarded from `messages`, so the next turn
        // starts as if the model never spoke (plan §B.2).
        //
        // Reset at the top of every iteration so each provider.chat call has
        // its own buffer; only the in-flight iteration's text is partial — all
        // earlier iterations' content was already appended via line ~425 on
        // their successful return.
        let partial_assistant_text: Arc<std::sync::Mutex<String>> =
            Arc::new(std::sync::Mutex::new(String::new()));

        loop {
            if let Ok(mut buf) = partial_assistant_text.lock() {
                buf.clear();
            }
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

            // ── Inject pending background-terminal exit notifications ─────
            // A background pty that ended since the last turn (server crashed,
            // dev command completed, etc.) shows up here as a synthetic user
            // message. Appended before the condense check so it counts toward
            // the token estimate and the model sees it on the next provider call.
            if let Some(broker) = context.agent_terminals.as_ref() {
                let exits = broker.drain_pending_exits(task_id);
                if !exits.is_empty() {
                    let mut body = String::from(
                        "SYSTEM: one or more background terminals you started have exited. \
                         Review the output below and decide whether to restart, fix a bug, \
                         or proceed — the shell process is no longer running.\n",
                    );
                    for exit in &exits {
                        body.push_str(&format!("\n── Terminal #{} ({})", exit.session_id, exit.label));
                        if let Some(cmd) = &exit.last_command {
                            body.push_str(&format!("\nLast command: {}", cmd));
                        }
                        body.push_str("\nFinal output (tail):\n");
                        body.push_str("```\n");
                        if exit.output_tail.trim().is_empty() {
                            body.push_str("(no output)\n");
                        } else {
                            body.push_str(exit.output_tail.trim_end());
                            body.push('\n');
                        }
                        body.push_str("```\n");
                    }
                    messages.push(Message {
                        role: Role::User,
                        content: vec![ContentBlock::Text { text: body }],
                    });
                }
            }

            // ── Pre-call context condense check ───────────────────────────────────
            // Runs before EVERY API call — including mid-task tool-call iterations —
            // so the context is always trimmed before it hits the provider limit.
            //
            // Order matters: this must come BEFORE the turn-budget increment so that
            // a condense iteration does not consume a turn slot.
            //
            // Token count source:
            //   • `last_input_tokens > 0` → real value from the previous API response
            //     (accurate, updated every iteration after the first)
            //   • otherwise → rough char ÷ 4 estimate for the very first call
            if !just_condensed && self.config.context_window > 0 {
                // System prompt + tool definitions are sent on every request
                // and must be counted toward the context limit, even though
                // after the first call they are served from cache.
                let system_prompt_tokens = self
                    .config
                    .system_prompt
                    .as_deref()
                    .map(|s| (s.len() / 4) as u32)
                    .unwrap_or(0);
                let tool_defs_tokens: u32 = {
                    let total_chars: usize = tool_defs
                        .iter()
                        .map(|t| {
                            t.name.len()
                                + t.description.len()
                                + serde_json::to_string(&t.parameters)
                                    .map(|s| s.len())
                                    .unwrap_or(400)
                        })
                        .sum();
                    (total_chars / 4) as u32
                };

                let estimated_tokens = if last_input_tokens > 0 {
                    // `last_input_tokens` already reflects the real prior request size
                    // (including system/tools), so don't double-add those.
                    last_input_tokens
                } else {
                    let total_chars: usize = messages
                        .iter()
                        .flat_map(|m| m.content.iter())
                        .map(|b| match b {
                            ContentBlock::Text { text } => text.len(),
                            ContentBlock::ToolResult { content, .. } => content.len(),
                            ContentBlock::ToolUse { input, .. } => {
                                serde_json::to_string(input).map(|s| s.len()).unwrap_or(200)
                            }
                            // Images are ~1600 tokens for a typical image; approximate by data length / 4
                            ContentBlock::Image { data, .. } => data.len() / 4,
                            _ => 0,
                        })
                        .sum();
                    (total_chars / 4) as u32 + system_prompt_tokens + tool_defs_tokens
                };

                if condense::should_condense(
                    estimated_tokens,
                    self.config.context_window,
                    self.config.max_tokens,
                    self.config.thinking_budget,
                ) {
                    tracing::warn!(
                        "[executor] '{}' pre-call condense triggered (estimated_tokens={}, context_window={})",
                        task_id, estimated_tokens, self.config.context_window
                    );
                    let _ = event_tx.send(TaskEvent::ContextCondenseStarted {
                        task_id: task_id.clone(),
                    });
                    let original_count = messages.len() as u32;

                    match condense::condense_context(&self.provider, &self.config, messages).await {
                        Ok((condensed, condense_usage)) => {
                            *messages = condensed;
                            // Include the condensing call's tokens in the task cost
                            task_cost.add_turn(model, &condense_usage);
                            let _ = event_tx.send(TaskEvent::CostUpdate {
                                task_id: task_id.clone(),
                                cost: task_cost.clone(),
                            });
                        }
                        Err(e) => {
                            tracing::warn!("[executor] condense failed ({}), using sliding window", e);
                            *messages = condense::sliding_window_fallback(messages);
                        }
                    }

                    let _ = event_tx.send(TaskEvent::ContextCondenseCompleted {
                        task_id: task_id.clone(),
                        original_messages: original_count,
                        condensed_to: messages.len() as u32,
                    });

                    last_input_tokens = 0;
                    just_condensed = true;
                    continue; // Re-enter loop — api_messages rebuilt from condensed history,
                              // turn budget NOT yet incremented (no API call was made)
                }
            }
            just_condensed = false;

            // Strip UI-only ModelSwitch markers and redact thinking text before sending to the API.
            // Thinking blocks must be echoed back with their signature for the API to accept them,
            // but the thinking text itself can be cleared to avoid bloating context.
            // Also shrink older tool_result bodies via two strategies, applied together:
            //   1. Path/identity dedup: when the same read_file/grep/list_directory
            //      targets the same path/pattern multiple times, only the NEWEST
            //      occurrence keeps its full body; earlier ones are stubbed.
            //   2. Positional aging: the last AGE_KEEP_FULL tool_results keep their
            //      full body regardless; older ones get shrunk.
            // Either condition triggers the shrink.
            const AGE_KEEP_FULL: usize = 6;
            const AGED_PREVIEW_CHARS: usize = 300;

            // --- Build the set of tool_use_ids that should be shrunk by path dedup.
            // Walk forward, tracking the most recent tool_use_id for each
            // (tool_name, identity) key. When a key reappears, the previous id
            // goes into the shrink set.
            let mut shrink_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
            {
                let mut latest_for_key: std::collections::HashMap<String, String> =
                    std::collections::HashMap::new();
                for msg in messages.iter() {
                    for block in msg.content.iter() {
                        if let ContentBlock::ToolUse { id, name, input, .. } = block {
                            if let Some(key) = dedup_key_for_tool(name, input) {
                                let full_key = format!("{}::{}", name, key);
                                if let Some(prev_id) = latest_for_key.insert(full_key, id.clone()) {
                                    shrink_ids.insert(prev_id);
                                }
                            }
                        }
                    }
                }
            }

            let total_tool_results: usize = messages
                .iter()
                .flat_map(|m| m.content.iter())
                .filter(|b| matches!(b, ContentBlock::ToolResult { .. }))
                .count();
            let age_cutoff = total_tool_results.saturating_sub(AGE_KEEP_FULL);
            let mut seen_results: usize = 0;

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
                                duration_secs: None,
                            },
                            ContentBlock::ToolResult { tool_use_id, content, is_error } => {
                                let idx = seen_results;
                                seen_results += 1;
                                let aged_out = idx < age_cutoff;
                                let superseded = shrink_ids.contains(tool_use_id);
                                let should_shrink = (aged_out || superseded) && content.len() > AGED_PREVIEW_CHARS;
                                if should_shrink {
                                    let preview_end = content
                                        .char_indices()
                                        .nth(AGED_PREVIEW_CHARS)
                                        .map(|(i, _)| i)
                                        .unwrap_or(content.len());
                                    let reason = if superseded {
                                        "superseded by a later call with the same arguments"
                                    } else {
                                        "aged out from an earlier turn"
                                    };
                                    ContentBlock::ToolResult {
                                        tool_use_id: tool_use_id.clone(),
                                        content: format!(
                                            "{}\n\n[... {} — {} chars total. If you need the full result, re-run the tool.]",
                                            &content[..preview_end],
                                            reason,
                                            content.len()
                                        ),
                                        is_error: *is_error,
                                    }
                                } else {
                                    ContentBlock::ToolResult {
                                        tool_use_id: tool_use_id.clone(),
                                        content: content.clone(),
                                        is_error: *is_error,
                                    }
                                }
                            }
                            other => other.clone(),
                        })
                        .collect(),
                })
                .filter(|msg| !msg.content.is_empty())
                .collect();

            // Build streaming callback that forwards token deltas to the event channel
            let stream_task_id = task_id.clone();
            let stream_event_tx = event_tx.clone();
            let partial_for_cb = Arc::clone(&partial_assistant_text);
            let stream_cb: StreamCallback = Arc::new(move |event| {
                match event {
                    ProviderStreamEvent::TextDelta(text) => {
                        // Mirror the delta into the partial-message buffer so
                        // the cancel branch below can recover whatever the
                        // model said before the user aborted. Borrow first,
                        // then move `text` into the forwarded event.
                        if let Ok(mut buf) = partial_for_cb.lock() {
                            buf.push_str(&text);
                        }
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
                    ProviderStreamEvent::ServerToolUse { id, name, input } => {
                        // Inline emission — UI renders the tool card between
                        // text deltas, in the order the model emitted it.
                        let _ = stream_event_tx.send(TaskEvent::ToolUse {
                            task_id: stream_task_id.clone(),
                            tool_use_id: id,
                            tool_name: name,
                            tool_input: input,
                        });
                    }
                    ProviderStreamEvent::ServerToolResult { tool_use_id, content, is_error } => {
                        let _ = stream_event_tx.send(TaskEvent::ToolResult {
                            task_id: stream_task_id.clone(),
                            tool_use_id,
                            output: content,
                            is_error,
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
                    // The user aborted mid-stream (typically via "Stop & send"
                    // — plan §14 / §B.2). Persist whatever text the model had
                    // already streamed so the conversation history matches
                    // what the user just saw on screen, and so the queued
                    // follow-up message lands in a coherent context. Tool
                    // calls from this iteration are intentionally discarded:
                    // their JSON inputs are usually incomplete at cancel
                    // time, and re-feeding them as if executed would confuse
                    // the model on the next turn.
                    let partial = partial_assistant_text
                        .lock()
                        .ok()
                        .map(|s| s.clone())
                        .unwrap_or_default();
                    if !partial.is_empty() {
                        let assistant_msg = Message {
                            role: Role::Assistant,
                            content: vec![ContentBlock::Text { text: partial }],
                        };
                        messages.push(assistant_msg.clone());
                        let _ = event_tx.send(TaskEvent::MessageComplete {
                            task_id: task_id.clone(),
                            message: assistant_msg,
                        });
                    }
                    let _ = event_tx.send(TaskEvent::StatusChange {
                        task_id: task_id.clone(),
                        status: TaskStatus::Cancelled,
                    });
                    return Ok(task_cost);
                }
                Err(e) => return Err(e),
            };

            // Accumulate cost and emit update
            task_cost.add_turn(model, &response.usage);
            // Track total input going into the next call — including cache reads,
            // since cached tokens still count toward the context window limit
            // even though they're billed at 10% of the input rate.
            last_input_tokens = response
                .usage
                .input_tokens
                .saturating_add(response.usage.cache_read_tokens)
                .saturating_add(response.usage.cache_write_tokens);
            tracing::warn!(
                "[executor] '{}' turn complete: in={} out={} cache_read={} cache_write={} stop={:?} blocks={}",
                task_id,
                response.usage.input_tokens,
                response.usage.output_tokens,
                response.usage.cache_read_tokens,
                response.usage.cache_write_tokens,
                response.stop_reason,
                response.content.len()
            );
            let _ = event_tx.send(TaskEvent::RequestUsage {
                task_id: task_id.clone(),
                input_tokens: response.usage.input_tokens,
                output_tokens: response.usage.output_tokens,
                cache_read_tokens: response.usage.cache_read_tokens,
                cache_write_tokens: response.usage.cache_write_tokens,
                cost_usd: crate::task::cost::calculate_cost(model, &response.usage),
            });
            let _ = event_tx.send(TaskEvent::CostUpdate {
                task_id: task_id.clone(),
                cost: task_cost.clone(),
            });

            // ── Handle max_tokens truncation ────────────────────────────────────
            // When the model hits the token limit mid-response, any in-progress
            // tool call JSON is incomplete and will fail to parse. Instead of
            // letting the model loop on cryptic errors, we strip the truncated
            // tool calls, keep any complete text/thinking blocks, and inject a
            // user message telling the model what happened so it can retry with
            // a different (smaller) strategy.
            let was_truncated = matches!(response.stop_reason, StopReason::MaxTokens);

            let response_content = if was_truncated {
                tracing::warn!("[executor] '{}' response truncated by max_tokens — stripping incomplete tool calls", task_id);
                // Keep text and thinking blocks; drop tool_use blocks since their
                // JSON is almost certainly incomplete.
                response.content.iter().filter(|b| {
                    !matches!(b, ContentBlock::ToolUse { .. })
                }).cloned().collect::<Vec<_>>()
            } else {
                response.content.clone()
            };

            // Build assistant message from response
            let assistant_msg = Message {
                role: Role::Assistant,
                content: response_content.clone(),
            };
            messages.push(assistant_msg.clone());

            let _ = event_tx.send(TaskEvent::MessageComplete {
                task_id: task_id.clone(),
                message: assistant_msg,
            });

            // If truncated, inject a guidance message and loop back so the model
            // can retry with a smaller approach (e.g. write a script, use multiple
            // edit_file calls, or split content into chunks).
            if was_truncated {
                let guidance = format!(
                    "[OUTPUT_TRUNCATED] Your response was cut off because it exceeded the \
                     max_tokens limit ({} tokens). Any tool calls in that response were lost.\n\n\
                     To proceed, use a strategy that produces smaller tool calls:\n\
                     - Use `run_command` to write file content via a shell script or heredoc\n\
                     - Split large files into smaller `edit_file` or `apply_patch` calls\n\
                     - Create a helper script that generates the content, then run it\n\n\
                     Do NOT retry the same large tool call — it will be truncated again.",
                    self.config.max_tokens
                );
                messages.push(Message {
                    role: Role::User,
                    content: vec![ContentBlock::Text { text: guidance }],
                });
                continue; // loop back for the model to retry with a different strategy
            }

            // Server-resolved tool pairs (e.g. Anthropic web_search/web_fetch):
            // the assistant response contains a ToolResult block for the
            // same id, which means the server executed the call inline. UI
            // events for these were already emitted during streaming by the
            // Claude SSE parser via ProviderStreamEvent::ServerToolUse /
            // ServerToolResult, so we only need the id set here to skip local
            // re-execution below.
            let server_resolved_ids: std::collections::HashSet<String> = response_content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.clone()),
                    _ => None,
                })
                .collect();

            // Check if tool use is needed. Skip ids whose server already
            // executed the call — their result is already in the response.
            let tool_uses: Vec<_> = response_content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::ToolUse { id, name, input, .. } => {
                        if server_resolved_ids.contains(id) {
                            None
                        } else {
                            Some((id.clone(), name.clone(), input.clone()))
                        }
                    }
                    _ => None,
                })
                .collect();

            if tool_uses.is_empty() {
                // Check for active sub-agents before breaking
                let active = context.subagent_registry.active_for_task(task_id);
                tracing::warn!("[executor] No tool calls from model. Active sub-agents: {} for task '{}'",
                    active.len(), task_id);
                if active.is_empty() {
                    tracing::warn!("[executor] No sub-agents running, ending turn for '{}'", task_id);
                    break; // No tool calls and no sub-agents — turn complete
                }

                tracing::warn!("[executor] Waiting for sub-agent completion (task '{}')", task_id);
                // Wait for any sub-agent to finish, then inject result and loop
                match context.subagent_registry.wait_for_any(task_id).await {
                    None => {
                        tracing::warn!("[executor] wait_for_any returned None, ending turn for '{}'", task_id);
                        break; // No more agents
                    }
                    Some(crate::task::subagent::SubagentCompletionEvent::Completed(result)) => {
                        let still_active = context.subagent_registry.active_for_task(task_id);
                        let still_running_list: Vec<String> = still_active.iter().map(|a| a.agent_id.clone()).collect();

                        let mut injection = result.format_completion_block();
                        if still_running_list.is_empty() {
                            injection.push_str("\n[All sub-agents have finished]");
                        } else {
                            injection.push_str(&format!(
                                "\n[{} still running: {}]",
                                still_running_list.len(),
                                still_running_list.join(", ")
                            ));
                        }

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

            // Check cancellation once before executing the tool batch
            if let Some(token) = &context.cancel_token {
                if token.load(Ordering::SeqCst) {
                    let _ = event_tx.send(TaskEvent::StatusChange {
                        task_id: task_id.clone(),
                        status: TaskStatus::Cancelled,
                    });
                    return Ok(task_cost);
                }
            }

            // Intercept tool calls with parse errors — return an immediate error
            // so the model knows its arguments were lost and can retry properly.
            let mut parse_error_results: Vec<(String, ToolOutput)> = Vec::new();
            let tool_uses: Vec<_> = tool_uses
                .into_iter()
                .filter(|(tool_id, tool_name, tool_input)| {
                    if let Some(err) = tool_input.get("__parse_error").and_then(|v| v.as_str()) {
                        tracing::warn!("[executor] Tool '{}' (id={}) has parse error, short-circuiting", tool_name, tool_id);
                        parse_error_results.push((
                            tool_id.clone(),
                            ToolOutput {
                                content: format!(
                                    "ARGUMENT_PARSE_ERROR: Your tool call for '{}' could not be executed \
                                     because the arguments failed to parse. {}\n\
                                     Please retry this tool call and make sure to include all required \
                                     parameters as valid JSON.",
                                    tool_name, err
                                ),
                                is_error: true,
                            },
                        ));
                        false // filter out — don't execute
                    } else {
                        true // keep for normal execution
                    }
                })
                .collect();

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
            // Include any parse-error results from the filter step above
            results.extend(parse_error_results);

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

            // Drain any sub-agent completions that arrived while the model was generating
            // or tools were executing. This avoids the model needing to poll with list_active_agents.
            let pending_events = context.subagent_registry.drain_pending(task_id);
            for event in pending_events {
                let injection = match event {
                    crate::task::subagent::SubagentCompletionEvent::Completed(result) => {
                        let still_active = context.subagent_registry.active_for_task(task_id);
                        let mut block = result.format_completion_block();
                        if still_active.is_empty() {
                            block.push_str("\n[All sub-agents have finished]");
                        } else {
                            let names: Vec<String> = still_active.iter().map(|a| a.agent_id.clone()).collect();
                            block.push_str(&format!(
                                "\n[{} still running: {}]",
                                still_active.len(),
                                names.join(", ")
                            ));
                        }
                        block
                    }
                    crate::task::subagent::SubagentCompletionEvent::Failed { agent_id, error } => {
                        let still_active = context.subagent_registry.active_for_task(task_id);
                        if still_active.is_empty() {
                            format!(
                                "[Sub-agent '{}' FAILED: {}]\n[All sub-agents have finished]",
                                agent_id, error
                            )
                        } else {
                            let names: Vec<String> = still_active.iter().map(|a| a.agent_id.clone()).collect();
                            format!(
                                "[Sub-agent '{}' FAILED: {}]\n[{} still running: {}]",
                                agent_id, error, still_active.len(), names.join(", ")
                            )
                        }
                    }
                };
                tool_results.push(ContentBlock::Text { text: injection });
            }

            // Add tool results as a user message (Claude expects this)
            messages.push(Message {
                role: Role::User,
                content: tool_results,
            });

            // If the model called `complete_task` during this batch, the tool
            // wrote its summary into the shared slot. Break out of the loop
            // after appending the tool_result (required by the API pairing
            // rule), injecting the summary as a final assistant message so the
            // user sees it verbatim.
            let completion = context.completion_summary.lock().ok().and_then(|s| s.clone());
            if let Some(summary) = completion {
                tracing::warn!("[executor] '{}' complete_task called — ending loop with summary ({} chars)",
                    task_id, summary.len());
                // Persist the summary in history as a regular assistant text
                // message so resuming the task / re-rendering history still has
                // the content. The frontend de-duplicates this against the
                // `summary` field on the TaskComplete event when rendering the
                // greenish completion card.
                let final_msg = Message {
                    role: Role::Assistant,
                    content: vec![ContentBlock::Text { text: summary.clone() }],
                };
                messages.push(final_msg.clone());
                let _ = event_tx.send(TaskEvent::MessageComplete {
                    task_id: task_id.clone(),
                    message: final_msg,
                });
                break;
            }

            // After appending tool results, the context has grown beyond what
            // `last_input_tokens` (from the previous API call) reflects.  Add a
            // char-based estimate of the new content so the condense check at the
            // top of the next iteration sees an up-to-date token count and fires
            // when needed rather than waiting until the API reports overflow.
            if let Some(last_msg) = messages.last() {
                let new_chars: usize = last_msg.content.iter().map(|b| match b {
                    ContentBlock::Text { text } => text.len(),
                    ContentBlock::ToolResult { content, .. } => content.len(),
                    ContentBlock::ToolUse { input, .. } => {
                        serde_json::to_string(input).map(|s| s.len()).unwrap_or(200)
                    }
                    _ => 0,
                }).sum();
                last_input_tokens = last_input_tokens.saturating_add((new_chars / 4) as u32);
            }

            // Loop back for next provider call
        }

        // After the loop ends (no more tool calls, cancellation, or turn limit),
        // compute the diff and emit TaskComplete so the UI shows what changed.
        let diff = context
            .compute_diff_fn
            .as_ref()
            .map(|f| f())
            .unwrap_or_else(|| TaskDiff {
                files: Vec::new(),
                total_insertions: 0,
                total_deletions: 0,
            });

        let summary = context.completion_summary.lock().ok().and_then(|s| s.clone());
        let _ = event_tx.send(TaskEvent::TaskComplete {
            task_id: task_id.clone(),
            diff,
            summary,
        });

        Ok(task_cost)
    }
}

/// Return a "call identity" string for a tool invocation. When two calls share
/// the same identity (same tool + same target), the earlier result can be
/// stubbed because the later call produces an authoritative, newer answer.
///
/// Returns None for tools where superseding doesn't make sense (shell commands,
/// write operations, agent spawns, etc.) — those always keep their results.
fn dedup_key_for_tool(name: &str, input: &serde_json::Value) -> Option<String> {
    let get = |k: &str| input.get(k).and_then(|v| v.as_str()).map(|s| s.to_string());
    match name {
        "read_file" | "list_directory" => get("path"),
        "glob" => get("pattern"),
        "grep" => {
            let pat = get("pattern").unwrap_or_default();
            let path = get("path").unwrap_or_default();
            if pat.is_empty() { None } else { Some(format!("{}@{}", pat, path)) }
        }
        _ => None,
    }
}

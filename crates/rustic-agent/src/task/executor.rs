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
        // Surface the configured cheaper sub-agent model to the schema only
        // when the host has wired one in; otherwise the schema hides the
        // tier choice and `spawn_subagent` falls back to the main model.
        let fast_subagent_model: Option<String> = context
            .subagent_provider_config
            .as_ref()
            .map(|c| c.model.clone());
        let mut tool_defs = self
            .tools
            .definitions_for_host(&available_shells, fast_subagent_model.as_deref());
        tool_defs.extend(crate::tools::web_tools::definitions_for(&context.tool_config));
        tool_defs.extend(context.mcp_tool_defs.clone());

        // Strip sub-agent-irrelevant tools when running as a sub-agent.
        // Pattern modelled on Claude Code's per-agent tool allowlist/denylist:
        // each agent definition gets its own tool pool, and tools that don't
        // apply (orchestrator tools for a child, recursive sub-agent tools,
        // user-interaction tools, etc.) are stripped before the API call.
        //
        // Why this matters for cost, not just correctness: every tool def is
        // a hundreds-of-tokens JSON schema sitting in the cache prefix on
        // every API call the sub-agent makes. With Anthropic's 5-minute
        // ephemeral cache TTL and a sub-agent that runs 30+ tool iterations,
        // an unused tool def gets re-cache-written multiple times. Stripping
        // ~7 unused tools shrinks the prefix by several thousand tokens per
        // call, which compounds across cache invalidations.
        //
        // Sub-agents are identified by `agent_depth >= 1` (set in
        // `subagent_tools.rs` when constructing the child `ToolContext`).
        if context.agent_depth >= 1 {
            // Tools that are nonsensical or actively harmful inside a sub-agent:
            //   - spawn_subagent / list_active_agents / wait_for_subagents:
            //     recursive spawning is already blocked at the tool body via
            //     a depth check, but the schema was still in the prefix.
            //   - spawn_subtask / list_tasks_across_projects / read_task_history:
            //     orchestrator-only tools; a child has no authority to use them.
            //   - chat_message / ask_user_question: sub-agents are designed
            //     to work autonomously and report back via `complete_task`;
            //     they shouldn't be conversing with the user mid-flight.
            const SUBAGENT_DENYLIST: &[&str] = &[
                "spawn_subagent",
                "list_active_agents",
                "wait_for_subagents",
                "spawn_subtask",
                "list_tasks_across_projects",
                "read_task_history",
                "chat_message",
                "ask_user_question",
            ];
            tool_defs.retain(|td| !SUBAGENT_DENYLIST.contains(&td.name.as_str()));
        }

        // Strip orchestrator-only tools from the parent agent's tool pool when
        // it's not running in Global mode. The execute path already returns an
        // error if a project agent calls one of these (`is_global` check in
        // orchestrator_tools.rs), but the JSON schema for the tool was still
        // in the cache prefix on every API call — and weaker models would
        // see e.g. `list_projects` in their tool list, call it to "discover
        // other projects," then either get an error and retry or interpret
        // the question as cross-project work.
        //
        // Symptom this fixes (real reproduction observed with GPT-OSS 120B):
        // user opens project `linkedin_api` and asks "explain the tools in
        // our project." A smart model interprets "tools" as project files
        // (the messaging/network/post/profile folders) and reads them. A
        // weaker model sees `list_projects` and `spawn_subtask` in its tool
        // catalog, conflates "tools" with the agent's own tool categories,
        // and starts spawning sub-agents named "Explain file navigation
        // tools" / "Explain shell execution tools" — Rustic's own tool
        // categories — instead of working in the user's project. Removing
        // these tools from the schema when the agent has no authority to
        // call them eliminates the confusion.
        if !context.is_global {
            const ORCHESTRATOR_DENYLIST: &[&str] = &[
                "list_projects",
                "spawn_subtask",
                "list_tasks_across_projects",
                "read_task_history",
            ];
            tool_defs.retain(|td| !ORCHESTRATOR_DENYLIST.contains(&td.name.as_str()));
        }
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

        // Local helper: incrementally save the in-flight `messages` Vec to
        // durable storage at every stable point in the loop. The host (Tauri
        // command layer) supplies the closure; sub-agents and unit tests get
        // `None` and skip the call. Without these calls, a Stop+immediate-
        // close sequence wipes the entire conversation because messages were
        // only persisted in `mod.rs` AFTER `run_turn` returned. With them,
        // the DB always reflects the most recent stable state — every
        // assistant response and every tool batch is durable as soon as it
        // exists in memory.
        let has_persist_fn = context.persist_messages_fn.is_some();
        tracing::info!(
            target: "rustic_agent::persist",
            task = %task_id,
            has_persist_fn,
            "run_turn entered — persist callback {}",
            if has_persist_fn { "present" } else { "MISSING (messages will only save at end of turn)" }
        );
        let persist_now = |msgs: &[Message]| {
            if let Some(f) = context.persist_messages_fn.as_ref() {
                tracing::info!(
                    target: "rustic_agent::persist",
                    task = %task_id,
                    count = msgs.len(),
                    "persist_now firing",
                );
                (f)(msgs);
            } else {
                tracing::warn!(
                    target: "rustic_agent::persist",
                    task = %task_id,
                    "persist_now called but no callback wired",
                );
            }
        };

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

            // **Skip aging + path-dedup for sub-agents.** Both strategies were
            // catastrophic on the sub-agent path:
            //
            //   1. Cache-busting. The aging logic *mutates* old tool-result
            //      bodies in `api_messages` (shrinking 30k-token file reads
            //      down to a 300-char preview). The cached prefix from the
            //      previous turn has the full body; this turn's prefix has
            //      the shrunk body. Different bytes → different cache key →
            //      Anthropic can't hit the message-level cache, so every turn
            //      re-writes the entire conversation prefix. The
            //      `cache_read=6644` (fixed at system+tools size) /
            //      `cache_write=35000` (huge every turn) pattern in the trace
            //      logs is the smoking gun — message cache *never* hit.
            //
            //   2. Re-read loops. The aged-out preview ends with "re-run the
            //      tool" — and the model does. The re-read becomes a new
            //      tool result, supersedes the prior one, which now ages out
            //      with the same "re-run" message, etc. We were observing
            //      sub-agents read the same 7 files 20+ times in a row.
            //
            // For sub-agents (agent_depth >= 1) the conversation is short
            // and the file set is small — aging can't pay for itself. Let the
            // condense mechanism handle context overflow if it ever happens.
            // For top-level agents, keep the existing behavior — long-running
            // sessions still benefit from aging, and we don't have a better
            // story for them yet.
            let is_subagent = context.agent_depth >= 1;

            // --- Build the set of tool_use_ids that should be shrunk by path dedup.
            // Walk forward, tracking the most recent tool_use_id for each
            // (tool_name, identity) key. When a key reappears, the previous id
            // goes into the shrink set.
            let mut shrink_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
            if !is_subagent {
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
            // For sub-agents, set age_cutoff = 0 so no tool result is ever
            // considered "aged out" (idx < 0 is never true). For top-level
            // agents, keep the original cutoff.
            let age_cutoff = if is_subagent {
                0
            } else {
                total_tool_results.saturating_sub(AGE_KEEP_FULL)
            };

            // Anthropic server-side tool results (web_search/web_fetch) ride
            // through ContentBlock::ToolResult as a *stringified JSON array*
            // and get re-inflated to native shape by the Claude adapter on
            // the next send. The shrink path below would slice that string
            // mid-element and append explanatory text, producing invalid JSON
            // — Anthropic then 400s on `web_search_tool_result.content` not
            // being a valid array. So when a server-side result would be
            // shrunk, drop the whole pair (the `server_tool_use` and its
            // result) instead. The model has already used the info.
            let server_tool_use_ids: std::collections::HashSet<String> = messages
                .iter()
                .flat_map(|m| m.content.iter())
                .filter_map(|b| match b {
                    ContentBlock::ToolUse { id, name, .. }
                        if name == "web_search" || name == "web_fetch" =>
                    {
                        Some(id.clone())
                    }
                    _ => None,
                })
                .collect();
            let dropped_server_ids: std::collections::HashSet<String> = {
                let mut dropped = std::collections::HashSet::new();
                let mut idx: usize = 0;
                for m in messages.iter() {
                    for b in m.content.iter() {
                        if let ContentBlock::ToolResult { tool_use_id, content, .. } = b {
                            let aged_out = idx < age_cutoff;
                            let superseded = shrink_ids.contains(tool_use_id);
                            let would_shrink = (aged_out || superseded)
                                && content.len() > AGED_PREVIEW_CHARS;
                            if would_shrink && server_tool_use_ids.contains(tool_use_id) {
                                dropped.insert(tool_use_id.clone());
                            }
                            idx += 1;
                        }
                    }
                }
                dropped
            };

            let mut seen_results: usize = 0;

            let api_messages: Vec<Message> = messages
                .iter()
                .map(|msg| Message {
                    role: msg.role.clone(),
                    content: msg
                        .content
                        .iter()
                        .filter(|b| !matches!(b, ContentBlock::ModelSwitch { .. }))
                        .filter_map(|b| match b {
                            // Drop the server-side tool_use whose paired result
                            // we are dropping below — keeping it would orphan
                            // a server_tool_use, which Anthropic also rejects.
                            ContentBlock::ToolUse { id, .. }
                                if dropped_server_ids.contains(id) =>
                            {
                                None
                            }
                            ContentBlock::ToolResult { tool_use_id, .. }
                                if dropped_server_ids.contains(tool_use_id) =>
                            {
                                // Still advance the index so the cutoff math
                                // for surviving results stays aligned.
                                seen_results += 1;
                                None
                            }
                            ContentBlock::Thinking { signature, .. } => Some(ContentBlock::Thinking {
                                thinking: String::new(),
                                signature: signature.clone(),
                                duration_secs: None,
                            }),
                            ContentBlock::ToolResult { tool_use_id, content, is_error } => {
                                let idx = seen_results;
                                seen_results += 1;
                                let aged_out = idx < age_cutoff;
                                let superseded = shrink_ids.contains(tool_use_id);
                                let should_shrink = (aged_out || superseded) && content.len() > AGED_PREVIEW_CHARS;
                                Some(if should_shrink {
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
                                    // **Don't end the preview with "re-run the tool".**
                                    // That phrasing actively encouraged a re-read loop:
                                    // the model would re-run the same call to "recover"
                                    // the content, which would then itself age out next
                                    // turn, and so on. Frame it instead as a memory aid:
                                    // earlier in the conversation the full content was
                                    // available, the model should refer to its own
                                    // understanding of it. Only suggest re-running when
                                    // the file/data has demonstrably changed.
                                    ContentBlock::ToolResult {
                                        tool_use_id: tool_use_id.clone(),
                                        content: format!(
                                            "{}\n\n[... {} — {} chars total. \
                                             Earlier in this conversation you saw the \
                                             full content; rely on what you already \
                                             know about it. Only re-run if the underlying \
                                             data has demonstrably changed since then.]",
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
                                })
                            }
                            other => Some(other.clone()),
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
                    ProviderStreamEvent::ToolUseStart { id, name } => {
                        let _ = stream_event_tx.send(TaskEvent::ToolUseStart {
                            task_id: stream_task_id.clone(),
                            tool_use_id: id,
                            tool_name: name,
                        });
                    }
                    ProviderStreamEvent::ToolUseInputDelta { id, partial_json } => {
                        let _ = stream_event_tx.send(TaskEvent::ToolUseInputDelta {
                            task_id: stream_task_id.clone(),
                            tool_use_id: id,
                            partial_json,
                        });
                    }
                    ProviderStreamEvent::ToolUseStop { id } => {
                        let _ = stream_event_tx.send(TaskEvent::ToolUseStop {
                            task_id: stream_task_id.clone(),
                            tool_use_id: id,
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
                    // Save right away on cancel so the worker thread doesn't
                    // need to survive the rest of the run_turn unwind for the
                    // partial response to be durable.
                    persist_now(messages);
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
            // Persist as soon as the model's response is in `messages`. If
            // the user clicks Stop before the next iteration's tools run,
            // this snapshot survives.
            persist_now(messages);

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
            // Persist again so tool outputs are durable before the next
            // provider call begins. A crash mid-call now keeps everything
            // up to and including the just-completed tool batch.
            persist_now(messages);

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
        //
        // Flip the UI to `Completed` BEFORE the (potentially slow) diff scan and
        // BEFORE the outer task's DB-persistence loop. The model has already
        // finished generating at this point — the cancel/reaper checks above
        // didn't fire, so the only way to get here naturally is "turn complete".
        // Without this early flip the spinner stays up for the full duration of
        // `compute_task_diff` + `delete_messages_for_task` + N inserts, which
        // is what users were perceiving as "stuck at generating after the
        // response is finished". The outer task in `mod.rs` still emits a final
        // status (and may downgrade to Failed/Cancelled if a late check trips);
        // the frontend treats repeated Completed flips as idempotent.
        let was_cancelled = context
            .cancel_token
            .as_ref()
            .map(|t| t.load(Ordering::SeqCst))
            .unwrap_or(false);
        if !was_cancelled {
            let _ = event_tx.send(TaskEvent::StatusChange {
                task_id: task_id.clone(),
                status: TaskStatus::Completed,
            });
        }

        let summary = context.completion_summary.lock().ok().and_then(|s| s.clone());
        let _ = event_tx.send(TaskEvent::TaskComplete {
            task_id: task_id.clone(),
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

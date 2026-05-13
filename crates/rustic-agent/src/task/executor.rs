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
        tool_defs.extend(crate::tools::media_tools::definitions_for(&context.tool_config));
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
            const SUBAGENT_DENYLIST: &[&str] = &[
                "spawn_subagent",
                "list_active_agents",
                "wait_for_subagents",
                "spawn_subtask",
                "list_tasks_across_projects",
                "read_task_history",
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
                    let _ = event_tx.try_send(TaskEvent::StatusChange {
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
                // after the first call they are served from cache. Defer the
                // (serialization-heavy) tool_defs measurement to the cold path
                // below — once `last_input_tokens` is populated from a real
                // provider response, none of this estimation runs.
                let estimated_tokens = if last_input_tokens > 0 {
                    // `last_input_tokens` already reflects the real prior request size
                    // (including system/tools), so don't double-add those.
                    last_input_tokens
                } else {
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
                    let _ = event_tx.try_send(TaskEvent::ContextCondenseStarted {
                        task_id: task_id.clone(),
                    });
                    let original_count = messages.len() as u32;

                    match condense::condense_context(&self.provider, &self.config, messages).await {
                        Ok((condensed, condense_usage)) => {
                            *messages = condensed;
                            // Include the condensing call's tokens in the task cost
                            task_cost.add_turn(model, &condense_usage);
                            let _ = event_tx.try_send(TaskEvent::CostUpdate {
                                task_id: task_id.clone(),
                                cost: task_cost.clone(),
                            });
                        }
                        Err(e) => {
                            tracing::warn!("[executor] condense failed ({}), using sliding window", e);
                            *messages = condense::sliding_window_fallback(messages);
                        }
                    }

                    let _ = event_tx.try_send(TaskEvent::ContextCondenseCompleted {
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

            // Strip UI-only ModelSwitch markers before sending to the API.
            // Thinking blocks are passed through unchanged — Anthropic rejects any
            // modification to the latest assistant message's thinking/redacted_thinking
            // blocks, and we can't know which one is "latest" at this point.
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
                            ContentBlock::Thinking { thinking, signature, duration_secs } => {
                                // Pass thinking blocks through unchanged. Anthropic rejects
                                // any modification to the *latest* assistant message's
                                // thinking blocks ("thinking or redacted_thinking blocks in
                                // the latest assistant message cannot be modified") — and
                                // we don't know at this point which message will end up
                                // being the latest, so we can't selectively redact.
                                Some(ContentBlock::Thinking {
                                    thinking: thinking.clone(),
                                    signature: signature.clone(),
                                    duration_secs: *duration_secs,
                                })
                            }
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

            // ── P0.6: persist shrinkage back into `messages` so the prefix bytes
            // sent to the provider stay stable across inner turns. Background:
            // the api_messages reshape above shrinks aged-out / superseded
            // tool_results from ~30K chars to ~300 chars. Without this step,
            // every inner turn re-decides what to shrink based on current
            // total_tool_results / shrink_ids — and as the conversation grows
            // by exactly one tool_result per iteration, the shrinkage boundary
            // advances and a previously-FULL result flips to SHRUNK in turn
            // N+1 vs turn N. Different bytes in the cached prefix → cache
            // miss → 12K cache-creation tokens per turn (R.2 F3, 15× the CLI
            // baseline). Persisting the shrunk content makes "once shrunk,
            // stays shrunk" an invariant (which is already true under current
            // shrink rules — neither aging nor dedup can un-shrink). Sub-agents
            // already skipped aging entirely, so the persist step is a no-op
            // for them.
            if !is_subagent {
                let shrunk_map: std::collections::HashMap<&str, &str> = api_messages
                    .iter()
                    .flat_map(|m| m.content.iter())
                    .filter_map(|b| match b {
                        ContentBlock::ToolResult { tool_use_id, content, .. } => {
                            Some((tool_use_id.as_str(), content.as_str()))
                        }
                        _ => None,
                    })
                    .collect();
                for msg in messages.iter_mut() {
                    for block in msg.content.iter_mut() {
                        if let ContentBlock::ToolResult { tool_use_id, content, .. } = block {
                            if let Some(new_content) = shrunk_map.get(tool_use_id.as_str()) {
                                // Only write back when api_messages is strictly
                                // smaller — guarantees idempotency and prevents
                                // any accidental restore of dropped content.
                                if new_content.len() < content.len() {
                                    *content = new_content.to_string();
                                }
                            }
                        }
                    }
                }
            }

            // ── P0.6: optional request dump for prompt-cache investigation.
            // When RUSTIC_DUMP_PROMPTS is set to a directory path, write the
            // full system prompt + tools + messages payload to a numbered file
            // per turn so two consecutive turns can be byte-diffed to identify
            // any remaining sources of cache invalidation. No-op when unset.
            dump_request_if_enabled(
                &task_id,
                self.config.system_prompt.as_deref(),
                &tool_defs,
                &api_messages,
            );

            // ── P0.4: budget gates ──────────────────────────────────────────
            // Check the daily ceiling before we make the API call. If the
            // user's spent past it today, emit an event the UI can render as
            // a "raise ceiling or stop" prompt, then bail out of the task
            // turn. Without this, every concurrent task happily burns
            // through the budget after the user has signalled they want to
            // cap spend.
            // P0.4 fix #4: on a ceiling breach, park the turn on the
            // ceiling broker and wait for the user to pick "Raise to …"
            // or "Stop task" in the frontend modal. Loop until we either
            // get Allowed or the user picks Stop (or the 24h timeout
            // fires); each RaiseTo bump from the user updates the Budget
            // in place and we re-check before proceeding.
            loop {
                match context.budget.check_within_ceiling() {
                    crate::budget::CeilingCheck::Allowed => break,
                    crate::budget::CeilingCheck::Blocked {
                        ceiling_cents,
                        spent_cents,
                    } => {
                        let request_id = uuid::Uuid::new_v4().to_string();
                        tracing::warn!(
                            task = %task_id,
                            spent_cents,
                            ceiling_cents,
                            request_id = %request_id,
                            "P0.4 fix #4: ceiling breached, parking turn on broker"
                        );
                        let _ = event_tx.try_send(TaskEvent::CeilingBreached {
                            task_id: task_id.clone(),
                            request_id: request_id.clone(),
                            ceiling_cents,
                            spent_cents,
                        });
                        let resolution = context
                            .ceiling_broker
                            .wait_for_resolution(&request_id)
                            .await;
                        match resolution {
                            Some(crate::task::ceiling_broker::CeilingResolution::RaiseTo(new_cents)) => {
                                // The Tauri command also persists this into
                                // ai_config.budget so subsequent tasks see
                                // the new ceiling; here we only update the
                                // in-memory Budget the executor sees. Loop
                                // back to re-check — if `new_cents` is
                                // still <= spent_cents the user gets the
                                // modal again with the new context.
                                context.budget.raise_ceiling(new_cents);
                                tracing::info!(
                                    task = %task_id,
                                    new_ceiling_cents = new_cents,
                                    "P0.4 fix #4: ceiling raised, re-checking"
                                );
                                continue;
                            }
                            Some(crate::task::ceiling_broker::CeilingResolution::Stop) | None => {
                                // User chose Stop, or the 24h timeout fired
                                // before any response. Either way, fail the
                                // task with the same error shape as before
                                // so the UI's existing failure path runs.
                                let _ = event_tx.try_send(TaskEvent::StatusChange {
                                    task_id: task_id.clone(),
                                    status: TaskStatus::Failed,
                                });
                                return Err(anyhow::anyhow!(
                                    "Daily cost ceiling reached: ${:.2} of ${:.2} spent today. \
                                     Task stopped at user's request.",
                                    spent_cents as f64 / 100.0,
                                    ceiling_cents as f64 / 100.0
                                ));
                            }
                        }
                    }
                }
            }
            // Acquire the stream permit (blocks on contention when many tasks
            // are racing for slots). Held until this iteration's retry loop
            // finishes — covers all retry attempts for the same logical turn,
            // so a stalling task doesn't free up a slot for someone else mid-
            // retry. The permit is `None` when the user has disabled the cap.
            let _stream_permit = context.budget.acquire_stream_permit().await;

            // ── P0.1: stream-retry wrap ────────────────────────────────────────
            // Up to 3 retries on transient provider errors (1 initial + 3 retries
            // = 4 calls). Backoffs between retries: immediate, 30s, 60s. Total
            // max wait ≈ 90s, matching the plan.
            //
            // We do NOT retry on:
            //   - "Task cancelled" — the user clicked Stop, propagate immediately.
            //   - 4xx-class errors from the provider (auth, malformed request,
            //     model not found). HTTP-layer retry already covered transient
            //     5xx + connection errors; what gets here is post-stream-start
            //     failures (mid-stream disconnects, parse errors, partial reads).
            //
            // P0.1 (full): mid-stream inactivity watchdog. Each attempt gets:
            //   - A per-attempt cancel token chained to the user cancel —
            //     user Stop still aborts the in-flight HTTP request, AND the
            //     watchdog can also flip it on stall detection without
            //     touching the user-facing cancel.
            //   - An Arc<AtomicU64> last_activity_ms updated by the stream_cb
            //     on every TextDelta / ThinkingDelta / ServerTool* / ToolUse*.
            //   - A tokio watchdog task that polls last_activity_ms every 5s.
            //     If `now - last > STALL_THRESHOLD_MS` (30s) AND the user
            //     hasn't cancelled, it flips a `stalled` flag plus the
            //     per-attempt cancel, then exits. The chat call wakes up
            //     with a cancel error; we check `stalled` and route the
            //     error through the retry path (with the same backoff).
            const MAX_STREAM_ATTEMPTS: u32 = 4; // 1 + 3 retries
            const STREAM_RETRY_BACKOFFS_MS: [u64; 3] = [0, 30_000, 60_000];
            const STALL_THRESHOLD_MS: u64 = 30_000;
            const STALL_POLL_INTERVAL_MS: u64 = 2_000;
            let mut stream_attempt: u32 = 0;
            let response = 'attempt_loop: loop {
                stream_attempt += 1;
                // ── P0.1: per-attempt watchdog state ──────────────────────
                // last_activity_ms updated on every stream event (below);
                // watchdog reads it. now_ms() is wall-clock — adequate for
                // a 30s coarse threshold; we don't need monotonic precision.
                let last_activity_ms = Arc::new(std::sync::atomic::AtomicU64::new(now_ms_for_watchdog()));
                let stalled = Arc::new(std::sync::atomic::AtomicBool::new(false));
                // Per-attempt cancel token routed into config. Set by EITHER
                // the watchdog (on stall) OR the user-cancel propagator below.
                let per_attempt_cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
                // Re-build the streaming callback each attempt — the prior
                // closure was consumed by `chat()`. Same Arc handles inside,
                // so this is cheap (the per-task partial-text buffer and
                // event channel are shared across attempts).
                let stream_task_id_a = task_id.clone();
                let stream_event_tx_a = event_tx.clone();
                let partial_for_cb_a = Arc::clone(&partial_assistant_text);
                let last_activity_for_cb = Arc::clone(&last_activity_ms);
                let stream_cb_attempt: StreamCallback = Arc::new(move |event| {
                    // P0.1: bump the activity timestamp on EVERY event type —
                    // not just text deltas — so a stream actively producing
                    // tool_use blocks or thinking deltas isn't mistaken for
                    // a stall.
                    last_activity_for_cb.store(now_ms_for_watchdog(), Ordering::Relaxed);
                    match event {
                    ProviderStreamEvent::TextDelta(text) => {
                        if let Ok(mut buf) = partial_for_cb_a.lock() {
                            buf.push_str(&text);
                        }
                        let _ = stream_event_tx_a.try_send(TaskEvent::TextDelta {
                            task_id: stream_task_id_a.clone(),
                            text,
                        });
                    }
                    ProviderStreamEvent::ThinkingDelta(text) => {
                        let _ = stream_event_tx_a.try_send(TaskEvent::ThinkingDelta {
                            task_id: stream_task_id_a.clone(),
                            text,
                        });
                    }
                    ProviderStreamEvent::ServerToolUse { id, name, input } => {
                        let _ = stream_event_tx_a.try_send(TaskEvent::ToolUse {
                            task_id: stream_task_id_a.clone(),
                            tool_use_id: id,
                            tool_name: name,
                            tool_input: input,
                        });
                    }
                    ProviderStreamEvent::ServerToolResult { tool_use_id, content, is_error } => {
                        let _ = stream_event_tx_a.try_send(TaskEvent::ToolResult {
                            task_id: stream_task_id_a.clone(),
                            tool_use_id,
                            output: content,
                            is_error,
                        });
                    }
                    ProviderStreamEvent::ToolUseStart { id, name } => {
                        let _ = stream_event_tx_a.try_send(TaskEvent::ToolUseStart {
                            task_id: stream_task_id_a.clone(),
                            tool_use_id: id,
                            tool_name: name,
                        });
                    }
                    ProviderStreamEvent::ToolUseInputDelta { id, partial_json } => {
                        let _ = stream_event_tx_a.try_send(TaskEvent::ToolUseInputDelta {
                            task_id: stream_task_id_a.clone(),
                            tool_use_id: id,
                            partial_json,
                        });
                    }
                    ProviderStreamEvent::ToolUseStop { id } => {
                        let _ = stream_event_tx_a.try_send(TaskEvent::ToolUseStop {
                            task_id: stream_task_id_a.clone(),
                            tool_use_id: id,
                        });
                    }
                    }
                });

                // ── P0.1: spawn watchdog + user-cancel propagator ─────────
                // The watchdog polls last_activity_ms; if no event for
                // STALL_THRESHOLD_MS it flips the stall flag and the
                // per-attempt cancel (which the provider polls on).
                let last_activity_w = Arc::clone(&last_activity_ms);
                let stalled_w = Arc::clone(&stalled);
                let per_attempt_cancel_w = Arc::clone(&per_attempt_cancel);
                let task_id_w = task_id.clone();
                let watchdog = tokio::spawn(async move {
                    loop {
                        tokio::time::sleep(std::time::Duration::from_millis(STALL_POLL_INTERVAL_MS)).await;
                        if per_attempt_cancel_w.load(Ordering::SeqCst) {
                            return;
                        }
                        let last = last_activity_w.load(Ordering::Relaxed);
                        let now = now_ms_for_watchdog();
                        if now.saturating_sub(last) > STALL_THRESHOLD_MS {
                            tracing::warn!(
                                task = %task_id_w,
                                last_activity_age_ms = now - last,
                                "P0.1: stream stalled (>{}ms with no events) — flipping per-attempt cancel",
                                STALL_THRESHOLD_MS
                            );
                            stalled_w.store(true, Ordering::SeqCst);
                            per_attempt_cancel_w.store(true, Ordering::SeqCst);
                            return;
                        }
                    }
                });
                // Propagator: copy user_cancel → per_attempt_cancel so a
                // user-initiated Stop still aborts the in-flight HTTP. The
                // chat call only checks per_attempt_cancel via its config;
                // without this bridge the user's Stop would do nothing
                // mid-stream. Exits when per_attempt_cancel flips for ANY
                // reason (user or watchdog).
                let user_cancel_for_prop = context.cancel_token.clone();
                let per_attempt_cancel_prop = Arc::clone(&per_attempt_cancel);
                let cancel_propagator = tokio::spawn(async move {
                    loop {
                        if let Some(ut) = &user_cancel_for_prop {
                            if ut.load(Ordering::SeqCst) {
                                per_attempt_cancel_prop.store(true, Ordering::SeqCst);
                                return;
                            }
                        }
                        if per_attempt_cancel_prop.load(Ordering::SeqCst) {
                            return;
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                    }
                });

                // Clone config and override its cancel_token to point at our
                // per-attempt token. The user's cancel is propagated above.
                let mut attempt_config = self.config.clone();
                attempt_config.cancel_token = Some(Arc::clone(&per_attempt_cancel));

                // Clone api_messages so a future retry can reuse them; the
                // provider's chat() takes ownership.
                let api_messages_for_attempt = api_messages.clone();
                let attempt_result = self
                    .provider
                    .chat(
                        api_messages_for_attempt,
                        tool_defs.clone(),
                        &attempt_config,
                        Some(stream_cb_attempt),
                    )
                    .await;
                // Stop the background tasks. Aborting an already-finished
                // task is a no-op; aborting a running one drops its future
                // immediately (we don't await the join handles).
                watchdog.abort();
                cancel_propagator.abort();
                let did_stall = stalled.load(Ordering::SeqCst);
                let user_cancelled = context
                    .cancel_token
                    .as_ref()
                    .map(|t| t.load(Ordering::SeqCst))
                    .unwrap_or(false);
                match attempt_result {
                    Ok(resp) => break 'attempt_loop Ok(resp),
                    Err(e) if e.to_string().contains("Task cancelled") && user_cancelled => {
                        // The user actually clicked Stop — propagate as-is.
                        // (If the per-attempt cancel fired due to a stall,
                        // the chat call will also return "Task cancelled",
                        // but user_cancelled is false in that case and we
                        // fall through to the retry branch below.)
                        break 'attempt_loop Err(e);
                    }
                    Err(_e) if did_stall && stream_attempt >= MAX_STREAM_ATTEMPTS => {
                        // Stalled out and no retries left — surface a clear
                        // stall-specific error instead of the bare cancel
                        // message so the UI can show something useful.
                        tracing::warn!(
                            task = %task_id,
                            attempts = stream_attempt,
                            "P0.1: stream stalled and retries exhausted"
                        );
                        break 'attempt_loop Err(anyhow::anyhow!(
                            "STREAM_STALLED: The provider stopped sending tokens for over 30 seconds \
                             and {} retry attempts also stalled. Check your network connection and try again.",
                            MAX_STREAM_ATTEMPTS
                        ));
                    }
                    Err(e) if stream_attempt >= MAX_STREAM_ATTEMPTS => {
                        // Out of retries — give up with the last error.
                        tracing::warn!(
                            task = %task_id,
                            error = %e,
                            attempts = stream_attempt,
                            "P0.1: stream retries exhausted, surfacing error to user"
                        );
                        break 'attempt_loop Err(e);
                    }
                    Err(e) if did_stall => {
                        // Stall detected mid-stream — retry with the standard
                        // backoff schedule. Log the stall explicitly so
                        // operators can correlate with provider incidents.
                        tracing::warn!(
                            task = %task_id,
                            attempt = stream_attempt,
                            "P0.1: stream stalled, retrying after backoff"
                        );
                        let backoff_idx = (stream_attempt - 1) as usize;
                        let waiting_ms = STREAM_RETRY_BACKOFFS_MS
                            .get(backoff_idx)
                            .copied()
                            .unwrap_or(60_000);
                        let _ = event_tx.try_send(TaskEvent::StreamRetry {
                            task_id: task_id.clone(),
                            attempt: stream_attempt + 1,
                            max_attempts: MAX_STREAM_ATTEMPTS,
                            waiting_ms: waiting_ms as u32,
                        });
                        if let Ok(mut buf) = partial_assistant_text.lock() {
                            buf.clear();
                        }
                        if waiting_ms > 0 {
                            // Same cancellable-sleep dance as the generic
                            // retry branch below. Bail to the cancel handler
                            // if the user clicks Stop during the backoff.
                            let sleep_fut = tokio::time::sleep(std::time::Duration::from_millis(waiting_ms));
                            tokio::pin!(sleep_fut);
                            let cancel_check = async {
                                loop {
                                    if let Some(token) = &context.cancel_token {
                                        if token.load(Ordering::SeqCst) {
                                            return;
                                        }
                                    }
                                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                                }
                            };
                            tokio::pin!(cancel_check);
                            tokio::select! {
                                _ = &mut sleep_fut => {}
                                _ = &mut cancel_check => {
                                    break 'attempt_loop Err(anyhow::anyhow!("Task cancelled"));
                                }
                            }
                        }
                        let _ = e; // discard the stall-as-cancel error
                        // Loop and try again.
                    }
                    Err(e) if crate::provider::is_provider_client_error(&e) => {
                        // P0.1: 4xx-class error (auth, malformed request, model
                        // not found, payload too large). Retrying re-issues the
                        // same bad request — burns 90s of backoff before the
                        // user sees the real problem. Surface immediately.
                        tracing::warn!(
                            task = %task_id,
                            error = %e,
                            attempt = stream_attempt,
                            "P0.1: provider 4xx — not retrying, surfacing to user"
                        );
                        break 'attempt_loop Err(e);
                    }
                    Err(e) => {
                        // Transient — back off and retry.
                        let backoff_idx = (stream_attempt - 1) as usize;
                        let waiting_ms = STREAM_RETRY_BACKOFFS_MS
                            .get(backoff_idx)
                            .copied()
                            .unwrap_or(60_000);
                        tracing::warn!(
                            task = %task_id,
                            error = %e,
                            attempt = stream_attempt,
                            waiting_ms,
                            "P0.1: stream call failed, retrying after backoff"
                        );
                        let _ = event_tx.try_send(TaskEvent::StreamRetry {
                            task_id: task_id.clone(),
                            attempt: stream_attempt + 1,
                            max_attempts: MAX_STREAM_ATTEMPTS,
                            waiting_ms: waiting_ms as u32,
                        });
                        // Discard any partial tokens from the failed attempt
                        // so the next attempt's stream isn't appended to a
                        // stale prefix (matters for the cancel-recovery path
                        // which reads partial_assistant_text on a hard exit).
                        if let Ok(mut buf) = partial_assistant_text.lock() {
                            buf.clear();
                        }
                        if waiting_ms > 0 {
                            // Honour cancellation during the sleep, too.
                            let sleep_fut = tokio::time::sleep(std::time::Duration::from_millis(waiting_ms));
                            tokio::pin!(sleep_fut);
                            let cancel_check = async {
                                loop {
                                    if let Some(token) = &context.cancel_token {
                                        if token.load(Ordering::SeqCst) {
                                            return;
                                        }
                                    }
                                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                                }
                            };
                            tokio::pin!(cancel_check);
                            tokio::select! {
                                _ = &mut sleep_fut => {}
                                _ = &mut cancel_check => {
                                    // User cancelled during backoff — synthesize
                                    // a cancel error so the existing handler runs.
                                    break 'attempt_loop Err(anyhow::anyhow!("Task cancelled"));
                                }
                            }
                        }
                        // Loop and try again with a fresh stream_cb.
                    }
                }
            };
            // Note: the original single-attempt match below is preserved so
            // user-cancel + final-error paths behave identically to before.
            let response = match response {
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
                        let _ = event_tx.try_send(TaskEvent::MessageComplete {
                            task_id: task_id.clone(),
                            message: assistant_msg,
                        });
                    }
                    // Save right away on cancel so the worker thread doesn't
                    // need to survive the rest of the run_turn unwind for the
                    // partial response to be durable.
                    persist_now(messages);
                    let _ = event_tx.try_send(TaskEvent::StatusChange {
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
            let request_cost_usd = crate::task::cost::calculate_cost(model, &response.usage);
            // P0.4: feed this turn's spend into the daily-cost-ceiling
            // counter. No-op when the user has disabled the ceiling.
            context.budget.record_cost(request_cost_usd);
            let _ = event_tx.try_send(TaskEvent::RequestUsage {
                task_id: task_id.clone(),
                input_tokens: response.usage.input_tokens,
                output_tokens: response.usage.output_tokens,
                cache_read_tokens: response.usage.cache_read_tokens,
                cache_write_tokens: response.usage.cache_write_tokens,
                cost_usd: request_cost_usd,
            });
            let _ = event_tx.try_send(TaskEvent::CostUpdate {
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
                // JSON is almost certainly incomplete. Also drop tool_result
                // blocks — in an assistant message these are server-tool results
                // (web_search / web_fetch) paired with the server_tool_use we
                // just stripped. Leaving them produces orphan srvtoolu_* ids and
                // a 400 "tool_result must have a corresponding tool_use" on the
                // next request.
                response.content.iter().filter(|b| {
                    !matches!(b, ContentBlock::ToolUse { .. } | ContentBlock::ToolResult { .. })
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

            let _ = event_tx.try_send(TaskEvent::MessageComplete {
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
                    let _ = event_tx.try_send(TaskEvent::StatusChange {
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
                let _ = event_tx.try_send(TaskEvent::ToolUse {
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
            let (read_only_tools, mut write_tools): (Vec<_>, Vec<_>) = tool_uses
                .iter()
                .partition(|(_, name, _)| BuiltinTools::is_read_only(name));

            let mut results: Vec<(String, ToolOutput)> = Vec::new();
            // Include any parse-error results from the filter step above
            results.extend(parse_error_results);

            // P0.3 plan mode: when the user has flipped this task into plan
            // mode, the agent can investigate freely (read tools run as
            // normal in parallel below) but every write- / execute-class
            // tool is rejected before it reaches its handler. The agent
            // sees PERMISSION_DENIED with a clear plan-mode message and is
            // expected to propose changes as plain text instead. Exits
            // plan mode by the user toggling it off in the task panel
            // (frontend follow-up).
            //
            // Sub-agents inherit the parent's plan-mode setting (wired in
            // tools/subagent_tools.rs), so a parent can't delegate around
            // the gate by spawning a child.
            if context.is_plan_mode && !write_tools.is_empty() {
                let blocked: Vec<(String, ToolOutput)> = write_tools
                    .iter()
                    .map(|(tool_id, tool_name, _)| {
                        (
                            tool_id.clone(),
                            ToolOutput {
                                content: format!(
                                    "PERMISSION_DENIED: `{}` is blocked because this task is in \
                                     PLAN MODE. Plan mode allows only investigation and proposal \
                                     — file writes, patches, command execution, terminal control, \
                                     and any MCP write-tools are rejected until the user disables \
                                     plan mode on the task panel. Continue investigating with \
                                     read_file / grep_search / glob / list_directory; once you \
                                     have a plan, summarise it as plain text and end your turn so \
                                     the user can review and exit plan mode.",
                                    tool_name
                                ),
                                is_error: true,
                            },
                        )
                    })
                    .collect();
                results.extend(blocked);
                // Drain write_tools so the sequential-execution loop below
                // skips them entirely (we've already returned synthetic
                // results for each one).
                write_tools.clear();
            }

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
                let _ = event_tx.try_send(TaskEvent::ToolResult {
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

            // Drain any non-token tool costs (media generation: image / video
            // tools that hit per-image or per-second priced endpoints). The
            // tools deposit estimated dollar spend into context.tool_cost_sink
            // during execution; we add it onto task_cost so the chat header's
            // cumulative cost reflects the real total, not just chat-model
            // token spend. Emits a CostUpdate so the UI re-renders the badge
            // immediately rather than waiting for the next provider call.
            let extra_tool_cost = {
                let mut sink = context.tool_cost_sink.lock().unwrap();
                let v = *sink;
                *sink = 0.0;
                v
            };
            if extra_tool_cost > 0.0 {
                task_cost.estimated_cost_usd += extra_tool_cost;
                let _ = event_tx.try_send(TaskEvent::CostUpdate {
                    task_id: task_id.clone(),
                    cost: task_cost.clone(),
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
            let _ = event_tx.try_send(TaskEvent::StatusChange {
                task_id: task_id.clone(),
                status: TaskStatus::Completed,
            });
        }

        // Natural termination — the model ended its turn with no tool calls.
        // The final assistant text bubble is the summary. We no longer carry
        // a separate `summary` string (that was the `complete_task` tool's job).
        let _ = event_tx.try_send(TaskEvent::TaskComplete {
            task_id: task_id.clone(),
            summary: None,
        });

        Ok(task_cost)
    }
}

/// P0.1 watchdog helper: wall-clock ms since the unix epoch, used to detect
/// "no events for >30s" stalls. The actual threshold doesn't need monotonic
/// precision — a 30s coarse check is tolerant of NTP adjustments and the
/// ~2s polling interval already dominates the error budget.
fn now_ms_for_watchdog() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// P0.6 investigation aid: when `RUSTIC_DUMP_PROMPTS` is set to a directory,
/// write the full per-turn request (system prompt + tools + messages) to a
/// timestamped file in that directory. Two consecutive turns can then be
/// byte-diffed to identify any remaining sources of cache invalidation.
///
/// The env var is read on every call so it can be toggled mid-session for
/// targeted captures. Failures (missing dir, write error, JSON serialization
/// problem) are silent — this is purely diagnostic and must never break the
/// agent loop.
fn dump_request_if_enabled(
    task_id: &str,
    system_prompt: Option<&str>,
    tool_defs: &[crate::provider::ToolDef],
    api_messages: &[Message],
) {
    let Ok(dir) = std::env::var("RUSTIC_DUMP_PROMPTS") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::debug!(?e, dir = %dir.display(), "RUSTIC_DUMP_PROMPTS: mkdir failed");
        return;
    }
    // Counter file per task so multiple runs / multiple tasks don't collide
    // and we can byte-diff turn N vs turn N+1 by filename order.
    let counter_path = dir.join(format!(".rustic-dump-counter-{}", sanitize_for_path(task_id)));
    let next: u64 = std::fs::read_to_string(&counter_path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
        + 1;
    let _ = std::fs::write(&counter_path, next.to_string());

    let filename = dir.join(format!(
        "{}-turn-{:04}.json",
        sanitize_for_path(task_id),
        next
    ));
    let payload = serde_json::json!({
        "task_id": task_id,
        "turn": next,
        "system_prompt": system_prompt,
        "tools": tool_defs.iter().map(|t| serde_json::json!({
            "name": t.name,
            "description": t.description,
            "input_schema": t.parameters,
        })).collect::<Vec<_>>(),
        "messages": api_messages,
    });
    match serde_json::to_string_pretty(&payload) {
        Ok(s) => {
            if let Err(e) = std::fs::write(&filename, s) {
                tracing::debug!(?e, file = %filename.display(), "RUSTIC_DUMP_PROMPTS: write failed");
            }
        }
        Err(e) => {
            tracing::debug!(?e, "RUSTIC_DUMP_PROMPTS: serialize failed");
        }
    }
}

/// Keep dump filenames filesystem-safe across task_id values that may contain
/// `:` or `/`. ASCII-only mapping; UTF-8 multibyte passes through.
fn sanitize_for_path(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect()
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

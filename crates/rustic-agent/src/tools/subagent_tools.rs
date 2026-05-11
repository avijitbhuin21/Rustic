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

/// Build the sub-agent tool definitions.
///
/// `fast_model` is the user-configured cheaper/faster model id (e.g.
/// `"claude-haiku-4-5"`) or `None` when no sub-agent model is configured.
/// When `Some`, the `spawn_subagent` schema gains a required `model_tier`
/// enum so the orchestrator picks per-spawn between `"intelligent"` (the
/// main chat model) and `"fast"` (this configured model). When `None`, the
/// schema omits the choice entirely — every spawn uses the main model,
/// matching the original behaviour.
pub fn definitions(fast_model: Option<&str>) -> Vec<ToolDef> {
    let has_fast = fast_model.is_some();
    let fast_label = fast_model.unwrap_or("");

    let base_description = "Launch a sub-agent to handle a task in parallel. The sub-agent runs \
                          independently and can read files, search code, and generate content on its own. \
                          IMPORTANT: Delegate the TASK, not the solution — tell the sub-agent WHAT to \
                          accomplish, not the exact content to write. Do NOT pre-read files or generate \
                          content yourself to pass in the prompt. The sub-agent has full tool access. \
                          Use `wait_for_subagents` to wait for results. \
                          Only the main agent can spawn sub-agents (depth limit: 1). \
                          Declare `writes` for any files the sub-agent will modify — spawning a \
                          sub-agent whose writes collide with an already-running one is rejected. \
                          Max concurrent sub-agents: 4.";
    let description = if has_fast {
        format!(
            "{base} You MUST also pick `model_tier`: \"intelligent\" reuses the main chat \
             model for hard reasoning / multi-step planning work; \"fast\" routes the \
             sub-agent to the cheaper, faster model the user configured ({fast}). \
             Prefer \"fast\" for mechanical work (bulk file reads, simple search-and-replace \
             edits, summarising findings, drafting docstrings) — it's still good at tool \
             calls. Use \"intelligent\" when the sub-agent has to reason about tricky code, \
             design tradeoffs, or debug subtle behaviour.",
            base = base_description,
            fast = fast_label
        )
    } else {
        format!("{} The sub-agent inherits the same model as the main agent.", base_description)
    };

    let mut props = serde_json::Map::new();
    if has_fast {
        props.insert("model_tier".to_string(), json!({
            "type": "string",
            "enum": ["intelligent", "fast"],
            "description": format!(
                "Which model the sub-agent should use. \"intelligent\" = the main chat \
                 model (best for reasoning-heavy work). \"fast\" = the cheaper/faster model \
                 configured in settings ({}), good for tool-driven mechanical work. Pick \
                 based on the task's reasoning load, not its length.",
                fast_label
            ),
        }));
    }
    props.insert("name".to_string(), json!({
        "type": "string",
        "description": "A short (3-5 word) name for this sub-agent. Used as the \
                        agent's display name and ID. E.g. 'refactor auth module', \
                        'write unit tests', 'fix login bug'."
    }));
    props.insert("prompt".to_string(), json!({
        "type": "string",
        "description": "The task description for the sub-agent. Describe WHAT to do and \
                        WHERE (file paths, directories), but do NOT include file contents \
                        or pre-generated code. The sub-agent will read files and generate \
                        content itself using its tools."
    }));
    props.insert("writes".to_string(), json!({
        "type": "array",
        "items": { "type": "string" },
        "description": "File or directory paths (repo-relative) this sub-agent will \
                        create, edit, or delete. Used to detect collisions with other \
                        running sub-agents. Leave empty for read-only tasks (research, \
                        analysis, summarization). Directory entries cover everything \
                        beneath them. Be tight — over-declaring serializes agents that \
                        could have run in parallel."
    }));
    props.insert("reads".to_string(), json!({
        "type": "array",
        "items": { "type": "string" },
        "description": "Optional: file or directory paths the sub-agent will read. \
                        Informational only; reads never cause collisions."
    }));

    let required: Vec<&str> = if has_fast {
        vec!["model_tier", "name", "prompt"]
    } else {
        vec!["name", "prompt"]
    };

    vec![
        ToolDef {
            name: "spawn_subagent".to_string(),
            description,
            parameters: json!({
                "type": "object",
                "required": required,
                "properties": Value::Object(props),
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
                          a follow-up sub-agent. After calling this for every blocked write, finish \
                          the work you CAN do in-scope and end your turn with a plain-text summary of \
                          what you did and didn't complete — that text is what the parent agent sees. \
                          This tool does not end the task on its own — it only records. No-op when \
                          called by the main agent.".to_string(),
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
            "Recorded blocked write: '{}'. Finish whatever you can do in-scope, then end your \
             turn with a plain-text summary describing what you did and didn't complete — that \
             text is what the parent agent will see.",
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

    // Pick which provider config to use. The schema only exposes
    // `model_tier` when a sub-agent model is configured; when the main agent
    // calls with `model_tier: "fast"` AND the host has built a
    // `subagent_provider_config`, we route to that. In every other case we
    // use the parent's config — same as before this feature existed.
    let model_tier = params.get("model_tier")
        .and_then(|v| v.as_str())
        .unwrap_or("intelligent");
    let use_fast = model_tier.eq_ignore_ascii_case("fast")
        && context.subagent_provider_config.is_some();

    let chosen_config = if use_fast {
        // unwrap is safe: `use_fast` checked is_some() above.
        context.subagent_provider_config.as_ref().unwrap()
    } else {
        context.parent_provider_config.as_ref().ok_or_else(|| {
            anyhow::anyhow!("No parent provider config available for sub-agent spawning")
        })?
    };
    let model = chosen_config.model.clone();

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

    // Sub-agents use their OWN dedicated, lean system prompt — NOT the
    // parent's. The parent's prompt is huge (tool catalog, orchestrator
    // workflow, memory injection, etc.) and most of it is irrelevant to a
    // child sub-agent that's just doing "read these files and tell me X".
    //
    // Why this is a cost problem (not just a correctness one):
    //   - The full prompt sits at the start of every API call the sub-agent
    //     makes, included in the cache prefix.
    //   - Anthropic's ephemeral cache TTL is 5 minutes. A sub-agent running
    //     30+ tool iterations typically blows past that window, which forces
    //     a fresh cache_creation write of the *entire* prefix — including
    //     the 10-20k-token parent prompt — on the next call.
    //   - At Sonnet 4.6 cache_write rates ($3.75/M), each such invalidation
    //     was costing real money. Across multiple TTL boundaries and the
    //     accumulating tool-result content, total_cache_write_tokens for
    //     a single sub-agent regularly hit the millions and the displayed
    //     cost ran into double-digit dollars for "read 5 files" tasks.
    //
    // Using `build_subagent_prompt()` keeps the per-call prefix lean
    // (~700 tokens) so cache invalidations are cheap and the bulk of the
    // sub-agent's spend goes to the actual file content / output it
    // produces, not to re-caching prompt boilerplate.
    let sub_system_prompt = crate::system_prompt::build_subagent_prompt();
    let _unused_parent_prompt = &chosen_config.system_prompt;

    // Sub-agents run with thinking DISABLED. Pattern matches Claude Code's
    // own AgentTool/runAgent.ts where the inline comment reads:
    //
    //   For fork children (useExactTools), inherit thinking config to match
    //   the parent's API request prefix for prompt cache hits. For regular
    //   sub-agents, disable thinking to control output token costs.
    //
    // We don't have a "fork" path (sub-agents are always fresh in Rustic),
    // so the disable branch is the only one we need. Thinking on a sub-agent
    // doing "read these 5 files and summarize" is pure overhead — the model
    // doesn't need extended reasoning for mechanical lookup work, and
    // every API call inside the sub-agent's loop would otherwise burn
    // thinking-budget output tokens on top of the actual work.
    //
    // `0` is the convention for "thinking disabled" on the Claude provider
    // (`provider/claude.rs` checks `config.thinking_budget > 0` before
    // enabling extended thinking on a request).
    let sub_config = ProviderConfig {
        api_key: chosen_config.api_key.clone(),
        model: model.clone(),
        max_tokens: chosen_config.max_tokens,
        temperature: chosen_config.temperature,
        base_url: chosen_config.base_url.clone(),
        system_prompt: Some(sub_system_prompt),
        thinking_budget: 0,
        context_window: chosen_config.context_window,
        web_search_enabled: chosen_config.web_search_enabled,
        web_fetch_enabled: chosen_config.web_fetch_enabled,
        supports_temperature: chosen_config.supports_temperature,
        supports_reasoning_effort: chosen_config.supports_reasoning_effort,
        cancel_token: context.cancel_token.clone(),
    };

    // Register sub-agent (with declared writes for future collision checks)
    context.subagent_registry.register(&context.task_id, &agent_id, &model, writes.clone());
    tracing::warn!("[subagent] Registered '{}' under task '{}' with model '{}'", agent_id, context.task_id, model);

    // Emit spawned event
    let _ = context.event_tx.try_send(TaskEvent::SubagentSpawned {
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
    // Sub-agents always run in FullAuto regardless of the parent's level —
    // a sub-agent that pauses on a permission prompt would stall the parent
    // waiting on its completion. The FullAuto contract is "no approval
    // prompts," so the file-ops tier-2 check bypasses the sensitive-file
    // prompt for FullAuto callers; that means FullAuto sub-agents can read
    // `.env` and similar without prompting, matching the parent's mode.
    // `write_scope` (passed separately as `child_write_scope`) still
    // constrains exactly which files the sub-agent can touch.
    let child_shared_permissions = crate::task::permissions::SharedPermissions::new(
        crate::PermissionLevel::FullAuto,
        false,
    );
    let child_file_lock = Arc::clone(&context.file_lock);
    let child_permission_broker = Arc::clone(&context.permission_broker);
    let child_ai_config = Arc::clone(&context.ai_config);
    let child_tool_config = Arc::clone(&context.tool_config);
    let child_mcp_manager = context.mcp_manager.clone();
    let child_mcp_tool_defs = context.mcp_tool_defs.clone();
    let child_subagent_registry = Arc::clone(&context.subagent_registry);
    let child_allowed_paths = context.allowed_paths.clone();
    let child_write_scope = writes.clone();
    let child_blocked_writes: Arc<std::sync::Mutex<Vec<crate::task::subagent::BlockedWrite>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let blocked_writes_for_result = Arc::clone(&child_blocked_writes);
    let child_agent_terminals = context.agent_terminals.clone();
    let child_is_global = context.is_global;
    let child_orchestrator_host = context.orchestrator_host.clone();
    // Sub-agents share the parent's tracker handle and snapshot anchor.
    // Their edits/bashes belong to the same revert point as the parent's,
    // so a `/rewind` to the parent's user message rolls back everything
    // the sub-agent did too.
    let child_file_history = context.file_history.clone();
    let child_sweep_worker = context.sweep_worker.clone();
    let child_user_message_id = context.current_user_message_id.clone();

    tokio::spawn(async move {
        use crate::task::executor::TaskExecutor;
        use crate::provider::{Message, Role, ContentBlock};

        // Create a child event channel that forwards text events to parent
        let (child_event_tx, mut child_event_rx) =
            tokio::sync::mpsc::channel::<TaskEvent>(crate::EVENT_CHANNEL_CAP);

        // Forward sub-agent text/thinking deltas to parent as SubagentTextDelta
        let fwd_parent_tx = parent_event_tx.clone();
        let fwd_task_id = parent_task_id.clone();
        let fwd_agent_id = agent_id_clone.clone();
        tracing::warn!("[subagent] Starting event forwarder for '{}'", fwd_agent_id);
        tokio::spawn(async move {
            let mut event_count = 0u64;
            while let Some(event) = child_event_rx.recv().await {
                event_count += 1;
                if event_count <= 5 || event_count % 50 == 0 {
                    tracing::warn!("[subagent] '{}' event #{}: {:?}", fwd_agent_id, event_count,
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
                        let _ = fwd_parent_tx.try_send(TaskEvent::SubagentTextDelta {
                            task_id: fwd_task_id.clone(),
                            agent_id: fwd_agent_id.clone(),
                            text,
                        });
                    }
                    TaskEvent::ThinkingDelta { text, .. } => {
                        let _ = fwd_parent_tx.try_send(TaskEvent::SubagentTextDelta {
                            task_id: fwd_task_id.clone(),
                            agent_id: fwd_agent_id.clone(),
                            text: format!("[thinking] {}", text),
                        });
                    }
                    TaskEvent::ToolUse { tool_name, tool_use_id, tool_input, .. } => {
                        let _ = fwd_parent_tx.try_send(TaskEvent::SubagentToolUse {
                            task_id: fwd_task_id.clone(),
                            agent_id: fwd_agent_id.clone(),
                            tool_name,
                            tool_use_id,
                            input: tool_input,
                        });
                    }
                    TaskEvent::ToolResult { tool_use_id, output, is_error, .. } => {
                        let _ = fwd_parent_tx.try_send(TaskEvent::SubagentToolResult {
                            task_id: fwd_task_id.clone(),
                            agent_id: fwd_agent_id.clone(),
                            tool_use_id,
                            content: output,
                            is_error,
                        });
                    }
                    TaskEvent::CostUpdate { cost, .. } => {
                        let _ = fwd_parent_tx.try_send(TaskEvent::SubagentCostUpdate {
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
                        let _ = fwd_parent_tx.try_send(TaskEvent::PermissionRequest {
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
            persist_messages_fn: None,
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
            parent_provider_config: None, // sub-agents cannot spawn further sub-agents
            subagent_provider_config: None,
            write_scope: Some(child_write_scope),
            blocked_writes: child_blocked_writes,
            agent_terminals: child_agent_terminals,
            is_global: child_is_global,
            orchestrator_host: child_orchestrator_host,
            file_history: child_file_history,
            sweep_worker: child_sweep_worker,
            current_user_message_id: child_user_message_id,
            // Sub-agents get a fresh sink; the parent doesn't double-count
            // child media costs. (Sub-agents currently cannot call media
            // tools — none of `image_create` / `video_create` / `animate`
            // are in the sub-agent allowlist — so this sink stays at 0,
            // but we wire it for shape consistency.)
            tool_cost_sink: std::sync::Arc::new(std::sync::Mutex::new(0.0)),
        };

        let executor = TaskExecutor::new(provider, sub_config);
        let mut messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text { text: prompt }],
        }];

        tracing::warn!("[subagent] '{}' starting run_turn...", agent_id_clone);
        let result = executor.run_turn(&mut messages, &child_context).await;
        tracing::warn!("[subagent] '{}' run_turn finished: {}", agent_id_clone, if result.is_ok() { "OK" } else { "ERROR" });

        // ── Compute the summary first (fast — just reads memory) ─────────
        // The diff computation that used to follow `run_turn` is a
        // filesystem walk against the project snapshot — for non-trivial
        // workspaces it can take hundreds of ms, sometimes seconds. While
        // it ran, the sub-agent's spinner kept spinning even though the
        // model was already done generating, because `SubagentCompleted`
        // was deferred until after the diff. We now emit the completion
        // event *immediately* so the UI flips out of "running"; the diff
        // is computed afterwards and folded into the `SubagentResult` for
        // the orchestrator's bookkeeping.
        let summary = match &result {
            Ok(_) => {
                // The sub-agent's final assistant text IS the summary returned
                // to the parent. Walk backwards through assistant messages and
                // collect every non-empty text block from the most recent
                // assistant turn that produced text. Covers:
                //   - text → tool_use → text (concatenates both texts)
                //   - last turn = bare tool_use (skips it, keeps walking)
                //   - answer emitted earlier (still found via walk-back)
                let mut texts: Vec<String> = Vec::new();
                for m in messages.iter().rev() {
                    if !matches!(m.role, Role::Assistant) {
                        continue;
                    }
                    let mut msg_texts: Vec<String> = Vec::new();
                    for b in m.content.iter() {
                        if let ContentBlock::Text { text } = b {
                            let t = text.trim();
                            if !t.is_empty() {
                                msg_texts.push(t.to_string());
                            }
                        }
                    }
                    if !msg_texts.is_empty() {
                        // Take only ONE assistant turn worth of text — earlier
                        // turns typically contain in-progress chatter ("I'll
                        // read these files now"), not the final answer.
                        texts = msg_texts;
                        break;
                    }
                }
                let raw = if texts.is_empty() {
                    "Sub-agent completed without producing any text response. \
                     The model may have ended its run with a bare tool call. \
                     Re-spawn with a more explicit prompt asking for a written \
                     summary of what was done."
                        .to_string()
                } else {
                    texts.join("\n\n")
                };
                // Cap runaway summaries to keep the parent's context budget
                // sane, but make the cap generous — research/analysis sub-agents
                // legitimately produce multi-thousand-char deliverables. 32000
                // ≈ 8k words, plenty for any single sub-agent's output.
                const SUMMARY_CAP: usize = 32_000;
                if raw.len() > SUMMARY_CAP {
                    format!("{}…", &raw[..SUMMARY_CAP])
                } else {
                    raw
                }
            }
            Err(e) => {
                let err = format!("Sub-agent error: {}", e);
                tracing::warn!("[subagent] '{}' FAILED: {}", agent_id_clone, err);
                registry.fail(&parent_task_id, &agent_id_clone, err.clone());
                let _ = parent_event_tx.try_send(TaskEvent::SubagentFailed {
                    task_id: parent_task_id.clone(),
                    agent_id: agent_id_clone.clone(),
                    error: err,
                });
                return;
            }
        };

        // Fire the user-visible completion event NOW, before any slow
        // post-processing. The spinner on the sub-agent card stops the
        // moment this lands on the frontend.
        tracing::warn!("[subagent] '{}' completed successfully, summary len={}", agent_id_clone, summary.len());
        let _ = parent_event_tx.try_send(TaskEvent::SubagentCompleted {
            task_id: parent_task_id.clone(),
            agent_id: agent_id_clone.clone(),
            summary: summary.clone(),
        });

        let blocked_on = blocked_writes_for_result
            .lock()
            .map(|mut v| std::mem::take(&mut *v))
            .unwrap_or_default();

        let sub_result = SubagentResult {
            agent_id: agent_id_clone.clone(),
            model: String::new(),
            summary,
            notes: None,
            blocked_on,
        };
        registry.complete(&parent_task_id, sub_result);
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

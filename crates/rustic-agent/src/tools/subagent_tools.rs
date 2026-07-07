use crate::provider::claude::ClaudeProvider;
use crate::provider::compatible::CompatibleProvider;
use crate::provider::freebuff::FreeBuffProvider;
use crate::provider::gemini::GeminiProvider;
use crate::provider::openai::OpenAiProvider;
use crate::provider::ToolDef;
use crate::provider::{AiProvider, ContentBlock, Message, ProviderConfig, Role};
use crate::task::subagent::SubagentResult;
use crate::task::TaskEvent;
use crate::tools::{coerce_batch_array, ToolContext, ToolOutput};
use anyhow::Result;
use serde_json::{json, Value};
use std::sync::Arc;

/// Approximate cap (rendered characters) for the parent transcript that
/// `spawn_subagent` injects into a child's initial message when
/// `inherit_context` is true. When the flattened transcript exceeds this,
/// the oldest tool_result block contents are dropped first (preserving
/// text/thinking + tool_use + the most recent tool_results). Text/thinking
/// blocks are never truncated — the parent's reasoning is small and
/// valuable; tool_result bodies are the bulk of the cost. 30k chars ≈ 7-8k
/// tokens; the parent's full chat usually dwarfs this, so we trim from the
/// top.
const PARENT_CONTEXT_CHAR_CAP: usize = 30_000;

/// Truncate a UTF-8 string to at most `max_bytes` bytes without splitting a codepoint.
fn truncate_utf8(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Build a sub-agent's provider from a provider-TYPE key (the same strings
/// `ProviderEntry::provider_key()` and the main task builder in
/// `commands/agent/mod.rs` use), mirroring that builder exactly. Falls back to
/// the legacy model-name heuristic only when no type is known (e.g. a context
/// that didn't thread one). Routing by type is required because FreeBuff /
/// OpenRouter / Compatible all use `vendor/model` ids that can't be told apart
/// by name — name-guessing sent FreeBuff sub-agents through CompatibleProvider,
/// which 401'd for a missing OpenAI key.
pub(crate) fn provider_for_subagent(
    provider_type: Option<&str>,
    model: &str,
) -> Arc<dyn AiProvider> {
    if let Some(pt) = provider_type {
        if pt == "Claude" {
            return Arc::new(ClaudeProvider::new());
        } else if pt == "OpenAi" {
            return Arc::new(OpenAiProvider::new());
        } else if pt == "Gemini" {
            return Arc::new(GeminiProvider::new());
        } else if pt == "FreeBuff" {
            return Arc::new(FreeBuffProvider::new());
        } else if pt == "Compatible" || pt.starts_with("Compatible:") || pt == "OpenRouter" {
            return Arc::new(CompatibleProvider::new(pt.to_string()));
        }
    }
    // No provider type available — fall back to guessing from the model id.
    let model_lower = model.to_lowercase();
    if model_lower.starts_with("claude") {
        Arc::new(ClaudeProvider::new())
    } else if model_lower.starts_with("gpt")
        || model_lower.starts_with("o1")
        || model_lower.starts_with("o3")
    {
        Arc::new(OpenAiProvider::new())
    } else if model_lower.starts_with("gemini") {
        Arc::new(GeminiProvider::new())
    } else {
        Arc::new(CompatibleProvider::new("Compatible".to_string()))
    }
}

/// Flatten the parent's message list into a single readable transcript
/// suitable for prepending to a sub-agent's first user message. Each block
/// becomes one labelled paragraph. tool_result outputs longer than 4k chars
/// get a short ellipsis tail so a single huge file read doesn't bury the
/// rest of the history — the cap-driven trimming below then drops oldest
/// tool_results entirely when total length still exceeds PARENT_CONTEXT_CHAR_CAP.
fn render_parent_transcript(messages: &[Message]) -> String {
    fn shorten_tool_result(s: &str) -> String {
        const PER_RESULT_CAP: usize = 4_000;
        if s.chars().count() <= PER_RESULT_CAP {
            return s.to_string();
        }
        let cut = s
            .char_indices()
            .nth(PER_RESULT_CAP)
            .map(|(i, _)| i)
            .unwrap_or(s.len());
        format!(
            "{}…\n[truncated — full output {} chars]",
            &s[..cut],
            s.chars().count()
        )
    }

    // Tag each rendered chunk with whether it's a tool_result (eligible for
    // trim-from-top) or sticky content (text/thinking/tool_use — always
    // kept). Order is preserved on output regardless.
    enum Chunk {
        Sticky(String),
        ToolResult(String),
    }
    let mut chunks: Vec<Chunk> = Vec::new();

    for m in messages {
        let role_label = match m.role {
            Role::User => "User",
            Role::Assistant => "Assistant",
            Role::System => "System",
        };
        for block in &m.content {
            match block {
                ContentBlock::Text { text } => {
                    let t = text.trim();
                    if !t.is_empty() {
                        chunks.push(Chunk::Sticky(format!("[{}] {}", role_label, t)));
                    }
                }
                ContentBlock::Thinking { thinking, .. } => {
                    let t = thinking.trim();
                    if !t.is_empty() {
                        chunks.push(Chunk::Sticky(format!(
                            "[{} — internal reasoning] {}",
                            role_label, t
                        )));
                    }
                }
                ContentBlock::ToolUse { name, input, .. } => {
                    let input_str =
                        serde_json::to_string(input).unwrap_or_else(|_| "<unserialisable>".into());
                    chunks.push(Chunk::Sticky(format!(
                        "[Tool call: {}] {}",
                        name, input_str
                    )));
                }
                ContentBlock::ToolResult {
                    content, is_error, ..
                } => {
                    let label = if *is_error {
                        "Tool error"
                    } else {
                        "Tool result"
                    };
                    chunks.push(Chunk::ToolResult(format!(
                        "[{}] {}",
                        label,
                        shorten_tool_result(content)
                    )));
                }
                ContentBlock::Image { .. }
                | ContentBlock::RedactedThinking { .. }
                | ContentBlock::ModelSwitch { .. } => {
                    // Skip: images would explode the text payload, redacted
                    // thinking has no human-readable content, and ModelSwitch
                    // is a UI-only marker.
                }
            }
        }
    }

    // Compute total and trim oldest tool_results until under cap.
    let total = |cs: &[Chunk]| -> usize {
        cs.iter()
            .map(|c| match c {
                Chunk::Sticky(s) => s.len() + 2,
                Chunk::ToolResult(s) => s.len() + 2,
            })
            .sum()
    };

    let mut dropped_count = 0;
    while total(&chunks) > PARENT_CONTEXT_CHAR_CAP {
        // Find the FIRST tool_result (oldest) and drop it.
        let idx = chunks
            .iter()
            .position(|c| matches!(c, Chunk::ToolResult(_)));
        match idx {
            Some(i) => {
                chunks.remove(i);
                dropped_count += 1;
            }
            None => break, // No more tool_results to drop; sticky content alone is over cap.
        }
    }

    let mut out = String::new();
    if dropped_count > 0 {
        out.push_str(&format!(
            "[Note: {} oldest tool_result block(s) dropped to fit context cap]\n\n",
            dropped_count
        ));
    }
    for c in chunks {
        let s = match c {
            Chunk::Sticky(s) => s,
            Chunk::ToolResult(s) => s,
        };
        out.push_str(&s);
        out.push_str("\n\n");
    }
    out
}

pub fn definitions(fast_model: Option<&str>) -> Vec<ToolDef> {
    let has_fast = fast_model.is_some();
    let fast_label = fast_model.unwrap_or("");

    let base_description = "Delegate a task to a sub-agent. Best used for read-only exploration or \
                          research that returns a summary (the child spends its own context reading and \
                          hands you back conclusions), or for a self-contained chunk of work that is \
                          independent of every decision you're making and touches files you won't touch. \
                          Do NOT split interdependent coding work across sub-agents — do that yourself, in order. \
                          IMPORTANT: Delegate the TASK, not the solution — tell the sub-agent WHAT to \
                          accomplish, not the exact content to write. Do NOT pre-read files or generate \
                          content yourself to pass in the prompt. The sub-agent has full tool access. \
                          Sub-agents run ASYNCHRONOUSLY — this call returns immediately. Continue with \
                          other useful work after spawning; results are auto-injected as a \
                          `[Sub-agent '<id>' completed]` block on your next turn when each finishes. \
                          If you have nothing else to do, just end your turn — the executor parks the \
                          task and resumes it when results arrive. \
                          Only the main agent can spawn sub-agents (depth limit: 1). \
                          Declare `writes` for any files the sub-agent will modify — spawning a \
                          sub-agent whose writes collide with an already-running one is rejected. \
                          Max concurrent sub-agents: configurable in Settings → Budget (default 10). \
                          \
                          BATCH MODE (P1.13): to launch several sub-agents in one tool call, pass \
                          `agents: [...]` where each entry has the same fields you'd put in a single \
                          spawn (name, prompt, optional writes, optional reads, and `model_tier` if \
                          required). All entries are validated up-front; if any entry fails validation \
                          the whole batch is rejected. The output lists one agent_id per spawned entry \
                          (in input order) plus any per-entry rejection reasons (e.g. one entry \
                          collides with an already-running sibling).";
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
        format!(
            "{} The sub-agent inherits the same model as the main agent.",
            base_description
        )
    };

    let mut props = serde_json::Map::new();
    if has_fast {
        props.insert(
            "model_tier".to_string(),
            json!({
                "type": "string",
                "enum": ["intelligent", "fast"],
                "description": format!(
                    "Which model the sub-agent should use. \"intelligent\" = the main chat \
                     model (best for reasoning-heavy work). \"fast\" = the cheaper/faster model \
                     configured in settings ({}), good for tool-driven mechanical work. Pick \
                     based on the task's reasoning load, not its length.",
                    fast_label
                ),
            }),
        );
    }
    props.insert(
        "name".to_string(),
        json!({
            "type": "string",
            "description": "A short (3-5 word) name for this sub-agent. Used as the \
                            agent's display name and ID. E.g. 'refactor auth module', \
                            'write unit tests', 'fix login bug'."
        }),
    );
    props.insert(
        "prompt".to_string(),
        json!({
            "type": "string",
            "description": "The task description for the sub-agent. Describe WHAT to do and \
                            WHERE (file paths, directories), but do NOT include file contents \
                            or pre-generated code. The sub-agent will read files and generate \
                            content itself using its tools."
        }),
    );
    props.insert(
        "writes".to_string(),
        json!({
            "type": "array",
            "items": { "type": "string" },
            "description": "File or directory paths (repo-relative) this sub-agent will \
                            create, edit, or delete. Used to detect collisions with other \
                            running sub-agents. Leave empty for read-only tasks (research, \
                            analysis, summarization). Directory entries cover everything \
                            beneath them. Be tight — over-declaring serializes agents that \
                            could have run in parallel."
        }),
    );
    props.insert(
        "reads".to_string(),
        json!({
            "type": "array",
            "items": { "type": "string" },
            "description": "Optional: file or directory paths the sub-agent will read. \
                            Informational only; reads never cause collisions."
        }),
    );
    props.insert("project_root".to_string(), json!({
        "type": "string",
        "description": "Optional absolute path to a directory the sub-agent should treat as its \
                        project root instead of inheriting the parent's — e.g. another checkout \
                        you have access to. Omit for sub-agents that work in the parent's tree."
    }));
    props.insert("inherit_context".to_string(), json!({
        "type": "boolean",
        "description": "Whether the sub-agent receives a flattened transcript of YOUR \
                        conversation so far as background context (default: true). When true, \
                        the child can reuse the files / search results you've already loaded \
                        instead of re-reading them — cheaper and faster for delegating sub-tasks \
                        on the same body of code. Set to false for sub-agents whose work is \
                        unrelated to your current context (e.g. researching a separate library, \
                        running a self-contained refactor) — saves the inheritance token cost. \
                        Very large parent transcripts are auto-truncated by dropping the oldest \
                        tool_result blocks first."
    }));
    props.insert(
        "agents".to_string(),
        json!({
            "type": "array",
            "description": "Batch mode: launch N sub-agents in one call. Each entry uses the same \
                            shape as a top-level single spawn. Mutually exclusive with the \
                            top-level `name`/`prompt` fields. Empty array is an error.",
            "items": {
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "prompt": { "type": "string" },
                    "writes": { "type": "array", "items": { "type": "string" } },
                    "reads":  { "type": "array", "items": { "type": "string" } },
                    "model_tier": { "type": "string", "enum": ["intelligent", "fast"] },
                    "inherit_context": { "type": "boolean" }
                },
                "required": ["prompt"]
            }
        }),
    );

    let required: Vec<&str> = if has_fast {
        vec!["model_tier"]
    } else {
        Vec::new()
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
            name: "escalate_question".to_string(),
            description: "SUB-AGENT ONLY. Escalate a blocking question to your parent orchestrator \
                          and PAUSE until it replies (via send_message). Use this when you genuinely \
                          cannot proceed without a decision you lack the authority or information to \
                          make — ambiguous requirements, conflicting instructions, missing credentials. \
                          Ask sparingly: one clear, self-contained question including the options you \
                          see and your recommendation. If no answer arrives within 24h the call times \
                          out — proceed with your best judgment and record the open question in your \
                          closing summary.".to_string(),
            parameters: json!({
                "type": "object",
                "required": ["question"],
                "properties": {
                    "question": {
                        "type": "string",
                        "description": "The question, self-contained: what you're blocked on, the options you considered, and your recommendation."
                    }
                }
            }),
        },
        ToolDef {
            name: "list_subagents".to_string(),
            description: "List every sub-agent you've spawned in this task with their live state: \
                          status (running/completed/failed), model in use, turn count so far, \
                          cumulative estimated cost, and last recorded action. Non-blocking — sub-agent \
                          completions are auto-injected as `[Sub-agent '<id>' completed]` blocks on your \
                          next turn; you don't need to poll. End your turn if you have nothing else \
                          to do and the executor will park the task until results arrive.".to_string(),
            parameters: json!({ "type": "object", "properties": {} }),
        },
        ToolDef {
            name: "check_subagent".to_string(),
            description: "Inspect what a specific sub-agent is actually doing — the last N entries \
                          of its activity stream (text it wrote, tool calls it made, tool results \
                          it received, and any messages you queued via `send_message` / \
                          `nudge_subagent`). Unlike `list_subagents` which only shows the single \
                          `last_action` name, this gives you the full recent transcript so you can \
                          tell whether a child is making progress, looping, or drifting off-task. \
                          Defaults to the last 10 entries. Read-only and cheap — call it whenever \
                          you'd otherwise be tempted to assume what a child is up to.".to_string(),
            parameters: json!({
                "type": "object",
                "required": ["agent_id"],
                "properties": {
                    "agent_id": {
                        "type": "string",
                        "description": "Sub-agent id (from `spawn_subagent` / `list_subagents`)."
                    },
                    "tail": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 200,
                        "description": "How many of the most-recent activity entries to return. \
                                         Defaults to 10. Each entry is one of: assistant text, tool \
                                         call, tool result, or an orchestrator message you sent."
                    }
                }
            }),
        },
        ToolDef {
            name: "send_message".to_string(),
            description: "Queue a message for a running sub-agent. The sub-agent picks up the \
                          message at its next turn boundary (between tool calls / model turns — \
                          not as an interrupt). Useful when you want to feed the sub-agent extra \
                          context the orchestrator just learned, redirect its task, or correct a \
                          mis-step. Returns immediately; check `list_subagents` to see whether \
                          the sub-agent has acted on it. If you need the sub-agent to react NOW \
                          rather than at its next natural pause, use `nudge_subagent` instead.".to_string(),
            parameters: json!({
                "type": "object",
                "required": ["agent_id", "content"],
                "properties": {
                    "agent_id": {
                        "type": "string",
                        "description": "The sub-agent id (the id returned by `spawn_subagent` / \
                                         shown by `list_subagents`)."
                    },
                    "content": {
                        "type": "string",
                        "description": "What you want to tell the sub-agent. Plain text; framed \
                                         as orchestrator speech in the sub-agent's prompt."
                    }
                }
            }),
        },
        ToolDef {
            name: "nudge_subagent".to_string(),
            description: "Inject a steering hint into a running sub-agent. Unlike `send_message`, \
                          a nudge is framed as a system-level directive rather than orchestrator \
                          speech — use it for course corrections that should take precedence over \
                          the original task (e.g. \"stop reading files, just summarize what you \
                          have\", \"focus on src/auth/ only\"). Still consumed at the next turn \
                          boundary, not mid-tool-call.".to_string(),
            parameters: json!({
                "type": "object",
                "required": ["agent_id", "hint"],
                "properties": {
                    "agent_id": { "type": "string", "description": "Target sub-agent id." },
                    "hint": {
                        "type": "string",
                        "description": "Short imperative directive. Examples: \"stop reading, \
                                         summarize\", \"only edit files under tests/\"."
                    }
                }
            }),
        },
        ToolDef {
            name: "stop_subagent".to_string(),
            description: "Cancel a running sub-agent. Flips a cancel flag the sub-agent's executor \
                          watches between iterations — it stops at the next safe point (after the \
                          current tool batch completes), not mid-tool. Optionally records a reason \
                          string that will appear in the sub-agent's completion notes.".to_string(),
            parameters: json!({
                "type": "object",
                "required": ["agent_id"],
                "properties": {
                    "agent_id": { "type": "string", "description": "Sub-agent id to stop." },
                    "reason": {
                        "type": "string",
                        "description": "Optional short reason. Appended to the sub-agent's final \
                                         report so the orchestrator (and the user) can see why it \
                                         was cut short."
                    }
                }
            }),
        },
        // `wait_for_subagents` was removed in P1.9; completions are auto-injected at turn boundaries.
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
    match name {
        "spawn_subagent" => spawn_subagent(params, context).await,
        "list_subagents" => list_subagents(context).await,
        "check_subagent" => check_subagent(params, context).await,
        "report_blocked_write" => report_blocked_write(params, context).await,
        "send_message" => send_message(params, context).await,
        "escalate_question" => escalate_question(params, context).await,
        "nudge_subagent" => nudge_subagent(params, context).await,
        "stop_subagent" => stop_subagent(params, context).await,
        "wait_for_subagents" => Ok(ToolOutput {
            // legacy name — returns removal explanation
            content: "wait_for_subagents was removed in P1.9. Sub-agents now run \
                      asynchronously — their results are auto-injected into your next \
                      turn as soon as they complete. If you have no other useful work \
                      to do, simply end your turn; the executor will park and resume \
                      you when the next sub-agent finishes. Use `list_subagents` if \
                      you want to inspect current status."
                .into(),
            is_error: true,
            attachments: Vec::new(),
        }),
        _ => Ok(ToolOutput {
            content: format!("Unknown tool: {}", name),
            is_error: true,
            attachments: Vec::new(),
        }),
    }
}

fn deny_if_subagent(context: &ToolContext, tool: &str) -> Option<ToolOutput> {
    if context.agent_depth >= 1 {
        Some(ToolOutput {
            content: format!(
                "PERMISSION_DENIED: `{}` is callable only by the main agent — sub-agents \
                 cannot manage other sub-agents.",
                tool
            ),
            is_error: true,
            attachments: Vec::new(),
        })
    } else {
        None
    }
}

/// Child→parent escalation: register the question, wake the parent, and block
/// (up to 24h) until the parent's `send_message` delivers the answer.
async fn escalate_question(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    let Some((parent_task_id, agent_id)) = context.subagent_self.clone() else {
        return Ok(ToolOutput {
            content: "escalate_question is only available to sub-agents. As the main agent, \
                      use ask_user to ask the user directly."
                .into(),
            is_error: true,
            attachments: Vec::new(),
        });
    };
    let question = params["question"].as_str().unwrap_or("").trim().to_string();
    if question.is_empty() {
        return Ok(ToolOutput {
            content: "`question` must be a non-empty string.".into(),
            is_error: true,
            attachments: Vec::new(),
        });
    }
    let rx =
        match context
            .subagent_registry
            .register_escalation(&parent_task_id, &agent_id, &question)
        {
            Ok(rx) => rx,
            Err(e) => {
                return Ok(ToolOutput {
                    content: format!("ESCALATION_FAILED: {}", e),
                    is_error: true,
                    attachments: Vec::new(),
                });
            }
        };
    match tokio::time::timeout(std::time::Duration::from_secs(86_400), rx).await {
        Ok(Ok(answer)) => Ok(ToolOutput {
            content: format!("[Answer from orchestrator]\n{}", answer),
            is_error: false,
            attachments: Vec::new(),
        }),
        Ok(Err(_)) => Ok(ToolOutput {
            content: "ESCALATION_FAILED: the escalation channel closed without an answer. \
                      Proceed with your best judgment and record the open question in your \
                      closing summary."
                .into(),
            is_error: true,
            attachments: Vec::new(),
        }),
        Err(_) => {
            // Clean up the stale sender so a late parent reply gets a clear
            // "not delivered" instead of a silent drop.
            let _ = context.subagent_registry.answer_escalation(
                &parent_task_id,
                &agent_id,
                String::new(),
            );
            Ok(ToolOutput {
                content: "ESCALATION_TIMEOUT: no answer after 24h. Proceed with your best \
                          judgment and record the open question in your closing summary."
                    .into(),
                is_error: true,
                attachments: Vec::new(),
            })
        }
    }
}

async fn send_message(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    if let Some(out) = deny_if_subagent(context, "send_message") {
        return Ok(out);
    }
    let agent_id = params["agent_id"].as_str().unwrap_or("").trim();
    let content = params["content"].as_str().unwrap_or("").trim();
    if agent_id.is_empty() || content.is_empty() {
        return Ok(ToolOutput {
            content: "`agent_id` and `content` must both be non-empty strings.".into(),
            is_error: true,
            attachments: Vec::new(),
        });
    }
    // A pending escalation takes priority: the child is BLOCKED inside its
    // escalate_question call, so the message is delivered as the answer
    // (the inbox would only be drained at the next turn boundary, which
    // never comes while the child is blocked).
    if context
        .subagent_registry
        .has_pending_escalation(&context.task_id, agent_id)
    {
        return Ok(
            if context.subagent_registry.answer_escalation(
                &context.task_id,
                agent_id,
                content.to_string(),
            ) {
                context.subagent_registry.record_orchestrator_message(
                    &context.task_id,
                    agent_id,
                    crate::task::subagent::InboxKind::User,
                    content,
                );
                ToolOutput {
                content: format!(
                    "Answer delivered to sub-agent `{}`'s pending escalation — it resumes immediately.",
                    agent_id
                ),
                is_error: false,
                attachments: Vec::new(),
            }
            } else {
                ToolOutput {
                content: format!(
                    "Sub-agent `{}` stopped waiting for the escalation answer (timed out or exited). \
                     Message NOT delivered.",
                    agent_id
                ),
                is_error: true,
                attachments: Vec::new(),
            }
            },
        );
    }
    match context.subagent_registry.push_inbox(
        &context.task_id,
        agent_id,
        crate::task::subagent::InboxKind::User,
        content.to_string(),
    ) {
        Ok(()) => {
            context.subagent_registry.record_orchestrator_message(
                &context.task_id,
                agent_id,
                crate::task::subagent::InboxKind::User,
                content,
            );
            Ok(ToolOutput {
                content: format!(
                    "Message queued for sub-agent `{}`. It will see your message at its next turn boundary.",
                    agent_id
                ),
                is_error: false,
                attachments: Vec::new(),
            })
        }
        Err(err) => Ok(ToolOutput {
            content: err,
            is_error: true,
            attachments: Vec::new(),
        }),
    }
}

async fn nudge_subagent(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    if let Some(out) = deny_if_subagent(context, "nudge_subagent") {
        return Ok(out);
    }
    let agent_id = params["agent_id"].as_str().unwrap_or("").trim();
    let hint = params["hint"].as_str().unwrap_or("").trim();
    if agent_id.is_empty() || hint.is_empty() {
        return Ok(ToolOutput {
            content: "`agent_id` and `hint` must both be non-empty strings.".into(),
            is_error: true,
            attachments: Vec::new(),
        });
    }
    match context.subagent_registry.push_inbox(
        &context.task_id,
        agent_id,
        crate::task::subagent::InboxKind::Nudge,
        hint.to_string(),
    ) {
        Ok(()) => {
            context.subagent_registry.record_orchestrator_message(
                &context.task_id,
                agent_id,
                crate::task::subagent::InboxKind::Nudge,
                hint,
            );
            Ok(ToolOutput {
                content: format!("Nudge queued for sub-agent `{}`.", agent_id),
                is_error: false,
                attachments: Vec::new(),
            })
        }
        Err(err) => Ok(ToolOutput {
            content: err,
            is_error: true,
            attachments: Vec::new(),
        }),
    }
}

async fn stop_subagent(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    if let Some(out) = deny_if_subagent(context, "stop_subagent") {
        return Ok(out);
    }
    let agent_id = params["agent_id"].as_str().unwrap_or("").trim();
    if agent_id.is_empty() {
        return Ok(ToolOutput {
            content: "`agent_id` is required.".into(),
            is_error: true,
            attachments: Vec::new(),
        });
    }
    let reason = params["reason"].as_str().unwrap_or("").trim();
    if !reason.is_empty() {
        let _ = context.subagent_registry.push_inbox(
            &context.task_id,
            agent_id,
            crate::task::subagent::InboxKind::Nudge,
            format!("STOP requested by orchestrator. Reason: {reason}. Wrap up with whatever you've completed."),
        );
    }
    let signalled = context.subagent_registry.cancel(&context.task_id, agent_id);
    if signalled {
        Ok(ToolOutput {
            content: format!(
                "Stop signal sent to sub-agent `{}`. It will exit at the next safe boundary (after \
                 the current tool batch).",
                agent_id
            ),
            is_error: false,
            attachments: Vec::new(),
        })
    } else {
        Ok(ToolOutput {
            content: format!(
                "Could not stop `{}` — either no such sub-agent or it had no cancel token wired \
                 (this can happen for sub-agents spawned before P1.6).",
                agent_id
            ),
            is_error: true,
            attachments: Vec::new(),
        })
    }
}

async fn list_subagents(context: &ToolContext) -> Result<ToolOutput> {
    let entries = context.subagent_registry.all_for_task(&context.task_id);
    if entries.is_empty() {
        return Ok(ToolOutput {
            content: "No sub-agents have been spawned for this task.".into(),
            is_error: false,
            attachments: Vec::new(),
        });
    }
    let mut out = format!("Sub-agents for this task ({}):\n", entries.len());
    for e in entries {
        let status_str = match e.status {
            crate::task::subagent::SubagentStatus::Running => "running",
            crate::task::subagent::SubagentStatus::Completed => "completed",
            crate::task::subagent::SubagentStatus::Failed => "failed",
        };
        out.push_str(&format!(
            "  - {} [{}] model={} turns={} cost≈${:.4}{}\n",
            e.agent_id,
            status_str,
            e.model,
            e.turn_count,
            e.cumulative_cost_usd,
            e.last_action
                .as_ref()
                .map(|a| format!(" last={}", a))
                .unwrap_or_default(),
        ));
    }
    Ok(ToolOutput {
        content: out,
        is_error: false,
        attachments: Vec::new(),
    })
}

async fn check_subagent(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    if let Some(out) = deny_if_subagent(context, "check_subagent") {
        return Ok(out);
    }
    let agent_id = params
        .get("agent_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if agent_id.is_empty() {
        return Ok(ToolOutput {
            content: "`agent_id` is required.".into(),
            is_error: true,
            attachments: Vec::new(),
        });
    }
    let tail = params
        .get("tail")
        .and_then(|v| v.as_i64())
        .filter(|n| *n > 0)
        .map(|n| n as usize)
        .unwrap_or(10);

    let Some(readout) = context
        .subagent_registry
        .read_activity(&context.task_id, agent_id)
    else {
        return Ok(ToolOutput {
            content: format!(
                "No sub-agent `{}` under this task. Call `list_subagents` to see valid ids.",
                agent_id
            ),
            is_error: true,
            attachments: Vec::new(),
        });
    };

    use crate::task::subagent::{ActivityKind, SubagentStatus};
    let status_str = match readout.status {
        SubagentStatus::Running => "running",
        SubagentStatus::Completed => "completed",
        SubagentStatus::Failed => "failed",
    };
    let mut out = format!(
        "Sub-agent `{}` [{}] model={} turns={} cost≈${:.4}",
        readout.agent_id,
        status_str,
        readout.model,
        readout.turn_count,
        readout.cumulative_cost_usd
    );
    if let Some(la) = &readout.last_action {
        out.push_str(&format!(" last={}", la));
    }
    out.push('\n');

    if readout.activity.is_empty() {
        out.push_str("\nNo recorded activity yet — sub-agent hasn't started streaming.\n");
        return Ok(ToolOutput {
            content: out,
            is_error: false,
            attachments: Vec::new(),
        });
    }

    let total = readout.total_activity;
    let start = total.saturating_sub(tail);
    let shown = total - start;
    out.push_str(&format!(
        "\nShowing last {} of {} activity entries (oldest first):\n",
        shown, total
    ));

    for (i, entry) in readout.activity.iter().enumerate().skip(start) {
        let kind_str = match entry.kind {
            ActivityKind::AssistantText => "text",
            ActivityKind::ToolCall => "tool_call",
            ActivityKind::ToolResult => "tool_result",
            ActivityKind::OrchestratorMessage => "orchestrator_message",
            ActivityKind::OrchestratorNudge => "orchestrator_nudge",
        };
        out.push_str(&format!("\n#{} [{}]\n{}\n", i + 1, kind_str, entry.content));
    }

    Ok(ToolOutput {
        content: out,
        is_error: false,
        attachments: Vec::new(),
    })
}

async fn report_blocked_write(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    if context.write_scope.is_none() {
        return Ok(ToolOutput {
            content:
                "report_blocked_write has no effect for the main agent — you have unrestricted \
                      write scope. If you hit a genuine permission failure, handle it directly."
                    .to_string(),
            is_error: false,
            attachments: Vec::new(),
        });
    }

    let path = params["path"].as_str().unwrap_or("").trim();
    let reason = params["reason"].as_str().unwrap_or("").trim();
    if path.is_empty() || reason.is_empty() {
        return Ok(ToolOutput {
            content:
                "report_blocked_write requires both `path` and `reason` to be non-empty strings."
                    .to_string(),
            is_error: true,
            attachments: Vec::new(),
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
        attachments: Vec::new(),
    })
}

/// Accepts single-spawn (`{name, prompt, writes, ...}`) or batch (`{agents:[...]}`).
/// Repairs Claude's tendency to emit nested array fields as JSON-encoded strings
/// (seen with extended thinking active) — emits WARN on coercion, errors on parse failure.
fn coerce_stringified_arrays(params: &mut Value) -> std::result::Result<(), String> {
    let Some(obj) = params.as_object_mut() else {
        return Ok(());
    };
    for field in &["agents", "writes", "reads"] {
        coerce_field_to_array(obj, field)?;
    }
    if let Some(Value::Array(entries)) = obj.get_mut("agents") {
        for (i, entry) in entries.iter_mut().enumerate() {
            if let Some(entry_obj) = entry.as_object_mut() {
                for field in &["writes", "reads"] {
                    coerce_field_to_array(entry_obj, field)
                        .map_err(|e| format!("agents[{}]: {}", i, e))?;
                }
            }
        }
    }
    Ok(())
}

fn coerce_field_to_array(
    obj: &mut serde_json::Map<String, Value>,
    field: &str,
) -> std::result::Result<(), String> {
    let Some(v) = obj.get(field) else {
        return Ok(());
    };
    if v.is_null() || v.is_array() {
        return Ok(());
    }
    let Some(s) = v.as_str() else {
        return Err(format!(
            "Field `{}` must be a JSON array. Got a {}. Pass it as an array directly, \
             e.g. `\"{}\": [\"item1\", \"item2\"]`.",
            field,
            value_type_name(v),
            field,
        ));
    };
    let trimmed = s.trim();
    if trimmed.is_empty() {
        obj.insert(field.to_string(), Value::Array(Vec::new()));
        return Ok(());
    }
    match serde_json::from_str::<Value>(trimmed) {
        Ok(parsed) if parsed.is_array() => {
            tracing::warn!(
                target: "rustic::stream",
                field = field,
                "[spawn] coerced stringified JSON array for `{}` — the model emitted a string but the schema declares an array; parsed and continuing",
                field,
            );
            obj.insert(field.to_string(), parsed);
            Ok(())
        }
        Ok(other) => Err(format!(
            "Field `{}` was sent as a JSON-string, but its contents parse to a {}, not an array. \
             Pass an array directly, e.g. `\"{}\": [\"item1\", \"item2\"]` — do NOT wrap it in quotes.",
            field,
            value_type_name(&other),
            field,
        )),
        Err(parse_err) => Err(format!(
            "Field `{}` was sent as a JSON-string but does not parse as a JSON array: {}. \
             Pass the array directly, e.g. `\"{}\": [\"item1\", \"item2\"]` — do NOT wrap it \
             in quotes or escape the inner quotes.",
            field, parse_err, field,
        )),
    }
}

fn value_type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

async fn spawn_subagent(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    if context.agent_depth >= 1 {
        return Ok(ToolOutput {
            content: "PERMISSION_DENIED: Sub-agents cannot spawn further sub-agents (max depth 1)."
                .to_string(),
            is_error: true,
            attachments: Vec::new(),
        });
    }

    // Repair stringified-array inputs in place BEFORE dispatch. Without this
    // step the model's tool call would either drop entire fields silently
    // (the batch dispatcher missing `agents`, or `writes` defaulting to empty
    // so the agent runs read-only) or fall through to single-mode with no
    // prompt. The coercion succeeds for the common "JSON inside a string"
    // case and returns a model-facing error for genuinely malformed input.
    let mut params = params;
    if let Err(coercion_err) = coerce_stringified_arrays(&mut params) {
        return Ok(ToolOutput {
            content: format!(
                "SPAWN_REJECTED: {}\n\n\
                 Retry the `spawn_subagent` call with the array fields emitted as native JSON \
                 arrays — not strings. Examples:\n\
                 - `\"writes\": [\"src/a.ts\", \"src/b.ts\"]`  (NOT `\"writes\": \"[\\\"src/a.ts\\\"]\"`)\n\
                 - `\"reads\":  [\"src/c.ts\"]`               (NOT `\"reads\":  \"[\\\"src/c.ts\\\"]\"`)\n\
                 - Batch: `\"agents\": [{{...}}, {{...}}]`     (NOT `\"agents\": \"[...]\"`)\n\
                 If you are wrapping arrays in quotes to satisfy a perceived schema \
                 requirement, stop — the schema accepts native arrays directly.",
                coercion_err,
            ),
            is_error: true,
            attachments: Vec::new(),
        });
    }

    if let Some(agents) = coerce_batch_array(params.get("agents")) {
        return spawn_subagent_batch(agents, context).await;
    }

    let mut agent_id_out: Option<String> = None;
    spawn_subagent_inner(params, context, &mut agent_id_out).await
}

async fn spawn_subagent_batch(agents: Vec<Value>, context: &ToolContext) -> Result<ToolOutput> {
    if agents.is_empty() {
        return Ok(ToolOutput {
            content: "BATCH_SPAWN_REJECTED: `agents` array is empty. Pass at least one entry, \
                      or use the single-agent shape `{ name, prompt, ... }`."
                .to_string(),
            is_error: true,
            attachments: Vec::new(),
        });
    }

    let mut errors: Vec<String> = Vec::new();
    for (i, entry) in agents.iter().enumerate() {
        let prompt = entry
            .get("prompt")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if prompt.is_empty() {
            errors.push(format!("entry[{}]: missing required `prompt`", i));
        }
        if let Some(name_val) = entry.get("name") {
            if !name_val.is_null() && name_val.as_str().is_none() {
                errors.push(format!("entry[{}]: `name` must be a string", i));
            }
        }
        if let Some(writes_val) = entry.get("writes") {
            if !writes_val.is_array() {
                errors.push(format!(
                    "entry[{}]: `writes` must be an array of strings",
                    i
                ));
            }
        }
        if let Some(model_tier_val) = entry.get("model_tier") {
            let v = model_tier_val.as_str().unwrap_or("");
            if !matches!(v, "intelligent" | "fast") {
                errors.push(format!(
                    "entry[{}]: `model_tier` must be \"intelligent\" or \"fast\"",
                    i
                ));
            }
        }
    }
    if !errors.is_empty() {
        return Ok(ToolOutput {
            content: format!(
                "BATCH_SPAWN_REJECTED: {} entry/entries failed validation. Nothing was spawned.\n{}",
                errors.len(),
                errors.join("\n")
            ),
            is_error: true,
            attachments: Vec::new(),
        });
    }

    let mut spawned_ids: Vec<(usize, String)> = Vec::new();
    let mut rejections: Vec<(usize, String)> = Vec::new();
    for (i, entry) in agents.into_iter().enumerate() {
        let mut agent_id_out: Option<String> = None;
        match spawn_subagent_inner(entry, context, &mut agent_id_out).await {
            Ok(output) => {
                if let Some(id) = agent_id_out {
                    spawned_ids.push((i, id));
                } else {
                    rejections.push((i, output.content));
                }
            }
            Err(e) => {
                rejections.push((i, format!("internal error: {}", e)));
            }
        }
    }

    let mut body = format!(
        "Batch spawn: {} requested, {} spawned, {} rejected.\n",
        spawned_ids.len() + rejections.len(),
        spawned_ids.len(),
        rejections.len()
    );
    if !spawned_ids.is_empty() {
        body.push_str("\nSpawned:\n");
        for (i, id) in &spawned_ids {
            body.push_str(&format!("  [{}] {}\n", i, id));
        }
    }
    if !rejections.is_empty() {
        body.push_str("\nRejected (these did NOT spawn — see reason):\n");
        for (i, reason) in &rejections {
            body.push_str(&format!("  [{}] {}\n", i, reason));
        }
        body.push_str(
            "\nThe spawned children above are running in parallel. Decide whether to retry \
             the rejected entries (after adjusting writes / waiting for finished siblings) \
             or proceed with what you have.\n",
        );
    } else {
        body.push_str(
            "\nAll children are running in parallel. Results are injected automatically \
             as each finishes — continue with other work, or end your turn if you have \
             nothing else to do (the executor parks the task until results arrive).\n",
        );
    }
    Ok(ToolOutput {
        content: body,
        is_error: rejections.len() == (spawned_ids.len() + rejections.len()),
        attachments: Vec::new(),
    })
}

async fn spawn_subagent_inner(
    params: Value,
    context: &ToolContext,
    out_agent_id: &mut Option<String>,
) -> Result<ToolOutput> {
    let spawn_start = std::time::Instant::now();
    let name = params["name"].as_str().unwrap_or("").to_string();
    let prompt = params["prompt"].as_str().unwrap_or("").to_string();
    let writes: Vec<String> = params
        .get("writes")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    // `inherit_context` defaults to true at the schema level; only honour an
    // explicit boolean override here. Captured into the spawned task below.
    let parent_inherit_context: Option<bool> =
        params.get("inherit_context").and_then(|v| v.as_bool());
    let parent_message_snapshot_arc = Arc::clone(&context.parent_message_snapshot);

    tracing::info!(
        target: "rustic::stream",
        parent_task = %context.task_id,
        requested_name = %name,
        prompt_chars = prompt.chars().count(),
        prompt_bytes = prompt.len(),
        writes_count = writes.len(),
        "[spawn] spawn_subagent_inner entered"
    );

    if prompt.is_empty() {
        return Ok(ToolOutput {
            content: "Missing required parameter: prompt".to_string(),
            is_error: true,
            attachments: Vec::new(),
        });
    }

    {
        let cap = context
            .ai_config
            .budget
            .max_concurrent_subagents
            .or(Some(crate::budget::DEFAULT_MAX_CONCURRENT_SUBAGENTS));
        if let Some(cap) = cap {
            let running = context.subagent_registry.running_count(&context.task_id);
            if running >= cap {
                return Ok(ToolOutput {
                    content: format!(
                        "SPAWN_REJECTED: {} sub-agents already running (max = {}). \
                         End your turn (or do other useful work) and respawn after the next \
                         `[Sub-agent '<id>' completed]` block is injected, or raise the cap \
                         in Settings → Budget.",
                        running, cap
                    ),
                    is_error: true,
                    attachments: Vec::new(),
                });
            }
        }
    }

    if let Some(conflicting) = context
        .subagent_registry
        .find_write_collision(&context.task_id, &writes)
    {
        return Ok(ToolOutput {
            content: format!(
                "SPAWN_REJECTED: write collision with running sub-agent '{}'. \
                 Its declared writes overlap with yours. Either wait for '{}' to \
                 finish (its `[Sub-agent '{}' completed]` block will be auto-injected) \
                 before respawning, or narrow your `writes` list so it doesn't overlap.",
                conflicting, conflicting, conflicting
            ),
            is_error: true,
            attachments: Vec::new(),
        });
    }

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
        let slug = truncate_utf8(&slug, 30).to_string();
        if slug.is_empty() {
            format!("agent-{}", &uuid::Uuid::new_v4().to_string()[..8])
        } else {
            slug
        }
    };

    let model_tier = params
        .get("model_tier")
        .and_then(|v| v.as_str())
        .unwrap_or("intelligent");
    let use_fast =
        model_tier.eq_ignore_ascii_case("fast") && context.subagent_provider_config.is_some();

    let chosen_config = if use_fast {
        // unwrap is safe: `use_fast` checked is_some() above.
        context.subagent_provider_config.as_ref().unwrap()
    } else {
        context.parent_provider_config.as_ref().ok_or_else(|| {
            anyhow::anyhow!("No parent provider config available for sub-agent spawning")
        })?
    };
    let model = chosen_config.model.clone();

    // Build the provider from the parent's (or fast-tier's) provider TYPE, which
    // travels in the context. Guessing from the model id is wrong for FreeBuff:
    // `mimo/mimo-v2.5` etc. look like OpenRouter/Compatible ids, so they used to
    // route through CompatibleProvider and fail with an OpenAI 401 (no key).
    let chosen_provider_type = if use_fast {
        context.subagent_provider_type.as_deref()
    } else {
        context.parent_provider_type.as_deref()
    };
    let provider: Arc<dyn AiProvider> = provider_for_subagent(chosen_provider_type, &model);

    // Sub-agents use a lean, dedicated prompt (~700 tokens) rather than the parent's
    // full prompt. The parent prompt's 10-20k-token cache prefix was causing
    // expensive cache_creation writes on every TTL boundary inside long sub-agent runs.
    let sub_system_prompt = crate::system_prompt::build_subagent_prompt();
    let _unused_parent_prompt = &chosen_config.system_prompt;

    // Thinking disabled for sub-agents (thinking_budget=0): extended reasoning on
    // mechanical lookup work is pure overhead and burned output tokens per call.
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
        supports_adaptive_thinking: chosen_config.supports_adaptive_thinking,
        cancel_token: context.cancel_token.clone(),
        custom_input_cost: chosen_config.custom_input_cost,
        custom_output_cost: chosen_config.custom_output_cost,
        custom_cache_read_cost: chosen_config.custom_cache_read_cost,
        custom_cache_write_cost: chosen_config.custom_cache_write_cost,
        // Sub-agents may run a different model than the parent; don't inherit a
        // per-model provider allow-list that may not serve this model.
        allowed_providers: None,
    };

    let child_cancel_token = Arc::new(std::sync::atomic::AtomicBool::new(false));
    context.subagent_registry.register(
        &context.task_id,
        &agent_id,
        &model,
        writes.clone(),
        Some(Arc::clone(&child_cancel_token)),
    );
    tracing::warn!(
        "[subagent] Registered '{}' under task '{}' with model '{}'",
        agent_id,
        context.task_id,
        model
    );
    tracing::info!(
        target: "rustic::stream",
        agent_id = %agent_id,
        parent_task = %context.task_id,
        model = %model,
        register_ms = spawn_start.elapsed().as_millis() as u64,
        "[spawn] registered in registry"
    );

    let _ = context.event_tx.try_send(TaskEvent::SubagentSpawned {
        task_id: context.task_id.clone(),
        agent_id: agent_id.clone(),
        model: model.clone(),
        prompt: prompt.clone(),
        name: name.clone(),
    });

    let parent_task_id = context.task_id.clone();
    let agent_id_clone = agent_id.clone();
    let registry = Arc::clone(&context.subagent_registry);
    let parent_event_tx = context.event_tx.clone();
    // project_root override: must be absolute; write_scope + file_lock still gate illegal writes.
    let explicit_root = params
        .get("project_root")
        .and_then(|v| v.as_str())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty());
    let child_project_root: std::path::PathBuf = match explicit_root {
        Some(s) => {
            let p = std::path::PathBuf::from(s);
            if !p.is_absolute() {
                return Ok(ToolOutput {
                    content: format!(
                        "SPAWN_REJECTED: `project_root` must be an absolute path; got '{}'. \
                     Pass an absolute path to an existing directory (or omit the field \
                     entirely to inherit the parent's project root).",
                        s
                    ),
                    is_error: true,
                    attachments: Vec::new(),
                });
            }
            if !p.exists() {
                return Ok(ToolOutput {
                    content: format!(
                        "SPAWN_REJECTED: `project_root` path does not exist: '{}'. Pass an \
                     absolute path to an existing directory.",
                        s
                    ),
                    is_error: true,
                    attachments: Vec::new(),
                });
            }
            p
        }
        None => context.project_root.clone(),
    };
    // F-20: sub-agents inherit the parent's permission level (never escalate to FullAuto).
    // broker is shared, so any prompt surfaces in the parent's approval UI.
    let parent_level = context.shared_permissions.level();
    let parent_sensitive = context.shared_permissions.sensitive_files_allowed();
    let child_shared_permissions =
        crate::task::permissions::SharedPermissions::new(parent_level, parent_sensitive);
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
    let child_is_plan_mode = context.is_plan_mode;
    let child_budget = context.budget.clone();
    let child_ask_user_broker = context.ask_user_broker.clone();
    let child_ceiling_broker = context.ceiling_broker.clone();
    // Children share the parent's tracker/snapshot so /rewind rolls
    // back their edits too.
    let (child_file_history, child_sweep_worker) =
        (context.file_history.clone(), context.sweep_worker.clone());
    // Inherit the parent's baseline gate (cloned out before the spawn so we
    // don't capture `context` by reference inside the 'static task).
    let child_baseline_gate = context.baseline_gate.clone();
    let child_user_message_id = context.current_user_message_id.clone();
    // Spawns with an explicit project_root get their own WorkspaceServices;
    // siblings into the same root share one bundle via the registry.
    let child_workspace_services = if child_project_root == context.project_root {
        Arc::clone(&context.workspace_services)
    } else {
        let ws = context
            .workspace_registry
            .get_or_create(&child_project_root);
        ws.ensure_index_build_started(); // idempotent; no-ops for sibling spawns into same root
        ws
    };
    let child_subagent_self = Some((context.task_id.clone(), agent_id.clone()));
    let model_for_result = model.clone();
    let child_loaded_deferred_tools = Arc::clone(&context.loaded_deferred_tools);
    let child_workspace_registry = Arc::clone(&context.workspace_registry);

    let spawn_dispatch_start = std::time::Instant::now();
    tokio::spawn(async move {
        use crate::provider::{ContentBlock, Message, Role};
        use crate::task::executor::TaskExecutor;

        tracing::info!(
            target: "rustic::stream",
            agent_id = %agent_id_clone,
            dispatch_lag_ms = spawn_dispatch_start.elapsed().as_millis() as u64,
            "[spawn] child tokio task started"
        );

        let (child_event_tx, mut child_event_rx) =
            tokio::sync::mpsc::channel::<TaskEvent>(crate::EVENT_CHANNEL_CAP);

        let fwd_parent_tx = parent_event_tx.clone();
        let fwd_task_id = parent_task_id.clone();
        let fwd_agent_id = agent_id_clone.clone();
        let fwd_registry = Arc::clone(&registry);
        tracing::debug!("[subagent] Starting event forwarder for '{}'", fwd_agent_id);
        tokio::spawn(async move {
            let mut event_count = 0u64;
            while let Some(event) = child_event_rx.recv().await {
                event_count += 1;
                if event_count <= 5 || event_count % 50 == 0 {
                    tracing::trace!(
                        "[subagent] '{}' event #{}: {:?}",
                        fwd_agent_id,
                        event_count,
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
                        fwd_registry.record_text_delta(&fwd_task_id, &fwd_agent_id, &text);
                        let _ = fwd_parent_tx.try_send(TaskEvent::SubagentTextDelta {
                            task_id: fwd_task_id.clone(),
                            agent_id: fwd_agent_id.clone(),
                            text,
                        });
                    }
                    TaskEvent::ThinkingDelta { text, .. } => {
                        let _ = fwd_parent_tx.try_send(TaskEvent::SubagentThinkingDelta {
                            task_id: fwd_task_id.clone(),
                            agent_id: fwd_agent_id.clone(),
                            text,
                        });
                    }
                    TaskEvent::ToolUse {
                        tool_name,
                        tool_use_id,
                        tool_input,
                        ..
                    } => {
                        fwd_registry.record_tool_call(
                            &fwd_task_id,
                            &fwd_agent_id,
                            &tool_name,
                            &tool_input,
                        );
                        let _ = fwd_parent_tx.try_send(TaskEvent::SubagentToolUse {
                            task_id: fwd_task_id.clone(),
                            agent_id: fwd_agent_id.clone(),
                            tool_name,
                            tool_use_id,
                            input: tool_input,
                        });
                    }
                    TaskEvent::ToolResult {
                        tool_use_id,
                        output,
                        is_error,
                        ..
                    } => {
                        fwd_registry.record_tool_result(
                            &fwd_task_id,
                            &fwd_agent_id,
                            &output,
                            is_error,
                        );
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
                    TaskEvent::PermissionRequest {
                        request_id,
                        operation,
                        description,
                        preview,
                        ..
                    } => {
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
            cancel_token: Some(child_cancel_token),
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
            parent_provider_type: None,
            subagent_provider_type: None,
            write_scope: Some(child_write_scope),
            blocked_writes: child_blocked_writes,
            agent_terminals: child_agent_terminals,
            is_plan_mode: child_is_plan_mode,
            budget: child_budget,
            ask_user_broker: child_ask_user_broker,
            // Sub-agents never carry the suspend slot — ask_user is stripped
            // from their tool pool entirely.
            ask_user_suspend: None,
            // Sub-agent context is ephemeral — condense drops are not archived.
            conversation_archive: None,
            ceiling_broker: child_ceiling_broker,
            file_history: child_file_history,
            sweep_worker: child_sweep_worker,
            // Sub-agents mutate within the parent's snapshot, so they wait on the
            // parent's baseline gate (already resolved by the time a sub-agent runs).
            baseline_gate: child_baseline_gate,
            current_user_message_id: child_user_message_id,
            // Sub-agents get a fresh sink; the parent doesn't double-count
            // child media costs. (Sub-agents currently cannot call media
            // tools — none of `image_create` / `video_create` / `animate`
            // are in the sub-agent allowlist — so this sink stays at 0,
            // but we wire it for shape consistency.)
            tool_cost_sink: std::sync::Arc::new(std::sync::Mutex::new(
                crate::tools::ToolCostBucket::default(),
            )),
            workspace_services: child_workspace_services,
            subagent_self: child_subagent_self,
            // Fresh slot — a sub-agent's todo list is its own, not the parent's.
            current_todos: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            goal_state: None,
            loaded_deferred_tools: child_loaded_deferred_tools,
            deferred_tools: Arc::new(std::sync::Mutex::new(Vec::new())),
            workspace_registry: child_workspace_registry,
            // Sub-agents don't propagate context further — they can't spawn
            // sub-agents of their own, so an empty Vec is fine. If that
            // invariant ever loosens, this would need to carry the parent's
            // snapshot through (and we'd want a size guard so depth-2 doesn't
            // explode the context).
            parent_message_snapshot: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        };

        let executor = TaskExecutor::new(provider, sub_config);

        // Inherit-context: default ON. When the model passed
        // `inherit_context: false` on this entry we skip the prefix and the
        // child starts with just its prompt (cheaper, useful for unrelated
        // sub-tasks). Otherwise we grab the snapshot the parent's executor
        // wrote into ToolContext right before tool dispatch and prepend a
        // single synthetic user message containing the rendered transcript.
        // The actual spawn prompt follows as the next user message so the
        // child sees a real two-turn conversation: "here's the background"
        // → "your task".
        let inherit_context = parent_inherit_context.unwrap_or(true);
        let mut messages: Vec<Message> = Vec::new();
        if inherit_context {
            let parent_snapshot: Vec<Message> = parent_message_snapshot_arc
                .lock()
                .ok()
                .map(|g| g.clone())
                .unwrap_or_default();
            if !parent_snapshot.is_empty() {
                let transcript = render_parent_transcript(&parent_snapshot);
                if !transcript.trim().is_empty() {
                    let preface = format!(
                        "You are a sub-agent spawned by a parent agent that has been working \
                         on a related task. Below is the parent's conversation transcript so \
                         far, included as background context so you don't have to re-read \
                         files or re-run searches the parent already did.\n\n\
                         Use this as reference — if something you need is already in this \
                         transcript (file contents, search results, decisions made), work \
                         from there rather than calling tools to fetch it again. If you need \
                         information not covered below, run the tools normally.\n\n\
                         ## Parent transcript\n\n{}\n\n\
                         ## End of parent transcript\n\n\
                         Your specific assignment follows in the next message — focus on \
                         that, not on continuing the parent's work.",
                        transcript.trim_end()
                    );
                    messages.push(Message {
                        role: Role::User,
                        content: vec![ContentBlock::Text { text: preface }],
                    });
                    // Acknowledgement turn so the prompt that follows reads as
                    // a clean "user → assistant → user" beat rather than two
                    // consecutive user messages (some providers reject the
                    // latter). The acknowledgement is a single short line so
                    // it doesn't pollute the child's reasoning.
                    messages.push(Message {
                        role: Role::Assistant,
                        content: vec![ContentBlock::Text {
                            text: "Understood — I have the parent's context. Ready for my \
                                   specific assignment."
                                .to_string(),
                        }],
                    });
                }
            }
        }
        messages.push(Message {
            role: Role::User,
            content: vec![ContentBlock::Text { text: prompt }],
        });

        tracing::warn!("[subagent] '{}' starting run_turn...", agent_id_clone);
        let run_turn_start = std::time::Instant::now();
        tracing::info!(
            target: "rustic::stream",
            agent_id = %agent_id_clone,
            initial_prompt_chars = messages[0].content.iter().map(|b| match b {
                ContentBlock::Text { text } => text.chars().count(),
                _ => 0,
            }).sum::<usize>(),
            "[spawn] calling run_turn for child"
        );
        let result = executor.run_turn(&mut messages, &child_context).await;
        tracing::warn!(
            "[subagent] '{}' run_turn finished: {}",
            agent_id_clone,
            if result.is_ok() { "OK" } else { "ERROR" }
        );
        tracing::info!(
            target: "rustic::stream",
            agent_id = %agent_id_clone,
            run_turn_ms = run_turn_start.elapsed().as_millis() as u64,
            ok = result.is_ok(),
            message_count = messages.len(),
            "[spawn] run_turn returned"
        );

        // Emit completion immediately so the UI spinner stops; diff computation follows.
        let summary = match &result {
            Ok(_) => {
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
                const SUMMARY_CAP: usize = 32_000;
                if raw.len() > SUMMARY_CAP {
                    format!("{}…", truncate_utf8(&raw, SUMMARY_CAP))
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

        tracing::warn!(
            "[subagent] '{}' completed successfully, summary len={}",
            agent_id_clone,
            summary.len()
        );
        let _ = parent_event_tx.try_send(TaskEvent::SubagentCompleted {
            task_id: parent_task_id.clone(),
            agent_id: agent_id_clone.clone(),
            model: model_for_result.clone(),
            summary: summary.clone(),
        });

        let blocked_on = blocked_writes_for_result
            .lock()
            .map(|mut v| std::mem::take(&mut *v))
            .unwrap_or_default();

        // Structured metadata: the write set collected from the child's tool
        // calls, so the orchestrator can see WHAT changed without re-deriving
        // it from the free-text summary.
        let files_written = registry.files_written(&parent_task_id, &agent_id_clone);
        let mut note_parts: Vec<String> = Vec::new();
        if !files_written.is_empty() {
            note_parts.push(format!("Files written: {}", files_written.join(", ")));
        }
        let notes = if note_parts.is_empty() {
            None
        } else {
            Some(note_parts.join("\n"))
        };

        let sub_result = SubagentResult {
            agent_id: agent_id_clone.clone(),
            model: model_for_result.clone(),
            summary,
            notes,
            blocked_on,
        };
        registry.complete(&parent_task_id, sub_result);
    });

    *out_agent_id = Some(agent_id.clone());

    Ok(ToolOutput {
        content: format!(
            "Sub-agent '{}' spawned (model: {}). It will run in parallel and results will be \
             injected automatically when complete.",
            agent_id, model
        ),
        is_error: false,
        attachments: Vec::new(),
    })
}


#[cfg(test)]
mod p1_13_batch_validation_tests {
    use super::*;
    use serde_json::json;

    fn run<F: std::future::Future<Output = Result<ToolOutput>>>(fut: F) -> Result<ToolOutput> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(fut)
    }

    #[test]
    fn empty_agents_array_rejected() {
        let agents: Vec<Value> = Vec::new();
        let out = futures::executor::block_on(async move {
            spawn_subagent_batch_validation_only(agents).await
        });
        assert!(out.is_error);
        assert!(out.content.contains("BATCH_SPAWN_REJECTED"));
        assert!(out.content.contains("empty"));
    }

    #[test]
    fn entry_missing_prompt_fails_validation() {
        let agents = vec![
            json!({ "name": "ok", "prompt": "do a thing" }),
            json!({ "name": "broken" }), // missing prompt
        ];
        let out = futures::executor::block_on(async move {
            spawn_subagent_batch_validation_only(agents).await
        });
        assert!(out.is_error);
        assert!(out.content.contains("entry[1]"));
        assert!(out.content.contains("prompt"));
    }

    #[test]
    fn invalid_model_tier_fails_validation() {
        let agents = vec![json!({
            "name": "x",
            "prompt": "do work",
            "model_tier": "bogus",
        })];
        let out = futures::executor::block_on(async move {
            spawn_subagent_batch_validation_only(agents).await
        });
        assert!(out.is_error);
        assert!(out.content.contains("model_tier"));
    }

    #[test]
    fn writes_must_be_array() {
        let agents = vec![json!({
            "name": "x",
            "prompt": "work",
            "writes": "not-an-array",
        })];
        let out = futures::executor::block_on(async move {
            spawn_subagent_batch_validation_only(agents).await
        });
        assert!(out.is_error);
        assert!(out.content.contains("writes"));
    }

    /// Mirrors production batch validation without requiring a ToolContext.
    async fn spawn_subagent_batch_validation_only(agents: Vec<Value>) -> ToolOutput {
        if agents.is_empty() {
            return ToolOutput {
                content: "BATCH_SPAWN_REJECTED: `agents` array is empty. Pass at least one entry, \
                          or use the single-agent shape `{ name, prompt, ... }`."
                    .to_string(),
                is_error: true,
                attachments: Vec::new(),
            };
        }
        let mut errors: Vec<String> = Vec::new();
        for (i, entry) in agents.iter().enumerate() {
            let prompt = entry
                .get("prompt")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            if prompt.is_empty() {
                errors.push(format!("entry[{}]: missing required `prompt`", i));
            }
            if let Some(name_val) = entry.get("name") {
                if !name_val.is_null() && name_val.as_str().is_none() {
                    errors.push(format!("entry[{}]: `name` must be a string", i));
                }
            }
            if let Some(writes_val) = entry.get("writes") {
                if !writes_val.is_array() {
                    errors.push(format!(
                        "entry[{}]: `writes` must be an array of strings",
                        i
                    ));
                }
            }
            if let Some(model_tier_val) = entry.get("model_tier") {
                let v = model_tier_val.as_str().unwrap_or("");
                if !matches!(v, "intelligent" | "fast") {
                    errors.push(format!(
                        "entry[{}]: `model_tier` must be \"intelligent\" or \"fast\"",
                        i
                    ));
                }
            }
        }
        if !errors.is_empty() {
            return ToolOutput {
                content: format!(
                    "BATCH_SPAWN_REJECTED: {} entry/entries failed validation. Nothing was spawned.\n{}",
                    errors.len(),
                    errors.join("\n")
                ),
                is_error: true,
                attachments: Vec::new(),
            };
        }
        ToolOutput {
            content: "OK".into(),
            is_error: false,
            attachments: Vec::new(),
        }
    }

    fn validate_project_root_input(
        raw: Option<&str>,
    ) -> Result<Option<std::path::PathBuf>, String> {
        match raw.map(|s| s.trim()).filter(|s| !s.is_empty()) {
            Some(s) => {
                let p = std::path::PathBuf::from(s);
                if !p.is_absolute() {
                    return Err(format!(
                        "SPAWN_REJECTED: `project_root` must be an absolute path; got '{}'.",
                        s
                    ));
                }
                if !p.exists() {
                    return Err(format!(
                        "SPAWN_REJECTED: `project_root` path does not exist: '{}'.",
                        s
                    ));
                }
                Ok(Some(p))
            }
            None => Ok(None),
        }
    }

    #[test]
    fn project_root_omitted_is_ok() {
        assert!(matches!(validate_project_root_input(None), Ok(None)));
        assert!(matches!(validate_project_root_input(Some("")), Ok(None)));
        assert!(matches!(validate_project_root_input(Some("   ")), Ok(None)));
    }

    #[test]
    fn project_root_relative_path_rejected() {
        let err = validate_project_root_input(Some("relative/path")).unwrap_err();
        assert!(err.contains("absolute path"), "msg: {}", err);
        let err = validate_project_root_input(Some("./bar")).unwrap_err();
        assert!(err.contains("absolute path"), "msg: {}", err);
    }

    #[test]
    fn project_root_nonexistent_absolute_path_rejected() {
        let bogus = if cfg!(windows) {
            "C:\\__rustic_test_does_not_exist_xyz123__"
        } else {
            "/__rustic_test_does_not_exist_xyz123__"
        };
        let err = validate_project_root_input(Some(bogus)).unwrap_err();
        assert!(err.contains("does not exist"), "msg: {}", err);
    }

    #[test]
    fn project_root_existing_absolute_path_accepted() {
        let dir = tempfile::tempdir().unwrap();
        let abs = dir.path().to_string_lossy().into_owned();
        let got = validate_project_root_input(Some(&abs)).unwrap();
        assert_eq!(got.as_deref(), Some(dir.path()));
    }
}

#[cfg(test)]
mod stringified_array_coercion_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn writes_string_is_coerced_to_array() {
        let mut p = json!({ "writes": "[\"a.ts\", \"b.ts\"]" });
        coerce_stringified_arrays(&mut p).unwrap();
        assert_eq!(p["writes"], json!(["a.ts", "b.ts"]));
    }

    #[test]
    fn reads_string_is_coerced_to_array() {
        let mut p = json!({ "reads": "[\"x.ts\"]" });
        coerce_stringified_arrays(&mut p).unwrap();
        assert_eq!(p["reads"], json!(["x.ts"]));
    }

    #[test]
    fn agents_string_is_coerced_to_array_of_objects() {
        let mut p = json!({
            "agents": "[{\"name\":\"a\",\"prompt\":\"do a\"},{\"name\":\"b\",\"prompt\":\"do b\"}]"
        });
        coerce_stringified_arrays(&mut p).unwrap();
        let arr = p["agents"].as_array().expect("coerced to array");
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["name"], json!("a"));
        assert_eq!(arr[1]["prompt"], json!("do b"));
    }

    #[test]
    fn per_entry_writes_inside_agents_are_coerced() {
        let mut p = json!({
            "agents": [
                { "name": "a", "prompt": "x", "writes": "[\"src/a.ts\"]" },
                { "name": "b", "prompt": "y", "writes": ["src/b.ts"] },
            ]
        });
        coerce_stringified_arrays(&mut p).unwrap();
        assert_eq!(p["agents"][0]["writes"], json!(["src/a.ts"]));
        assert_eq!(p["agents"][1]["writes"], json!(["src/b.ts"]));
    }

    #[test]
    fn already_an_array_is_left_alone() {
        let mut p = json!({ "writes": ["src/a.ts"] });
        coerce_stringified_arrays(&mut p).unwrap();
        assert_eq!(p["writes"], json!(["src/a.ts"]));
    }

    #[test]
    fn null_field_is_left_alone() {
        let mut p = json!({ "writes": null });
        coerce_stringified_arrays(&mut p).unwrap();
        assert_eq!(p["writes"], serde_json::Value::Null);
    }

    #[test]
    fn missing_field_is_ok() {
        let mut p = json!({ "name": "x" });
        coerce_stringified_arrays(&mut p).unwrap();
        // Nothing to coerce; no error, no insertion.
        assert!(p.get("writes").is_none());
    }

    #[test]
    fn empty_string_becomes_empty_array() {
        let mut p = json!({ "writes": "   " });
        coerce_stringified_arrays(&mut p).unwrap();
        assert_eq!(p["writes"], json!([]));
    }

    #[test]
    fn malformed_json_string_returns_clear_error() {
        let mut p = json!({ "writes": "[\"a.ts\", " }); // truncated JSON
        let err = coerce_stringified_arrays(&mut p).unwrap_err();
        assert!(err.contains("writes"), "msg: {}", err);
        assert!(err.contains("does not parse"), "msg: {}", err);
        // Error message must instruct on the correct shape.
        assert!(err.contains("[\"item1\""), "msg: {}", err);
    }

    #[test]
    fn string_parses_to_non_array_returns_clear_error() {
        // Valid JSON but it's an object, not an array.
        let mut p = json!({ "writes": "{\"foo\":\"bar\"}" });
        let err = coerce_stringified_arrays(&mut p).unwrap_err();
        assert!(err.contains("writes"), "msg: {}", err);
        assert!(err.contains("not an array"), "msg: {}", err);
    }

    #[test]
    fn non_string_non_array_value_returns_clear_error() {
        // E.g. the model passes a number — clearly wrong; tell it so.
        let mut p = json!({ "writes": 42 });
        let err = coerce_stringified_arrays(&mut p).unwrap_err();
        assert!(err.contains("writes"), "msg: {}", err);
        assert!(err.contains("number"), "msg: {}", err);
    }

    #[test]
    fn per_entry_error_carries_entry_index() {
        // Second entry has a bad writes; error should point at agents[1].
        let mut p = json!({
            "agents": [
                { "name": "a", "prompt": "x", "writes": ["ok.ts"] },
                { "name": "b", "prompt": "y", "writes": 42 },
            ]
        });
        let err = coerce_stringified_arrays(&mut p).unwrap_err();
        assert!(err.contains("agents[1]"), "msg: {}", err);
        assert!(err.contains("writes"), "msg: {}", err);
    }
}

#[cfg(test)]
mod parent_context_render_tests {
    use super::*;
    use crate::provider::{ContentBlock, Message, Role};

    fn user_text(t: &str) -> Message {
        Message {
            role: Role::User,
            content: vec![ContentBlock::Text { text: t.into() }],
        }
    }
    fn asst_text(t: &str) -> Message {
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::Text { text: t.into() }],
        }
    }
    fn asst_tool(name: &str, id: &str, input: serde_json::Value) -> Message {
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: id.into(),
                name: name.into(),
                input,
                thought_signature: None,
            }],
        }
    }
    fn tool_result(id: &str, body: &str) -> Message {
        Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: id.into(),
                content: body.into(),
                is_error: false,
            }],
        }
    }

    #[test]
    fn empty_history_renders_empty() {
        let out = render_parent_transcript(&[]);
        assert!(out.trim().is_empty());
    }

    #[test]
    fn renders_text_and_tool_blocks_in_order() {
        let msgs = vec![
            user_text("look at foo.rs"),
            asst_text("checking it"),
            asst_tool("read_file", "tu_1", serde_json::json!({"path": "foo.rs"})),
            tool_result("tu_1", "pub fn foo() {}"),
            asst_text("found it, simple function"),
        ];
        let out = render_parent_transcript(&msgs);
        let i_user = out.find("[User] look at foo.rs").expect("user msg");
        let i_asst1 = out.find("[Assistant] checking it").expect("first asst");
        let i_call = out.find("[Tool call: read_file]").expect("tool call");
        let i_result = out.find("[Tool result]").expect("tool result");
        let i_asst2 = out.find("[Assistant] found it").expect("second asst");
        assert!(i_user < i_asst1 && i_asst1 < i_call && i_call < i_result && i_result < i_asst2);
        // File content should be inlined verbatim so the child can read it.
        assert!(out.contains("pub fn foo() {}"));
    }

    #[test]
    fn drops_oldest_tool_results_when_over_cap() {
        // Build a history with many large tool_results; total > cap.
        let big = "X".repeat(5_000);
        let mut msgs = Vec::new();
        for i in 0..10 {
            msgs.push(asst_tool(
                "read_file",
                &format!("tu_{}", i),
                serde_json::json!({"i": i}),
            ));
            msgs.push(tool_result(&format!("tu_{}", i), &big));
        }
        msgs.push(asst_text("done — keep this sticky text"));
        let out = render_parent_transcript(&msgs);
        // Sticky text must survive.
        assert!(out.contains("keep this sticky text"));
        // Drop notice must be present.
        assert!(
            out.contains("dropped to fit context cap"),
            "expected drop notice in:\n{}",
            out
        );
        // Final rendered length should be within ballpark of the cap.
        assert!(
            out.len() < PARENT_CONTEXT_CHAR_CAP + 5_000,
            "render {} chars, cap {}",
            out.len(),
            PARENT_CONTEXT_CHAR_CAP
        );
    }

    #[test]
    fn truncates_individual_huge_tool_result() {
        let huge = "Y".repeat(20_000);
        let msgs = vec![
            asst_tool(
                "read_file",
                "tu_huge",
                serde_json::json!({"path": "huge.bin"}),
            ),
            tool_result("tu_huge", &huge),
        ];
        let out = render_parent_transcript(&msgs);
        assert!(
            out.contains("[truncated"),
            "expected per-result truncation marker: {}",
            out
        );
    }
}

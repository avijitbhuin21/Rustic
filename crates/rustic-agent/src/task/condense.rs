use crate::model_registry;
use crate::provider::{AiProvider, ContentBlock, Message, ProviderConfig, Role, TokenUsage};
use crate::task::TodoItem;
use anyhow::Result;
use std::sync::Arc;

/// Returns the context window size (in tokens) for a given model.
///
/// Precedence:
///   1. `custom_override` (user-configured per-provider value) — if > 0.
///   2. `[1m]` suffix on a Claude model id → 1M (overrides the registry's
///      standard 200K so auto-condense respects the extended window).
///   3. `model_registry` entry for the model id.
///   4. Family-based conservative default.
pub fn get_context_window(model: &str, custom_override: u32) -> u32 {
    if custom_override > 0 {
        tracing::info!(
            "[context_window] Using custom override: model={} custom_override={}",
            model,
            custom_override
        );
        return custom_override;
    }

    let m = model.to_lowercase();

    if m.contains("claude") && m.contains("[1m]") {
        tracing::info!(
            "[context_window] Detected [1m] marker: model={} context_window=1000000",
            model
        );
        return 1_000_000;
    }

    // Strip the `[1m]` marker before registry lookup so "claude-opus-4-7[1m]"
    // still matches the "claude-opus-4-7" entry.
    let stripped = m.replace("[1m]", "");
    if let Some(spec) = model_registry::lookup(&stripped) {
        tracing::info!(
            "[context_window] Found in registry: model={} stripped={} context_window={}",
            model,
            stripped,
            spec.context_window
        );
        return spec.context_window;
    }

    let fallback = if m.contains("claude") {
        200_000
    } else if m.starts_with("gemini") {
        1_048_576
    } else if m.starts_with("gpt-4o") || m.starts_with("gpt-4") {
        128_000
    } else if m.starts_with("o1") || m.starts_with("o3") {
        200_000
    } else {
        128_000
    };

    tracing::warn!(
        "[context_window] NOT FOUND in registry, using fallback: model={} stripped={} fallback={}",
        model,
        stripped,
        fallback
    );
    fallback
}

/// Returns true if the context is full enough that the next turn is likely to overflow.
/// Uses 93% of the *available* window (after reserving space for output + thinking).
/// This matches Claude Code's reactive "autocompact" threshold — fires close to
/// the actual limit rather than proactively. More aggressive than the old 70%,
/// which means fewer condensing operations and longer preserved history, but the
/// agent works closer to the context edge.
pub const CONDENSE_THRESHOLD_PCT: f64 = 0.93;

/// Returns the input-token count at which auto-condense triggers for the given limits.
pub fn condense_threshold(
    context_window: u32,
    max_output_tokens: u32,
    thinking_budget: u32,
) -> u32 {
    if context_window == 0 {
        return 0;
    }
    let reserved = max_output_tokens.saturating_add(thinking_budget);
    let available = context_window.saturating_sub(reserved);
    (available as f64 * CONDENSE_THRESHOLD_PCT) as u32
}

pub fn should_condense(
    input_tokens: u32,
    context_window: u32,
    max_output_tokens: u32,
    thinking_budget: u32,
) -> bool {
    if context_window == 0 {
        tracing::debug!("[condense] SKIPPED: context_window is 0");
        return false;
    }
    let reserved = max_output_tokens.saturating_add(thinking_budget);
    let available = context_window.saturating_sub(reserved);
    let threshold = condense_threshold(context_window, max_output_tokens, thinking_budget);
    let should = input_tokens > threshold;

    // Always log the condensing evaluation for debugging, not just when triggered
    let log_message = format!(
        "[condense] {} | input_tokens={} threshold={} | context_window={} max_output={} thinking={} | available={} reserved={} | threshold_pct={:.1}%",
        if should { "TRIGGERED" } else { "NOT_NEEDED" },
        input_tokens, threshold, context_window, max_output_tokens, thinking_budget,
        available, reserved, CONDENSE_THRESHOLD_PCT * 100.0
    );

    if should {
        tracing::info!("{}", log_message);
    } else {
        tracing::debug!("{}", log_message);
    }

    should
}

const CONDENSE_SYSTEM_PROMPT: &str =
    "You are a conversation summarizer for an AI coding assistant. \
     Respond ONLY with the requested summary. Do NOT call any tools. \
     Do NOT add commentary outside the summary.";

const CONDENSE_USER_PROMPT: &str = r#"The conversation above is the full history of an AI coding task.
Summarize it into a single, detailed context document so that work can continue seamlessly.

Be as concise as possible. Aim for under 10,000 tokens. Do NOT exceed 20,000 tokens under any circumstances — omit verbose tool outputs, repetitive steps, and intermediate dead-ends. Only preserve what is necessary to continue the task.

Your summary MUST include these sections:

## Original Task
The user's original request — quote it verbatim if short, or paraphrase precisely.

## Key Decisions
Important decisions made during the conversation and their rationale.

## Completed Work
Files created, modified, or deleted. Commands run and their outcomes. Include specific file paths and function names.

## Errors & Fixes
Any errors encountered and exactly how they were resolved.

## Pending Tasks
Remaining work that was explicitly requested but not yet done.

## Current State
What was being worked on immediately before this summary. Include the file name, the function or section, and what the next concrete step should be.

Be specific about file paths, line numbers, function names, and technical details. Do NOT omit information that would be needed to continue the task."#;

/// Convert a message history into a plain-text representation for the condensing prompt.
fn messages_to_text(messages: &[Message]) -> String {
    let mut out = String::with_capacity(messages.len() * 200);

    for msg in messages {
        let role_label = match msg.role {
            Role::User => "USER",
            Role::Assistant => "ASSISTANT",
            Role::System => "SYSTEM",
        };

        for block in &msg.content {
            match block {
                ContentBlock::Text { text } => {
                    out.push_str(&format!("[{}] {}\n\n", role_label, text));
                }
                ContentBlock::ToolUse { name, input, .. } => {
                    let input_str = serde_json::to_string(input).unwrap_or_default();
                    // Truncate very large tool inputs
                    let truncated = if input_str.len() > 2000 {
                        // Find a safe character boundary at or before byte 2000
                        let mut boundary = 2000.min(input_str.len());
                        while boundary > 0 && !input_str.is_char_boundary(boundary) {
                            boundary -= 1;
                        }
                        format!("{}... [truncated]", &input_str[..boundary])
                    } else {
                        input_str
                    };
                    out.push_str(&format!("[ASSISTANT → Tool: {}] {}\n\n", name, truncated));
                }
                ContentBlock::ToolResult {
                    content, is_error, ..
                } => {
                    let label = if *is_error {
                        "Tool Error"
                    } else {
                        "Tool Result"
                    };
                    // Truncate very large tool results
                    let truncated = if content.len() > 3000 {
                        // Find a safe character boundary at or before byte 3000
                        let mut boundary = 3000.min(content.len());
                        while boundary > 0 && !content.is_char_boundary(boundary) {
                            boundary -= 1;
                        }
                        format!("{}... [truncated]", &content[..boundary])
                    } else {
                        content.clone()
                    };
                    out.push_str(&format!("[{}] {}\n\n", label, truncated));
                }
                ContentBlock::Thinking { .. }
                | ContentBlock::RedactedThinking { .. }
                | ContentBlock::ModelSwitch { .. }
                | ContentBlock::Image { .. } => {
                    // Skip thinking, redacted-thinking, model-switch, and image blocks
                }
            }
        }
    }

    out
}

/// Number of most-recent messages kept verbatim when condensing. The model
/// summarizes only the *middle* of the conversation; the head (original task)
/// and tail (recent turns) pass through unmodified so the model retains its
/// immediate working context.
///
/// A larger tail = more verbatim context survives each compact, which helps
/// the post-compact model pick up where it left off without re-reading files
/// it had just finished reasoning over. Claude Code's `partialCompact`
/// strategy preserves similar-sized windows (12-16 messages). We previously
/// used 6, which was often too aggressive on tool-heavy tasks — a single
/// edit-read-verify cycle can eat 4 messages.
pub const CONDENSE_KEEP_TAIL: usize = 12;

/// Appended to condense/truncation notices when the host archives dropped
/// messages, so the model knows verbatim history is recoverable.
const ARCHIVE_HINT: &str = "\n[The dropped turns were archived verbatim — load `search_history` via `tool_search` to grep them, then `read_history` to read a specific archived message in full.]";

/// The exact slice a successful `condense_context` will drop for this message
/// list; lives next to the split logic so the executor archives precisely
/// what condensing discards.
pub fn condense_dropped_slice(messages: &[Message]) -> &[Message] {
    let n = messages.len();
    if n > CONDENSE_KEEP_TAIL + 2 {
        &messages[1..n - CONDENSE_KEEP_TAIL]
    } else {
        messages
    }
}

/// The exact slice `sliding_window_fallback` will drop for this message list.
pub fn sliding_window_dropped_slice(messages: &[Message]) -> &[Message] {
    if messages.len() <= 3 {
        return &[];
    }
    let remaining_len = messages.len() - 1;
    let keep_count = remaining_len / 2;
    &messages[1..1 + (remaining_len - keep_count)]
}

/// True for synthetic messages produced by condensing itself (the summary
/// message and the sliding-window truncation notice); filtered out when the
/// full raw history is reconstructed from the archive for display.
pub fn is_condense_artifact_blocks(content: &[ContentBlock]) -> bool {
    matches!(content.first(), Some(ContentBlock::Text { text })
        if text.starts_with("[Context Condensed")
            || text.starts_with("[Earlier conversation history was truncated"))
}

/// `is_condense_artifact_blocks` for hosts holding serialized content_json.
pub fn is_condense_artifact_json(content_json: &str) -> bool {
    serde_json::from_str::<Vec<ContentBlock>>(content_json)
        .map(|blocks| is_condense_artifact_blocks(&blocks))
        .unwrap_or(false)
}

/// Total byte budget for the preserved-file-reads block injected into the
/// summary message. Caps how much verbatim file content survives a condense
/// pass beyond what the tail already keeps. ~96 KB ≈ 24K tokens — enough for
/// ~5 medium source files but small enough not to dominate the post-condense
/// prompt.
const PRESERVED_READS_BUDGET_BYTES: usize = 96 * 1024;

/// Per-file cap inside the preserved block. A single large file shouldn't
/// consume the whole budget — leave room for several smaller ones.
const PRESERVED_READS_PER_FILE_CAP: usize = 32 * 1024;

/// Preservation key + display label for a tool call whose result should
/// survive condensing verbatim. Covers the tools that load source content or
/// codebase structure into context — losing them forces re-reads after every
/// condense: `read_file` (per path), single-query `grep_search` (per
/// query+scope), and `outline` (per file). Batch-mode calls are not preserved
/// (their results interleave many targets; the summary covers them).
fn preserve_key(name: &str, input: &serde_json::Value) -> Option<(String, String)> {
    let get = |k: &str| input.get(k).and_then(|v| v.as_str()).unwrap_or("");
    match name {
        "read_file" => {
            let path = input.get("path").and_then(|v| v.as_str())?;
            Some((format!("file:{}", path), path.to_string()))
        }
        "grep_search" => {
            let query = input.get("query").and_then(|v| v.as_str())?;
            if query.is_empty() {
                return None;
            }
            let key = format!(
                "grep:{}@{}#i={}#e={}",
                query,
                get("path"),
                get("include"),
                get("exclude")
            );
            let label = format!("grep_search results for '{}'", query);
            Some((key, label))
        }
        "outline" => {
            let path = input.get("path").and_then(|v| v.as_str())?;
            Some((format!("outline:{}", path), format!("outline of {}", path)))
        }
        _ => None,
    }
}

/// True for result bodies that carry no preservable content (registry stubs
/// and shrink-pass placeholders).
fn is_stub_result(content: &str) -> bool {
    content.starts_with("FILE_UNCHANGED") || content.starts_with("[Old ")
}

/// Walk a slice of messages, build a `preserve-key -> (label, last successful
/// result)` map by pairing preservable tool_use ids with their matching
/// tool_result content. Later results for the same key overwrite earlier ones.
fn extract_recent_file_reads(messages: &[Message]) -> Vec<(String, String, String)> {
    use std::collections::HashMap;

    // tool_use_id -> (key, label), for preservable calls only.
    let mut id_to_key: HashMap<String, (String, String)> = HashMap::new();
    for msg in messages {
        for block in &msg.content {
            if let ContentBlock::ToolUse {
                id, name, input, ..
            } = block
            {
                if let Some((key, label)) = preserve_key(name, input) {
                    id_to_key.insert(id.clone(), (key, label));
                }
            }
        }
    }

    // key -> latest content. Walking forward overwrites earlier entries so
    // the final value per key is the most recent result.
    let mut latest: HashMap<String, (String, String)> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for msg in messages {
        for block in &msg.content {
            if let ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
                ..
            } = block
            {
                if *is_error {
                    continue;
                }
                if let Some((key, label)) = id_to_key.get(tool_use_id) {
                    // Skip registry stubs and shrink-pass placeholders — they
                    // reference an earlier result, which is preserved on its own.
                    if is_stub_result(content) {
                        continue;
                    }
                    if !latest.contains_key(key) {
                        order.push(key.clone());
                    }
                    latest.insert(key.clone(), (label.clone(), content.clone()));
                }
            }
        }
    }

    // Return in encounter order so the latest results of newly-seen keys
    // appear after earlier ones.
    order
        .into_iter()
        .filter_map(|k| latest.remove(&k).map(|(label, c)| (k, label, c)))
        .collect()
}

/// Set of preserve-keys that the tail already contains a successful result
/// for. These don't need to be preserved separately — they survive verbatim.
fn paths_in_tail_reads(tail: &[Message]) -> std::collections::HashSet<String> {
    use std::collections::HashSet;
    let mut id_to_key: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for msg in tail {
        for block in &msg.content {
            if let ContentBlock::ToolUse {
                id, name, input, ..
            } = block
            {
                if let Some((key, _label)) = preserve_key(name, input) {
                    id_to_key.insert(id.clone(), key);
                }
            }
        }
    }
    let mut out: HashSet<String> = HashSet::new();
    for msg in tail {
        for block in &msg.content {
            if let ContentBlock::ToolResult {
                tool_use_id,
                is_error,
                content,
                ..
            } = block
            {
                if *is_error {
                    continue;
                }
                if is_stub_result(content) {
                    continue;
                }
                if let Some(key) = id_to_key.get(tool_use_id) {
                    out.insert(key.clone());
                }
            }
        }
    }
    out
}

/// Format a preserved-reads block to append onto the summary message. Returns
/// `None` when there's nothing worth preserving.
fn format_preserved_reads_block(middle: &[Message], tail: &[Message]) -> Option<String> {
    let reads = extract_recent_file_reads(middle);
    if reads.is_empty() {
        return None;
    }

    let tail_paths = paths_in_tail_reads(tail);

    let mut out = String::from(
        "\n\n---\n\n\
         ## Preserved reads\n\n\
         These file reads, grep results, and outlines were loaded earlier in the conversation. \
         They are kept here verbatim so you don't have to re-run the calls. If a file has \
         changed on disk since the original read, re-read it; otherwise refer back to this \
         section.\n\n",
    );

    let mut total = 0usize;
    let mut kept = 0usize;
    for (key, label, content) in reads {
        if tail_paths.contains(&key) {
            continue;
        }

        let body = if content.len() > PRESERVED_READS_PER_FILE_CAP {
            // Find a safe character boundary at or before the cap
            let mut boundary = PRESERVED_READS_PER_FILE_CAP.min(content.len());
            while boundary > 0 && !content.is_char_boundary(boundary) {
                boundary -= 1;
            }
            format!(
                "{}\n\n[truncated at {} bytes — re-read with offset/limit for the rest]",
                &content[..boundary],
                boundary
            )
        } else {
            content
        };

        let entry = format!("### {}\n\n{}\n\n", label, body);
        if total + entry.len() > PRESERVED_READS_BUDGET_BYTES {
            out.push_str(&format!(
                "_(Additional preserved reads dropped — budget of {} bytes exhausted.)_\n",
                PRESERVED_READS_BUDGET_BYTES
            ));
            break;
        }
        total += entry.len();
        out.push_str(&entry);
        kept += 1;
    }

    if kept == 0 {
        return None;
    }
    Some(out)
}

/// Format the agent's todo list as a verbatim block appended to the condensed
/// summary. The plan must never depend on the summarizer reproducing it —
/// returns `None` when there's nothing worth carrying (empty or all done).
fn format_todo_block(todos: &[TodoItem]) -> Option<String> {
    if todos.is_empty() || todos.iter().all(|t| t.status == "completed") {
        return None;
    }
    let mut out = String::from(
        "\n\n---\n\n\
         ## Active todo list (preserved verbatim — not summarized)\n\n",
    );
    for (i, t) in todos.iter().enumerate() {
        out.push_str(&format!("{}. [{}] {}\n", i + 1, t.status, t.content));
    }
    out.push_str("\nContinue working from this list; keep it current with todo_write.\n");
    Some(out)
}

/// Condense the conversation by asking a cheaper model to produce a structured
/// summary of the *middle* portion, while preserving:
///   - the first user message (original task, verbatim)
///   - the last CONDENSE_KEEP_TAIL messages (verbatim)
///   - the agent's current todo list (verbatim, appended to the summary)
///
/// The summary replaces only what's between. Returns the rebuilt message vec,
/// the token usage of the condensing call itself, and the model id that
/// actually ran the summarization (usually a cheaper sibling — callers must
/// bill the usage under THAT model, not the main task model).
pub async fn condense_context(
    provider: &Arc<dyn AiProvider>,
    config: &ProviderConfig,
    messages: &[Message],
    current_todos: &[TodoItem],
    preferred_condense_model: Option<&str>,
    archive_hint: bool,
) -> Result<(Vec<Message>, TokenUsage, String)> {
    // Figure out the head (original task), tail (recent turns), and middle
    // (what actually gets summarized).
    let n = messages.len();
    // Need at least head(1) + tail + 1 middle message to be worth condensing.
    // If the conversation is too short, fall through to condensing everything.
    let middle = condense_dropped_slice(messages);
    let (head, tail): (&[Message], &[Message]) = if n > CONDENSE_KEEP_TAIL + 2 {
        (&messages[..1], &messages[n - CONDENSE_KEEP_TAIL..])
    } else {
        (&[], &[])
    };

    let conversation_text = messages_to_text(middle);

    let condense_message = format!("{}\n\n---\n\n{}", conversation_text, CONDENSE_USER_PROMPT);

    // Route the condensing call to a cheaper model in the same family.
    // Summarization is a routine transform; Haiku/gpt-4o-mini handle it fine
    // at a fraction of the cost, and we don't waste Opus/Sonnet tokens on it.
    let condense_model = cheaper_sibling_for(&config.model, preferred_condense_model);

    // Use the model's full max output tokens from the registry so the summary
    // doesn't get truncated. The prompt itself instructs the model to stay brief
    // (under 20k tokens), but we don't want to hard-cap below what's available.
    let condense_max_tokens = model_registry::max_output_tokens(&condense_model, config.max_tokens);

    let condense_config = ProviderConfig {
        api_key: config.api_key.clone(),
        model: condense_model,
        max_tokens: condense_max_tokens,
        temperature: 0.0,
        base_url: config.base_url.clone(),
        system_prompt: Some(CONDENSE_SYSTEM_PROMPT.to_string()),
        thinking_budget: 0,
        context_window: 0, // not needed for the condensing call itself
        web_search_enabled: false,
        web_fetch_enabled: false,
        supports_temperature: config.supports_temperature,
        supports_reasoning_effort: config.supports_reasoning_effort,
        supports_adaptive_thinking: config.supports_adaptive_thinking,
        cancel_token: config.cancel_token.clone(),
        custom_input_cost: config.custom_input_cost,
        custom_output_cost: config.custom_output_cost,
        custom_cache_read_cost: config.custom_cache_read_cost,
        custom_cache_write_cost: config.custom_cache_write_cost,
        // Condensing runs a cheaper sibling model, not the main one, so the
        // per-model provider allow-list doesn't apply — route freely.
        allowed_providers: None,
    };

    let condense_input = vec![Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: condense_message,
        }],
    }];

    let (response, used_model) = match provider
        .chat(condense_input.clone(), vec![], &condense_config, None)
        .await
    {
        Ok(r) => {
            let model = condense_config.model.clone();
            (r, model)
        }
        Err(e) if condense_config.model != config.model => {
            tracing::warn!(
                "[condense] summarizer model '{}' failed ({}); retrying once with the main model '{}'",
                condense_config.model,
                e,
                config.model
            );
            let mut retry_config = condense_config.clone();
            retry_config.model = config.model.clone();
            retry_config.max_tokens =
                model_registry::max_output_tokens(&config.model, config.max_tokens);
            retry_config.allowed_providers = config.allowed_providers.clone();
            let r = provider
                .chat(condense_input, vec![], &retry_config, None)
                .await?;
            (r, retry_config.model)
        }
        Err(e) => return Err(e),
    };

    // Extract text from the response
    let summary = response
        .content
        .iter()
        .filter_map(|b| {
            if let ContentBlock::Text { text } = b {
                Some(text.as_str())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    if summary.trim().is_empty() {
        return Err(anyhow::anyhow!("Condensing produced an empty summary"));
    }

    // Rebuild: head (original task) + summary + tail (recent turns verbatim).
    // If we had no head/tail (short conversation path), the summary stands alone.
    let mut rebuilt: Vec<Message> = Vec::with_capacity(head.len() + 1 + tail.len());
    rebuilt.extend_from_slice(head);

    // Sanitize tail: if the first tail message is an assistant message whose
    // first content block is a ToolResult... actually, tool_results go on user
    // messages. The real constraint is that a `tool_result` must follow its
    // `tool_use` in the same session. Since the middle gets replaced with a
    // text summary, any tool_result in the tail that references a tool_use
    // from the middle would be dangling. Strip orphan tool_result blocks from
    // the tail so the API doesn't 400.
    //
    // Collect tool_use ids still visible to the API (head + tail tool_uses).
    let surviving_ids: std::collections::HashSet<String> = head
        .iter()
        .chain(tail.iter())
        .flat_map(|m| m.content.iter())
        .filter_map(|b| {
            if let ContentBlock::ToolUse { id, .. } = b {
                Some(id.clone())
            } else {
                None
            }
        })
        .collect();

    // F4 (R.2): also preserve verbatim content of the most-recent read_file
    // result per path that was about to be summarized away. Without this the
    // agent re-reads files it already loaded into context, costing turns.
    let preserved_block = format_preserved_reads_block(middle, tail).unwrap_or_default();

    // The todo list rides through verbatim — long-session drift after a
    // condense was traced to the plan only surviving as a lossy summary.
    let todo_block = format_todo_block(current_todos).unwrap_or_default();

    let summary_msg = Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: format!(
                "[Context Condensed — the middle of this conversation was summarized to save tokens. \
                 The original task above and the most recent turns below are preserved verbatim.]{}\n\n{}{}{}",
                if archive_hint { ARCHIVE_HINT } else { "" },
                summary, todo_block, preserved_block
            ),
        }],
    };
    rebuilt.push(summary_msg);

    for msg in tail.iter() {
        let filtered_content: Vec<ContentBlock> = msg
            .content
            .iter()
            .filter(|b| match b {
                ContentBlock::ToolResult { tool_use_id, .. } => surviving_ids.contains(tool_use_id),
                _ => true,
            })
            .cloned()
            .collect();
        if !filtered_content.is_empty() {
            rebuilt.push(Message {
                role: msg.role.clone(),
                content: filtered_content,
            });
        }
    }

    sanitize_tool_pairing(&mut rebuilt);

    Ok((rebuilt, response.usage, used_model))
}

/// Pick a cheaper sibling model for the condensing call. `preferred` (the
/// user-configured fast-tier model, when it runs on the same provider) wins
/// over the hardcoded family fallbacks so the mapping doesn't rot as
/// providers deprecate model ids. Falls back to the original model when we
/// don't know a good match — e.g. Compatible / custom providers where routing
/// to a different model id would likely fail.
fn cheaper_sibling_for(model: &str, preferred: Option<&str>) -> String {
    if let Some(p) = preferred {
        let p = p.trim();
        if !p.is_empty() {
            return p.to_string();
        }
    }
    let m = model.to_lowercase();
    if m.contains("claude") {
        // Any Claude model → the current Haiku. ~1/20th the cost of Opus.
        return "claude-haiku-4-5-20251001".to_string();
    }
    if m.starts_with("gpt-") || m.starts_with("o1") || m.starts_with("o3") || m.starts_with("o4") {
        return "gpt-5.4-mini".to_string();
    }
    if m.starts_with("gemini") {
        return "gemini-3.1-flash-lite".to_string();
    }
    model.to_string()
}

/// Repair broken tool_use/tool_result pairing by rewriting orphaned blocks as plain-text blocks so every provider accepts the history; returns true when anything was rewritten.
pub fn sanitize_tool_pairing(messages: &mut [Message]) -> bool {
    use std::collections::HashSet;

    let per_msg: Vec<(HashSet<String>, HashSet<String>)> = messages
        .iter()
        .map(|m| {
            let mut uses = HashSet::new();
            let mut results = HashSet::new();
            for b in &m.content {
                match b {
                    ContentBlock::ToolUse { id, .. } => {
                        uses.insert(id.clone());
                    }
                    ContentBlock::ToolResult { tool_use_id, .. } => {
                        results.insert(tool_use_id.clone());
                    }
                    _ => {}
                }
            }
            (uses, results)
        })
        .collect();

    let mut changed = false;
    let n = messages.len();
    for i in 0..n {
        for block in messages[i].content.iter_mut() {
            match block {
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => {
                    let paired = per_msg[i].0.contains(tool_use_id.as_str())
                        || (i > 0 && per_msg[i - 1].0.contains(tool_use_id.as_str()));
                    if !paired {
                        let label = if *is_error {
                            "tool error"
                        } else {
                            "tool result"
                        };
                        let text = format!(
                            "[Earlier {label} \u{2014} its tool call is no longer in the visible history (dropped during context truncation); content kept as plain text:]\n{content}"
                        );
                        *block = ContentBlock::Text { text };
                        changed = true;
                    }
                }
                ContentBlock::ToolUse {
                    id, name, input, ..
                } => {
                    let answered = per_msg[i].1.contains(id.as_str())
                        || (i + 1 < n && per_msg[i + 1].1.contains(id.as_str()));
                    if !answered {
                        let mut input_str = serde_json::to_string(input).unwrap_or_default();
                        if input_str.len() > 400 {
                            let mut b = 400;
                            while b > 0 && !input_str.is_char_boundary(b) {
                                b -= 1;
                            }
                            input_str.truncate(b);
                            input_str.push_str("\u{2026} [truncated]");
                        }
                        let text = format!(
                            "[Tool call `{name}` with input {input_str} never received a result (interrupted). Re-issue the call if it is still needed.]"
                        );
                        *block = ContentBlock::Text { text };
                        changed = true;
                    }
                }
                _ => {}
            }
        }
    }
    changed
}

/// Fallback: sliding window truncation when API-based condensing fails.
/// Keeps the first message (original task), the agent's todo list (verbatim),
/// and the last ~50% of messages.
pub fn sliding_window_fallback(
    messages: &[Message],
    current_todos: &[TodoItem],
    archive_hint: bool,
) -> Vec<Message> {
    if messages.len() <= 3 {
        return messages.to_vec();
    }

    let mut result = Vec::new();

    // Keep the first message (original task)
    result.push(messages[0].clone());

    // Insert a truncation notice, carrying the todo list through verbatim so
    // the plan survives even the lossy fallback path.
    let todo_block = format_todo_block(current_todos).unwrap_or_default();
    result.push(Message {
        role: Role::Assistant,
        content: vec![ContentBlock::Text {
            text: format!(
                "[Earlier conversation history was truncated to fit the context window. \
                 Continuing from the most recent messages.]{}{}",
                if archive_hint { ARCHIVE_HINT } else { "" },
                todo_block
            ),
        }],
    });

    // Keep everything after the dropped slice (the last ~50%, excluding the first)
    let dropped = sliding_window_dropped_slice(messages);
    result.extend_from_slice(&messages[1 + dropped.len()..]);

    sanitize_tool_pairing(&mut result);

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(i: usize) -> Message {
        Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: format!("m{}", i),
            }],
        }
    }

    fn text_of(m: &Message) -> &str {
        match &m.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text"),
        }
    }

    #[test]
    fn sliding_window_dropped_plus_kept_reconstructs_original() {
        for n in [4usize, 5, 20, 21] {
            let messages: Vec<Message> = (0..n).map(msg).collect();
            let dropped = sliding_window_dropped_slice(&messages).to_vec();
            let kept = sliding_window_fallback(&messages, &[], false);
            assert_eq!(text_of(&kept[0]), text_of(&messages[0]));
            assert!(text_of(&kept[1]).starts_with("[Earlier conversation history was truncated"));
            let reconstructed: Vec<&str> = std::iter::once(text_of(&messages[0]))
                .chain(dropped.iter().map(text_of))
                .chain(kept[2..].iter().map(text_of))
                .collect();
            let original: Vec<&str> = messages.iter().map(text_of).collect();
            assert_eq!(reconstructed, original, "n={}", n);
        }
    }

    #[test]
    fn sliding_window_short_conversation_drops_nothing() {
        let messages: Vec<Message> = (0..3).map(msg).collect();
        assert!(sliding_window_dropped_slice(&messages).is_empty());
        assert_eq!(sliding_window_fallback(&messages, &[], false).len(), 3);
    }

    #[test]
    fn condense_dropped_slice_matches_split_boundaries() {
        let messages: Vec<Message> = (0..30).map(msg).collect();
        let dropped = condense_dropped_slice(&messages);
        assert_eq!(text_of(&dropped[0]), "m1");
        assert_eq!(dropped.len(), 30 - 1 - CONDENSE_KEEP_TAIL);

        let short: Vec<Message> = (0..5).map(msg).collect();
        assert_eq!(condense_dropped_slice(&short).len(), 5);
    }

    #[test]
    fn artifact_detection_matches_condense_and_fallback_notices() {
        let fallback = sliding_window_fallback(&(0..10).map(msg).collect::<Vec<_>>(), &[], true);
        assert!(is_condense_artifact_blocks(&fallback[1].content));
        assert!(!is_condense_artifact_blocks(&fallback[0].content));
        assert!(is_condense_artifact_json(
            "[{\"type\":\"text\",\"text\":\"[Context Condensed \u{2014} the middle was summarized]\"}]"
        ));
        assert!(!is_condense_artifact_json(
            "[{\"type\":\"text\",\"text\":\"hello\"}]"
        ));
        assert!(!is_condense_artifact_json("not json"));
    }

    fn tool_use(id: &str, name: &str) -> ContentBlock {
        ContentBlock::ToolUse {
            id: id.to_string(),
            name: name.to_string(),
            input: serde_json::json!({"path": "src/lib.rs"}),
            thought_signature: None,
        }
    }

    fn tool_result(id: &str, content: &str) -> ContentBlock {
        ContentBlock::ToolResult {
            tool_use_id: id.to_string(),
            content: content.to_string(),
            is_error: false,
        }
    }

    #[test]
    fn sanitize_keeps_valid_pairs_untouched() {
        let mut messages = vec![
            msg(0),
            Message {
                role: Role::Assistant,
                content: vec![tool_use("t1", "read_file")],
            },
            Message {
                role: Role::User,
                content: vec![tool_result("t1", "file body")],
            },
        ];
        assert!(!sanitize_tool_pairing(&mut messages));
        assert!(matches!(
            &messages[1].content[0],
            ContentBlock::ToolUse { .. }
        ));
        assert!(matches!(
            &messages[2].content[0],
            ContentBlock::ToolResult { .. }
        ));
    }

    #[test]
    fn sanitize_converts_orphan_tool_result_to_text() {
        let mut messages = vec![
            msg(0),
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::Text {
                    text: "[Earlier conversation history was truncated]".to_string(),
                }],
            },
            Message {
                role: Role::User,
                content: vec![tool_result("gone", "grep output here")],
            },
        ];
        assert!(sanitize_tool_pairing(&mut messages));
        assert!(matches!(
            &messages[2].content[0],
            ContentBlock::Text { text } if text.contains("grep output here")
        ));
    }

    #[test]
    fn sanitize_converts_dangling_tool_use_to_text() {
        let mut messages = vec![
            msg(0),
            Message {
                role: Role::Assistant,
                content: vec![tool_use("t9", "run_command")],
            },
            Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "can you continue ?".to_string(),
                }],
            },
        ];
        assert!(sanitize_tool_pairing(&mut messages));
        assert!(matches!(
            &messages[1].content[0],
            ContentBlock::Text { text } if text.contains("run_command")
        ));
    }

    #[test]
    fn sanitize_allows_inline_server_tool_pairs() {
        let mut messages = vec![
            msg(0),
            Message {
                role: Role::Assistant,
                content: vec![
                    tool_use("srvtoolu_1", "web_search"),
                    tool_result("srvtoolu_1", "search results"),
                ],
            },
        ];
        assert!(!sanitize_tool_pairing(&mut messages));
        assert!(matches!(
            &messages[1].content[0],
            ContentBlock::ToolUse { .. }
        ));
    }

    #[test]
    fn sliding_window_fallback_never_leaves_orphan_tool_blocks() {
        let messages = vec![
            msg(0),
            msg(1),
            msg(2),
            Message {
                role: Role::Assistant,
                content: vec![tool_use("t1", "read_file")],
            },
            Message {
                role: Role::User,
                content: vec![tool_result("t1", "file body")],
            },
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::Text {
                    text: "done".to_string(),
                }],
            },
        ];
        let kept = sliding_window_fallback(&messages, &[], false);
        for m in &kept {
            for b in &m.content {
                assert!(
                    !matches!(b, ContentBlock::ToolResult { .. })
                        && !matches!(b, ContentBlock::ToolUse { .. }),
                    "fallback left a tool block that would orphan: {:?}",
                    b
                );
            }
        }
    }
}

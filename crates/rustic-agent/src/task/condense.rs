use crate::model_registry;
use crate::provider::{
    AiProvider, ContentBlock, Message, ProviderConfig, Role, TokenUsage,
};
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
            model, custom_override
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
            model, stripped, spec.context_window
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
        model, stripped, fallback
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

pub fn should_condense(input_tokens: u32, context_window: u32, max_output_tokens: u32, thinking_budget: u32) -> bool {
    if context_window == 0 {
        tracing::debug!("[condense] SKIPPED: context_window is 0");
        return false;
    }
    let reserved = max_output_tokens.saturating_add(thinking_budget);
    let available = context_window.saturating_sub(reserved);
    let threshold = (available as f64 * CONDENSE_THRESHOLD_PCT) as u32;
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
                ContentBlock::ToolResult { content, is_error, .. } => {
                    let label = if *is_error { "Tool Error" } else { "Tool Result" };
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

/// Total byte budget for the preserved-file-reads block injected into the
/// summary message. Caps how much verbatim file content survives a condense
/// pass beyond what the tail already keeps. ~96 KB ≈ 24K tokens — enough for
/// ~5 medium source files but small enough not to dominate the post-condense
/// prompt.
const PRESERVED_READS_BUDGET_BYTES: usize = 96 * 1024;

/// Per-file cap inside the preserved block. A single large file shouldn't
/// consume the whole budget — leave room for several smaller ones.
const PRESERVED_READS_PER_FILE_CAP: usize = 32 * 1024;

/// Tool names recognised as "file reads" whose results we want to preserve
/// through condensing. Kept narrow on purpose — only the tools that load
/// actual source content into context.
fn is_preservable_read_tool(name: &str) -> bool {
    matches!(name, "read_file")
}

/// Extract the path argument from a `read_file` tool input.
fn read_file_path(input: &serde_json::Value) -> Option<String> {
    input
        .get("path")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// Walk a slice of messages, build a `path -> last successful read content`
/// map by pairing `read_file` tool_use ids with their matching tool_result
/// content. Later reads of the same path overwrite earlier ones.
fn extract_recent_file_reads(messages: &[Message]) -> Vec<(String, String)> {
    use std::collections::HashMap;

    // tool_use_id -> path, for read_file calls only.
    let mut id_to_path: HashMap<String, String> = HashMap::new();
    for msg in messages {
        for block in &msg.content {
            if let ContentBlock::ToolUse { id, name, input, .. } = block {
                if is_preservable_read_tool(name) {
                    if let Some(path) = read_file_path(input) {
                        id_to_path.insert(id.clone(), path);
                    }
                }
            }
        }
    }

    // path -> latest content. Walking forward overwrites earlier entries so
    // the final value per path is the most recent read.
    let mut latest: HashMap<String, String> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for msg in messages {
        for block in &msg.content {
            if let ContentBlock::ToolResult { tool_use_id, content, is_error, .. } = block {
                if *is_error { continue; }
                if let Some(path) = id_to_path.get(tool_use_id) {
                    // Skip FILE_UNCHANGED stubs — they reference an earlier
                    // read in the conversation, which may have been before
                    // the middle and is still in the head/tail anyway.
                    if content.starts_with("FILE_UNCHANGED") { continue; }
                    if !latest.contains_key(path) {
                        order.push(path.clone());
                    }
                    latest.insert(path.clone(), content.clone());
                }
            }
        }
    }

    // Return in encounter order so the latest reads of newly-seen paths
    // appear after earlier ones.
    order.into_iter()
        .filter_map(|p| latest.remove(&p).map(|c| (p, c)))
        .collect()
}

/// Set of file paths that the tail already contains a successful read for.
/// These don't need to be preserved separately — they survive verbatim.
fn paths_in_tail_reads(tail: &[Message]) -> std::collections::HashSet<String> {
    use std::collections::HashSet;
    let mut id_to_path: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for msg in tail {
        for block in &msg.content {
            if let ContentBlock::ToolUse { id, name, input, .. } = block {
                if is_preservable_read_tool(name) {
                    if let Some(path) = read_file_path(input) {
                        id_to_path.insert(id.clone(), path);
                    }
                }
            }
        }
    }
    let mut out: HashSet<String> = HashSet::new();
    for msg in tail {
        for block in &msg.content {
            if let ContentBlock::ToolResult { tool_use_id, is_error, content, .. } = block {
                if *is_error { continue; }
                if content.starts_with("FILE_UNCHANGED") { continue; }
                if let Some(path) = id_to_path.get(tool_use_id) {
                    out.insert(path.clone());
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
    if reads.is_empty() { return None; }

    let tail_paths = paths_in_tail_reads(tail);

    let mut out = String::from(
        "\n\n---\n\n\
         ## Preserved file reads\n\n\
         These files were read earlier in the conversation. Their contents are kept here \
         verbatim so you don't have to re-read them. If a file has changed on disk since the \
         original read, re-read it; otherwise refer back to this section.\n\n"
    );

    let mut total = 0usize;
    let mut kept = 0usize;
    for (path, content) in reads {
        if tail_paths.contains(&path) { continue; }

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

        let entry = format!("### {}\n\n{}\n\n", path, body);
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

    if kept == 0 { return None; }
    Some(out)
}

/// Condense the conversation by asking a cheaper model to produce a structured
/// summary of the *middle* portion, while preserving:
///   - the first user message (original task, verbatim)
///   - the last CONDENSE_KEEP_TAIL messages (verbatim)
///
/// The summary replaces only what's between. Returns the rebuilt message vec
/// and the token usage of the condensing call itself, so callers can fold it
/// into the task's running cost.
pub async fn condense_context(
    provider: &Arc<dyn AiProvider>,
    config: &ProviderConfig,
    messages: &[Message],
) -> Result<(Vec<Message>, TokenUsage)> {
    // Figure out the head (original task), tail (recent turns), and middle
    // (what actually gets summarized).
    let n = messages.len();
    // Need at least head(1) + tail + 1 middle message to be worth condensing.
    // If the conversation is too short, fall through to condensing everything.
    let (head, middle, tail): (&[Message], &[Message], &[Message]) =
        if n > CONDENSE_KEEP_TAIL + 2 {
            (&messages[..1], &messages[1..n - CONDENSE_KEEP_TAIL], &messages[n - CONDENSE_KEEP_TAIL..])
        } else {
            (&[], messages, &[])
        };

    let conversation_text = messages_to_text(middle);

    let condense_message = format!(
        "{}\n\n---\n\n{}",
        conversation_text, CONDENSE_USER_PROMPT
    );

    // Route the condensing call to a cheaper model in the same family.
    // Summarization is a routine transform; Haiku/gpt-4o-mini handle it fine
    // at a fraction of the cost, and we don't waste Opus/Sonnet tokens on it.
    let condense_model = cheaper_sibling_for(&config.model);

    // Use the model's full max output tokens from the registry so the summary
    // doesn't get truncated. The prompt itself instructs the model to stay brief
    // (under 20k tokens), but we don't want to hard-cap below what's available.
    let condense_max_tokens =
        model_registry::max_output_tokens(&condense_model, config.max_tokens);

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

    let response = provider
        .chat(
            vec![Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: condense_message,
                }],
            }],
            vec![], // no tools
            &condense_config,
            None, // no streaming
        )
        .await?;

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

    let summary_msg = Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: format!(
                "[Context Condensed — the middle of this conversation was summarized to save tokens. \
                 The original task above and the most recent turns below are preserved verbatim.]\n\n{}{}",
                summary, preserved_block
            ),
        }],
    };
    rebuilt.push(summary_msg);

    for msg in tail.iter() {
        let filtered_content: Vec<ContentBlock> = msg
            .content
            .iter()
            .filter(|b| match b {
                ContentBlock::ToolResult { tool_use_id, .. } => {
                    surviving_ids.contains(tool_use_id)
                }
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

    Ok((rebuilt, response.usage))
}

/// Pick a cheaper sibling model from the same provider family for the
/// condensing call. Falls back to the original model when we don't know a
/// good match — e.g. Compatible / custom providers where routing to a
/// different model id would likely fail.
fn cheaper_sibling_for(model: &str) -> String {
    let m = model.to_lowercase();
    if m.contains("claude") {
        // Any Claude model → the current Haiku. ~1/20th the cost of Opus.
        return "claude-haiku-4-5-20251001".to_string();
    }
    if m.starts_with("gpt-4") || m.starts_with("o1") || m.starts_with("o3") {
        return "gpt-4o-mini".to_string();
    }
    if m.starts_with("gemini") {
        return "gemini-1.5-flash".to_string();
    }
    model.to_string()
}

/// Fallback: sliding window truncation when API-based condensing fails.
/// Keeps the first message (original task) and the last ~50% of messages.
pub fn sliding_window_fallback(messages: &[Message]) -> Vec<Message> {
    if messages.len() <= 3 {
        return messages.to_vec();
    }

    let mut result = Vec::new();

    // Keep the first message (original task)
    result.push(messages[0].clone());

    // Insert a truncation notice
    result.push(Message {
        role: Role::Assistant,
        content: vec![ContentBlock::Text {
            text: "[Earlier conversation history was truncated to fit the context window. \
                   Continuing from the most recent messages.]"
                .to_string(),
        }],
    });

    // Keep the last 50% of messages (excluding the first)
    let remaining = &messages[1..];
    let keep_count = remaining.len() / 2;
    let start = remaining.len() - keep_count;
    result.extend_from_slice(&remaining[start..]);

    result
}

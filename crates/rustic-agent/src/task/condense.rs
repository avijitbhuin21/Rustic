use crate::provider::{
    AiProvider, ContentBlock, Message, ProviderConfig, Role,
};
use anyhow::Result;
use std::sync::Arc;

/// Returns the context window size (in tokens) for a given model.
pub fn get_context_window(model: &str, large_context: bool) -> u32 {
    let m = model.to_lowercase();

    if m.contains("claude") {
        if large_context { 1_000_000 } else { 200_000 }
    } else if m.starts_with("gemini") {
        1_048_576
    } else if m.starts_with("gpt-4o") || m.starts_with("gpt-4") {
        128_000
    } else if m.starts_with("o1") || m.starts_with("o3") {
        200_000
    } else {
        128_000 // conservative default
    }
}

/// Returns true if the context is full enough that the next turn is likely to overflow.
/// Uses 80% of the *available* window (after reserving space for output + thinking).
pub fn should_condense(input_tokens: u32, context_window: u32, max_output_tokens: u32, thinking_budget: u32) -> bool {
    if context_window == 0 {
        return false;
    }
    let reserved = max_output_tokens.saturating_add(thinking_budget);
    let available = context_window.saturating_sub(reserved);
    let threshold = (available as f64 * 0.80) as u32;
    input_tokens > threshold
}

const CONDENSE_SYSTEM_PROMPT: &str =
    "You are a conversation summarizer for an AI coding assistant. \
     Respond ONLY with the requested summary. Do NOT call any tools. \
     Do NOT add commentary outside the summary.";

const CONDENSE_USER_PROMPT: &str = r#"The conversation above is the full history of an AI coding task.
Summarize it into a single, detailed context document so that work can continue seamlessly.

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
                        format!("{}... [truncated]", &input_str[..2000])
                    } else {
                        input_str
                    };
                    out.push_str(&format!("[ASSISTANT → Tool: {}] {}\n\n", name, truncated));
                }
                ContentBlock::ToolResult { content, is_error, .. } => {
                    let label = if *is_error { "Tool Error" } else { "Tool Result" };
                    // Truncate very large tool results
                    let truncated = if content.len() > 3000 {
                        format!("{}... [truncated]", &content[..3000])
                    } else {
                        content.clone()
                    };
                    out.push_str(&format!("[{}] {}\n\n", label, truncated));
                }
                ContentBlock::Thinking { .. } | ContentBlock::ModelSwitch { .. } => {
                    // Skip thinking and model-switch blocks
                }
            }
        }
    }

    out
}

/// Condense the conversation by asking the same model to produce a structured summary.
/// Returns a new message vec with a single user message containing the summary.
pub async fn condense_context(
    provider: &Arc<dyn AiProvider>,
    config: &ProviderConfig,
    messages: &[Message],
) -> Result<Vec<Message>> {
    let conversation_text = messages_to_text(messages);

    let condense_message = format!(
        "{}\n\n---\n\n{}",
        conversation_text, CONDENSE_USER_PROMPT
    );

    // Build a lightweight config for the condensing call: no thinking, low temperature
    let condense_config = ProviderConfig {
        api_key: config.api_key.clone(),
        model: config.model.clone(),
        max_tokens: 8192, // summary should be concise
        temperature: 0.0,
        base_url: config.base_url.clone(),
        system_prompt: Some(CONDENSE_SYSTEM_PROMPT.to_string()),
        thinking_budget: 0,
        context_window: 0, // not needed for the condensing call itself
        cancel_token: config.cancel_token.clone(),
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

    Ok(vec![Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: format!(
                "[Context Condensed — this summary replaces the earlier conversation history]\n\n{}",
                summary
            ),
        }],
    }])
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

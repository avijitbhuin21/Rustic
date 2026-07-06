//! Shared AI commit-message generation used by both the desktop (Tauri) and
//! server (Axum) `generate_commit_message` commands. The caller resolves the
//! configured provider entry + diff text; this module owns the prompt, the
//! provider routing, the one-shot chat call, and output cleanup.

use crate::config::ModelCapabilities;
use crate::provider::claude::ClaudeProvider;
use crate::provider::compatible::CompatibleProvider;
use crate::provider::freebuff::FreeBuffProvider;
use crate::provider::gemini::GeminiProvider;
use crate::provider::openai::OpenAiProvider;
use crate::provider::{AiProvider, ContentBlock, Message, ProviderConfig, Role};
use std::sync::Arc;

/// Cap on how much diff text is sent to the model. Beyond this the diff is
/// cut at a char boundary and the prompt notes the truncation.
pub const DIFF_CHAR_CAP: usize = 60_000;

const SYSTEM_PROMPT: &str = "You write git commit messages. Reply with ONLY the commit message itself — no explanations, no surrounding quotes, no markdown code fences.\n\nUse the Conventional Commits format: `<type>(<optional scope>): <summary>` where type is one of feat, fix, refactor, perf, docs, test, build, ci, chore, style, revert — pick whichever genuinely fits the change. The summary line is imperative mood and at most 72 characters. Only when the change set is too varied for one line, add a blank line and 1-4 short `-` bullet lines describing the key changes.";

/// Everything the caller must resolve from its `AiConfig` before generating.
pub struct CommitMessageRequest {
    /// `ProviderEntry::provider_key()` — decides the adapter routing.
    pub provider_key: String,
    pub model: String,
    pub api_key: String,
    pub base_url: Option<String>,
    pub capabilities: ModelCapabilities,
    /// OpenRouter per-model provider allow-list, when restricted.
    pub allowed_providers: Option<Vec<String>>,
}

/// Build the provider adapter for a provider-key / type string, mirroring the
/// routing used for sub-agents (type string decides, never the model name).
fn provider_for(provider_type: &str) -> Arc<dyn AiProvider> {
    if provider_type == "Claude" {
        Arc::new(ClaudeProvider::new())
    } else if provider_type == "OpenAi" {
        Arc::new(OpenAiProvider::new())
    } else if provider_type == "Gemini" {
        Arc::new(GeminiProvider::new())
    } else if provider_type == "FreeBuff" {
        Arc::new(FreeBuffProvider::new())
    } else {
        Arc::new(CompatibleProvider::new(provider_type.to_string()))
    }
}

/// Generate a conventional-commit style message for `diff` via a one-shot
/// chat call to the configured model. Returns the cleaned message text.
pub async fn generate_commit_message(
    req: CommitMessageRequest,
    mut diff: String,
) -> Result<String, String> {
    if diff.trim().is_empty() {
        return Err("No changes to generate a commit message from.".to_string());
    }
    if req.api_key.trim().is_empty() && req.provider_key != "FreeBuff" {
        return Err("The configured provider has no API key.".to_string());
    }

    let mut truncated = false;
    if diff.chars().count() > DIFF_CHAR_CAP {
        let cut = diff
            .char_indices()
            .nth(DIFF_CHAR_CAP)
            .map(|(i, _)| i)
            .unwrap_or(diff.len());
        diff.truncate(cut);
        truncated = true;
    }

    let user_prompt = format!(
        "Generate a commit message for the following diff{}:\n\n{}",
        if truncated {
            " (truncated — summarize what is visible)"
        } else {
            ""
        },
        diff
    );

    let config = ProviderConfig {
        api_key: req.api_key,
        model: req.model,
        max_tokens: 4096,
        temperature: 0.3,
        base_url: req.base_url,
        system_prompt: Some(SYSTEM_PROMPT.to_string()),
        thinking_budget: 0,
        context_window: 0,
        web_search_enabled: false,
        web_fetch_enabled: false,
        supports_temperature: req.capabilities.supports_temperature,
        supports_reasoning_effort: req.capabilities.supports_reasoning_effort,
        supports_adaptive_thinking: req.capabilities.supports_adaptive_thinking,
        cancel_token: None,
        custom_input_cost: None,
        custom_output_cost: None,
        custom_cache_read_cost: None,
        custom_cache_write_cost: None,
        allowed_providers: req.allowed_providers,
    };

    let provider = provider_for(&req.provider_key);
    let messages = vec![Message {
        role: Role::User,
        content: vec![ContentBlock::Text { text: user_prompt }],
    }];
    let response = provider
        .chat(messages, Vec::new(), &config, None)
        .await
        .map_err(|e| e.to_string())?;

    let mut text = String::new();
    for block in &response.content {
        if let ContentBlock::Text { text: t } = block {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(t);
        }
    }
    let message = clean_message(&text);
    if message.is_empty() {
        return Err("The model returned an empty message.".to_string());
    }
    Ok(message)
}

/// Strip markdown fences / wrapping quotes the model may add despite the
/// prompt asking it not to.
fn clean_message(raw: &str) -> String {
    let mut s = raw.trim();
    if s.starts_with("```") {
        s = s.trim_start_matches("```");
        if let Some(nl) = s.find('\n') {
            let (first, rest) = s.split_at(nl);
            if !first.contains(' ') && first.len() <= 12 {
                s = rest;
            }
        }
        s = s.trim_end_matches("```");
        s = s.trim();
    }
    let s = s.trim_matches('"').trim();
    s.to_string()
}

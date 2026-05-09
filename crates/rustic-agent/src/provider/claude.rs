use super::{
    AiProvider, AiResponse, ContentBlock, Message, ModelInfo, ProviderConfig, ProviderStreamEvent,
    Role, StopReason, StreamCallback, TokenUsage, ToolDef,
};
use anyhow::Result;
use async_trait::async_trait;
use futures::StreamExt;
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub struct ClaudeProvider {
    client: reqwest::Client,
}

impl ClaudeProvider {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }
}

/// Models that accept `thinking: {type: "adaptive"}`. Opus 4.7 *only* accepts
/// adaptive; Opus 4.6 / Sonnet 4.6 accept both but adaptive is the recommended
/// path and the manual form is deprecated there. Everything older (4.5 and
/// below) has to stay on the manual path.
///
/// Match is done against the model id — covers bare aliases like `claude-opus-4-7`
/// and dated snapshots like `claude-opus-4-7-20260501`.
fn supports_adaptive_thinking(model: &str) -> bool {
    model.contains("opus-4-7")
        || model.contains("opus-4-6")
        || model.contains("sonnet-4-6")
        || model.contains("mythos")
}

/// Map the user-facing `thinking_budget` token count onto an adaptive-mode
/// effort level. The breakpoints roughly mirror the common manual budgets
/// users pick today (2k/8k/20k/40k) so behavior stays recognizable across the
/// switch. Only called when `thinking_budget > 0`.
fn budget_to_effort(budget: u32) -> &'static str {
    match budget {
        0..=4_000 => "low",
        4_001..=16_000 => "medium",
        16_001..=32_000 => "high",
        _ => "max",
    }
}

#[async_trait]
impl AiProvider for ClaudeProvider {
    async fn chat(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDef>,
        config: &ProviderConfig,
        stream_cb: Option<StreamCallback>,
    ) -> Result<AiResponse> {
        let url = config
            .base_url
            .as_deref()
            .unwrap_or("https://api.anthropic.com/v1/messages");

        let (msg_system, api_messages) = convert_messages(&messages);
        let system_msg = config.system_prompt.as_deref().or(msg_system.as_deref());

        // When thinking is enabled the API requires max_tokens > budget_tokens.
        // Clamp upward so a registry miss (unknown short-form model ID) can never
        // cause a 400 error; the model itself enforces the real upper limit.
        let effective_max_tokens = if config.thinking_budget > 0 {
            config.max_tokens.max(config.thinking_budget + 1024)
        } else {
            config.max_tokens
        };

        // Apply prompt caching: mark the last user-turn content block with
        // cache_control so the full prefix (system + tools + all prior turns)
        // is cached. Anthropic's cache has a 5-minute TTL — turn-by-turn work
        // inside the TTL window reads from cache at 10% of the input cost.
        let api_messages = apply_message_cache_breakpoint(api_messages);

        let mut body = json!({
            "model": config.model,
            "max_tokens": effective_max_tokens,
            "stream": true,
            "messages": api_messages,
        });

        if let Some(sys) = system_msg {
            // System prompt as a cached text block — same 5-minute TTL.
            body["system"] = json!([
                {
                    "type": "text",
                    "text": sys,
                    "cache_control": { "type": "ephemeral" }
                }
            ]);
        }

        // Extended thinking: when enabled, temperature must not be set.
        //
        // Opus 4.7 only accepts `type: "adaptive"` and rejects the legacy
        // `{type: "enabled", budget_tokens}` form with a 400 error. Opus 4.6
        // and Sonnet 4.6 accept both but adaptive is Anthropic's recommended
        // path — the manual form is deprecated there. Everything older (4.5
        // and below) only understands the manual form.
        //
        // We translate the user-facing `thinking_budget` (a token count) into
        // an effort level for adaptive mode. Setting it to 0 disables thinking
        // entirely regardless of model.
        if config.thinking_budget > 0 && config.supports_reasoning_effort {
            if supports_adaptive_thinking(&config.model) {
                body["thinking"] = json!({
                    "type": "adaptive",
                    // Opus 4.7 defaults `display` to "omitted" (thinking field
                    // empty, signature still present). Force summarized so the
                    // UI can render thinking blocks like on older models.
                    "display": "summarized",
                });
                body["output_config"] = json!({
                    "effort": budget_to_effort(config.thinking_budget),
                });
            } else {
                body["thinking"] = json!({
                    "type": "enabled",
                    "budget_tokens": config.thinking_budget,
                });
            }
        } else if config.temperature != 0.7 && config.supports_temperature {
            body["temperature"] = json!(config.temperature);
        }

        // Anthropic exposes web_search / web_fetch as SERVER-side tools. When
        // the user has enabled either, we swap the generic ToolDef (if the
        // executor added one) out of the request body and inject the
        // server-side declaration instead. This way the model only sees one
        // version of each tool and Anthropic executes the network call.
        let mut claude_tools: Vec<serde_json::Value> = Vec::new();
        let mut saw_web_search = false;
        let mut saw_web_fetch = false;
        for t in &tools {
            match t.name.as_str() {
                "web_search" => { saw_web_search = true; }
                "web_fetch" => { saw_web_fetch = true; }
                _ => {
                    claude_tools.push(json!({
                        "name": t.name,
                        "description": t.description,
                        "input_schema": t.parameters,
                    }));
                }
            }
        }
        // Server-side declarations. The `name` field for these must match what
        // the model uses in `server_tool_use.name` — Anthropic uses the
        // short alias ("web_search" / "web_fetch") by convention.
        if saw_web_search || config.web_search_enabled {
            claude_tools.push(json!({
                "type": "web_search_20250305",
                "name": "web_search",
                "max_uses": 8,
            }));
        }
        if saw_web_fetch || config.web_fetch_enabled {
            claude_tools.push(json!({
                "type": "web_fetch_20250910",
                "name": "web_fetch",
                "max_uses": 8,
            }));
        }

        if !claude_tools.is_empty() {
            // Mark the last tool with cache_control — caches the entire tool
            // definitions block (which rarely changes between turns).
            let last_idx = claude_tools.len() - 1;
            for (i, t) in claude_tools.iter_mut().enumerate() {
                if i == last_idx {
                    if let Some(obj) = t.as_object_mut() {
                        obj.insert("cache_control".to_string(), json!({ "type": "ephemeral" }));
                    }
                }
            }
            body["tools"] = json!(claude_tools);
        }

        let mut request = self
            .client
            .post(url)
            .header("x-api-key", &config.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json");

        // Build comma-separated anthropic-beta header from all enabled betas.
        let mut betas: Vec<&str> = Vec::new();
        if config.thinking_budget > 0 && !supports_adaptive_thinking(&config.model) {
            // Interleaved thinking is only valid on the manual-mode path for
            // pre-4.6 models; adaptive mode enables it automatically and the
            // beta header is not valid there.
            betas.push("interleaved-thinking-2025-05-14");
        }
        if saw_web_search || config.web_search_enabled {
            betas.push("web-search-2025-03-05");
        }
        if saw_web_fetch || config.web_fetch_enabled {
            betas.push("web-fetch-2025-09-10");
        }
        if !betas.is_empty() {
            request = request.header("anthropic-beta", betas.join(","));
        }

        let resp = request.json(&body).send().await?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await?;
            return Err(anyhow::anyhow!("Claude API error {}: {}", status, text));
        }

        parse_sse_stream(resp, stream_cb, config.cancel_token.clone()).await
    }

    fn name(&self) -> &str {
        "Claude"
    }

    fn available_models(&self) -> Vec<ModelInfo> {
        crate::model_registry::models_for_provider("Claude")
            .into_iter()
            .map(|m| ModelInfo {
                id: m.id.to_string(),
                name: m.name.to_string(),
                max_tokens: m.max_output_tokens,
                input_cost_per_m: m.input_cost_per_m,
                output_cost_per_m: m.output_cost_per_m,
            })
            .collect()
    }
}

// === SSE stream parsing ===

/// State for a single content block being accumulated from the stream.
enum BlockState {
    Text { text: String },
    Thinking { thinking: String, signature: Option<String> },
    ToolUse { id: String, name: String, input_json: String },
    /// Anthropic server-side web_search_tool_result block. Content is attached
    /// to content_block_start (no streaming delta events); we just carry it
    /// through to the final ContentBlock list.
    WebSearchToolResult { tool_use_id: String, content: serde_json::Value },
    /// Anthropic server-side web_fetch_tool_result block.
    WebFetchToolResult { tool_use_id: String, content: serde_json::Value },
}

async fn parse_sse_stream(
    resp: reqwest::Response,
    stream_cb: Option<StreamCallback>,
    cancel_token: Option<Arc<AtomicBool>>,
) -> Result<AiResponse> {
    let mut byte_stream = resp.bytes_stream();
    let mut buffer = String::new();

    // Accumulated state
    let mut blocks: HashMap<usize, BlockState> = HashMap::new();
    let mut block_order: Vec<usize> = Vec::new(); // insertion-order of block indices
    let mut input_tokens: u32 = 0;
    let mut output_tokens: u32 = 0;
    let mut cache_read_tokens: u32 = 0;
    let mut cache_write_tokens: u32 = 0;
    let mut stop_reason = StopReason::EndTurn;

    while let Some(chunk) = byte_stream.next().await {
        // Check cancellation token to abort streaming early
        if let Some(ref token) = cancel_token {
            if token.load(Ordering::SeqCst) {
                return Err(anyhow::anyhow!("Task cancelled"));
            }
        }

        let chunk = chunk?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));

        // Process complete lines from the buffer
        loop {
            match buffer.find('\n') {
                None => break,
                Some(pos) => {
                    let line = buffer[..pos].trim_end_matches('\r').to_string();
                    buffer = buffer[pos + 1..].to_string();

                    if !line.starts_with("data: ") {
                        continue;
                    }

                    let data = &line["data: ".len()..];
                    if data == "[DONE]" {
                        break;
                    }

                    let event: SseEvent = match serde_json::from_str(data) {
                        Ok(e) => e,
                        Err(_) => continue, // skip unparseable lines
                    };

                    match event {
                        SseEvent::MessageStart { message } => {
                            if let Some(u) = message.usage {
                                input_tokens += u.input_tokens.unwrap_or(0);
                                cache_read_tokens += u.cache_read_input_tokens;
                                cache_write_tokens += u.cache_creation_input_tokens;
                            }
                        }

                        SseEvent::ContentBlockStart { index, content_block } => {
                            if !block_order.contains(&index) {
                                block_order.push(index);
                            }
                            let state = match content_block {
                                SseContentBlock::Text { .. } => BlockState::Text { text: String::new() },
                                SseContentBlock::Thinking { .. } => BlockState::Thinking { thinking: String::new(), signature: None },
                                SseContentBlock::ToolUse { id, name } => BlockState::ToolUse {
                                    id,
                                    name,
                                    input_json: String::new(),
                                },
                                // Anthropic server_tool_use behaves identically to client-side
                                // tool_use on the wire — same streaming pattern for input JSON.
                                // We reuse BlockState::ToolUse so the rest of the parser is
                                // unchanged; the executor distinguishes server-side calls by
                                // finding a paired ToolResult in the same response.
                                SseContentBlock::ServerToolUse { id, name } => BlockState::ToolUse {
                                    id,
                                    name,
                                    input_json: String::new(),
                                },
                                SseContentBlock::WebSearchToolResult { tool_use_id, content } => {
                                    // Result content is final at block_start (no streaming
                                    // deltas). Emit the UI event immediately so the tool card
                                    // appears in-place, before any subsequent text deltas.
                                    if let Some(cb) = &stream_cb {
                                        let stringified = serde_json::to_string(&content)
                                            .unwrap_or_else(|_| content.to_string());
                                        cb(ProviderStreamEvent::ServerToolResult {
                                            tool_use_id: tool_use_id.clone(),
                                            content: stringified,
                                            is_error: false,
                                        });
                                    }
                                    BlockState::WebSearchToolResult { tool_use_id, content }
                                }
                                SseContentBlock::WebFetchToolResult { tool_use_id, content } => {
                                    if let Some(cb) = &stream_cb {
                                        let stringified = serde_json::to_string(&content)
                                            .unwrap_or_else(|_| content.to_string());
                                        cb(ProviderStreamEvent::ServerToolResult {
                                            tool_use_id: tool_use_id.clone(),
                                            content: stringified,
                                            is_error: false,
                                        });
                                    }
                                    BlockState::WebFetchToolResult { tool_use_id, content }
                                }
                            };
                            blocks.insert(index, state);
                        }

                        SseEvent::ContentBlockDelta { index, delta } => {
                            match delta {
                                SseDelta::TextDelta { text } => {
                                    if let Some(cb) = &stream_cb {
                                        cb(ProviderStreamEvent::TextDelta(text.clone()));
                                    }
                                    if let Some(BlockState::Text { text: acc }) = blocks.get_mut(&index) {
                                        acc.push_str(&text);
                                    }
                                }
                                SseDelta::ThinkingDelta { thinking } => {
                                    if let Some(cb) = &stream_cb {
                                        cb(ProviderStreamEvent::ThinkingDelta(thinking.clone()));
                                    }
                                    if let Some(BlockState::Thinking { thinking: acc, .. }) = blocks.get_mut(&index) {
                                        acc.push_str(&thinking);
                                    }
                                }
                                SseDelta::SignatureDelta { signature } => {
                                    if let Some(BlockState::Thinking { signature: sig, .. }) = blocks.get_mut(&index) {
                                        let s = sig.get_or_insert_with(String::new);
                                        s.push_str(&signature);
                                    }
                                }
                                SseDelta::InputJsonDelta { partial_json } => {
                                    if let Some(BlockState::ToolUse { input_json, .. }) = blocks.get_mut(&index) {
                                        input_json.push_str(&partial_json);
                                    }
                                }
                            }
                        }

                        SseEvent::MessageDelta { delta, usage } => {
                            stop_reason = match delta.stop_reason.as_deref() {
                                Some("end_turn") => StopReason::EndTurn,
                                Some("tool_use") => StopReason::ToolUse,
                                Some("max_tokens") => StopReason::MaxTokens,
                                Some(other) => StopReason::Error(other.to_string()),
                                None => StopReason::EndTurn,
                            };
                            if let Some(u) = usage {
                                output_tokens += u.output_tokens.unwrap_or(0);
                            }
                        }

                        SseEvent::Error { error } => {
                            return Err(anyhow::anyhow!("Claude stream error: {}", error.message));
                        }

                        SseEvent::ContentBlockStop { index } => {
                            // When a server_tool_use (web_search / web_fetch) finishes
                            // streaming its input, emit an inline UI event so the
                            // tool card appears BEFORE the subsequent text deltas
                            // — same chronological order the model emitted them.
                            if let Some(BlockState::ToolUse { id, name, input_json }) = blocks.get(&index) {
                                if (name == "web_search" || name == "web_fetch")
                                    && stream_cb.is_some()
                                {
                                    let input: serde_json::Value = if input_json.trim().is_empty() {
                                        json!({})
                                    } else {
                                        serde_json::from_str(input_json).unwrap_or_else(|_| json!({}))
                                    };
                                    if let Some(cb) = &stream_cb {
                                        cb(ProviderStreamEvent::ServerToolUse {
                                            id: id.clone(),
                                            name: name.clone(),
                                            input,
                                        });
                                    }
                                }
                            }
                        }
                        SseEvent::MessageStop | SseEvent::Ping => {}
                    }
                }
            }
        }
    }

    // Build content blocks in order
    let mut content: Vec<ContentBlock> = Vec::new();
    block_order.sort_unstable();
    for idx in block_order {
        if let Some(state) = blocks.remove(&idx) {
            match state {
                BlockState::Text { text } if !text.is_empty() => {
                    content.push(ContentBlock::Text { text });
                }
                BlockState::Thinking { thinking, signature } if !thinking.is_empty() => {
                    content.push(ContentBlock::Thinking { thinking, signature, duration_secs: None });
                }
                BlockState::ToolUse { id, name, input_json } => {
                    // Empty input_json is legitimate for tools that take no
                    // parameters (e.g. list_projects, list_active_agents).
                    // Claude sends no input_json_delta events in that case,
                    // which is correct per the streaming spec. Treat it as
                    // an empty object so downstream dispatch doesn't choke.
                    let input: serde_json::Value = if input_json.trim().is_empty() {
                        json!({})
                    } else {
                        match serde_json::from_str(&input_json) {
                            Ok(v) => v,
                            Err(e) => {
                                tracing::warn!("[claude] WARNING: tool '{}' (id={}) has malformed input_json: {} — raw: {:?}",
                                    name, id, e, &input_json[..input_json.len().min(200)]);
                                json!({ "__parse_error": format!("Failed to parse tool arguments: {}. Please retry the tool call with valid JSON parameters.", e) })
                            }
                        }
                    };
                    content.push(ContentBlock::ToolUse { id, name, input, thought_signature: None });
                }
                BlockState::WebSearchToolResult { tool_use_id, content: result_content } => {
                    // Stringify the result array for transport through the
                    // generic ContentBlock::ToolResult. The round-trip
                    // serializer (convert_content_blocks) detects web_search
                    // pairs and re-inflates the JSON back to the server
                    // result-array shape before sending to the API.
                    let stringified = serde_json::to_string(&result_content)
                        .unwrap_or_else(|_| result_content.to_string());
                    content.push(ContentBlock::ToolResult {
                        tool_use_id,
                        content: stringified,
                        is_error: false,
                    });
                }
                BlockState::WebFetchToolResult { tool_use_id, content: result_content } => {
                    let stringified = serde_json::to_string(&result_content)
                        .unwrap_or_else(|_| result_content.to_string());
                    content.push(ContentBlock::ToolResult {
                        tool_use_id,
                        content: stringified,
                        is_error: false,
                    });
                }
                _ => {}
            }
        }
    }

    Ok(AiResponse {
        content,
        usage: TokenUsage {
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_write_tokens,
        },
        stop_reason,
    })
}

// === SSE event types ===

#[derive(Deserialize)]
#[serde(tag = "type")]
#[allow(dead_code)]
enum SseEvent {
    #[serde(rename = "message_start")]
    MessageStart { message: SseMessage },
    #[serde(rename = "content_block_start")]
    ContentBlockStart { index: usize, content_block: SseContentBlock },
    #[serde(rename = "content_block_delta")]
    ContentBlockDelta { index: usize, delta: SseDelta },
    #[serde(rename = "content_block_stop")]
    ContentBlockStop { index: usize },
    #[serde(rename = "message_delta")]
    MessageDelta { delta: SseMessageDelta, usage: Option<SseUsage> },
    #[serde(rename = "message_stop")]
    MessageStop,
    #[serde(rename = "ping")]
    Ping,
    #[serde(rename = "error")]
    Error { error: SseError },
}

#[derive(Deserialize)]
struct SseMessage {
    usage: Option<SseUsage>,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
#[allow(dead_code)]
enum SseContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "thinking")]
    Thinking { thinking: String },
    #[serde(rename = "tool_use")]
    ToolUse { id: String, name: String },
    /// Anthropic server-side tool call (web_search / web_fetch). Streams input
    /// via input_json_delta like a normal tool_use.
    #[serde(rename = "server_tool_use")]
    ServerToolUse { id: String, name: String },
    /// Anthropic-executed web_search result. Content is fully present at
    /// block_start — no streaming deltas.
    #[serde(rename = "web_search_tool_result")]
    WebSearchToolResult {
        tool_use_id: String,
        content: serde_json::Value,
    },
    /// Anthropic-executed web_fetch result.
    #[serde(rename = "web_fetch_tool_result")]
    WebFetchToolResult {
        tool_use_id: String,
        content: serde_json::Value,
    },
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum SseDelta {
    #[serde(rename = "text_delta")]
    TextDelta { text: String },
    #[serde(rename = "thinking_delta")]
    ThinkingDelta { thinking: String },
    #[serde(rename = "signature_delta")]
    SignatureDelta { signature: String },
    #[serde(rename = "input_json_delta")]
    InputJsonDelta { partial_json: String },
}

#[derive(Deserialize)]
struct SseMessageDelta {
    stop_reason: Option<String>,
}

#[derive(Deserialize)]
struct SseUsage {
    #[serde(default)]
    input_tokens: Option<u32>,
    #[serde(default)]
    output_tokens: Option<u32>,
    #[serde(default)]
    cache_read_input_tokens: u32,
    #[serde(default)]
    cache_creation_input_tokens: u32,
}

#[derive(Deserialize)]
struct SseError {
    message: String,
}

// === Message conversion ===

fn convert_messages(messages: &[Message]) -> (Option<String>, Vec<serde_json::Value>) {
    let mut system = None;
    let mut api_msgs = Vec::new();

    for msg in messages {
        match msg.role {
            Role::System => {
                if let Some(ContentBlock::Text { text }) = msg.content.first() {
                    system = Some(text.clone());
                }
            }
            Role::User => {
                let content = convert_content_blocks(&msg.content);
                api_msgs.push(json!({ "role": "user", "content": content }));
            }
            Role::Assistant => {
                let content = convert_content_blocks(&msg.content);
                api_msgs.push(json!({ "role": "assistant", "content": content }));
            }
        }
    }

    (system, api_msgs)
}

/// Stamp `cache_control: ephemeral` on a "rolling stable prefix" breakpoint —
/// specifically, the last block of the *second-to-last* message. We also
/// place one on the very last message so the current turn seeds the next
/// request's cache.
///
/// Why two breakpoints: Anthropic allows up to 4 cache_control markers and
/// matches the longest cached prefix. By marking N-1 as well as N, the prefix
/// through N-1 stays stable across turns (it doesn't change between requests),
/// so request N+1 gets a cache hit on everything up through turn N-1 — which
/// would otherwise be invalidated because the "last message" on every request
/// is always different.
///
/// On a single-message conversation, only the last gets marked.
fn apply_message_cache_breakpoint(mut msgs: Vec<serde_json::Value>) -> Vec<serde_json::Value> {
    let len = msgs.len();
    let mark = |msg: &mut serde_json::Value| {
        if let Some(content) = msg.get_mut("content").and_then(|c| c.as_array_mut()) {
            if let Some(last_block) = content.last_mut() {
                if let Some(obj) = last_block.as_object_mut() {
                    obj.insert(
                        "cache_control".to_string(),
                        json!({ "type": "ephemeral" }),
                    );
                }
            }
        }
    };
    if len >= 2 {
        // Rolling prefix: covers everything through the previous turn.
        mark(&mut msgs[len - 2]);
    }
    if len >= 1 {
        // Seed for the NEXT request's cache.
        mark(&mut msgs[len - 1]);
    }
    msgs
}

fn convert_content_blocks(blocks: &[ContentBlock]) -> serde_json::Value {
    // Build a map of tool_use_id → tool_name for the blocks in THIS message.
    // When an assistant turn contains a ToolUse named web_search/web_fetch
    // paired with a ToolResult for the same id, both blocks came from
    // Anthropic's server-side execution and must be re-serialized using the
    // native server_tool_use / web_*_tool_result shapes. Client-side results
    // (produced locally for OpenAI/Compatible) always arrive in a separate
    // user turn, so they never match by same-message pairing.
    let mut server_tool_ids: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for b in blocks {
        if let ContentBlock::ToolUse { id, name, .. } = b {
            if name == "web_search" || name == "web_fetch" {
                server_tool_ids.insert(id.clone(), name.clone());
            }
        }
    }

    let parts: Vec<serde_json::Value> = blocks
        .iter()
        .map(|b| match b {
            ContentBlock::Text { text } => json!({ "type": "text", "text": text }),
            ContentBlock::ToolUse { id, name, input, .. } => {
                let block_type = if name == "web_search" || name == "web_fetch" {
                    "server_tool_use"
                } else {
                    "tool_use"
                };
                json!({ "type": block_type, "id": id, "name": name, "input": input })
            }
            ContentBlock::ToolResult { tool_use_id, content, is_error } => {
                if let Some(server_name) = server_tool_ids.get(tool_use_id) {
                    // Server-side result — re-inflate the stringified content
                    // back to the original JSON array/object so Anthropic sees
                    // the exact shape it emitted last turn.
                    let result_type = if server_name == "web_search" {
                        "web_search_tool_result"
                    } else {
                        "web_fetch_tool_result"
                    };
                    let content_val: serde_json::Value = serde_json::from_str(content)
                        .unwrap_or_else(|_| json!(content));
                    json!({
                        "type": result_type,
                        "tool_use_id": tool_use_id,
                        "content": content_val,
                    })
                } else {
                    json!({
                        "type": "tool_result",
                        "tool_use_id": tool_use_id,
                        "content": content,
                        "is_error": is_error,
                    })
                }
            }
            ContentBlock::Thinking { thinking, signature, .. } => {
                // Must be re-sent to API when present in assistant turns
                let mut obj = json!({ "type": "thinking", "thinking": thinking });
                if let Some(sig) = signature {
                    obj["signature"] = json!(sig);
                }
                obj
            }
            ContentBlock::Image { media_type, data } => {
                json!({
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": media_type,
                        "data": data,
                    }
                })
            }
            // UI-only marker — filtered out before reaching the provider
            ContentBlock::ModelSwitch { .. } => json!(null),
        })
        .filter(|v| !v.is_null())
        .collect();
    json!(parts)
}

#[cfg(test)]
mod sse_snapshot_tests {
    //! Snapshot tests for the on-the-wire shape of Anthropic's SSE events.
    //! Catches upstream API drift before users do — if Anthropic renames a
    //! field or adds a required one, deserialization here fails.
    use super::*;

    fn parse(s: &str) -> SseEvent {
        serde_json::from_str(s).expect("SseEvent should deserialize")
    }

    #[test]
    fn message_start_with_usage() {
        let json = r#"{
            "type": "message_start",
            "message": {
                "usage": {
                    "input_tokens": 100,
                    "output_tokens": 1,
                    "cache_creation_input_tokens": 200,
                    "cache_read_input_tokens": 300
                }
            }
        }"#;
        let evt = parse(json);
        match evt {
            SseEvent::MessageStart { message } => {
                let u = message.usage.expect("usage present");
                assert_eq!(u.input_tokens, Some(100));
                assert_eq!(u.cache_read_input_tokens, 300);
                assert_eq!(u.cache_creation_input_tokens, 200);
            }
            _ => panic!("expected MessageStart"),
        }
    }

    #[test]
    fn content_block_start_text() {
        let json = r#"{
            "type": "content_block_start",
            "index": 0,
            "content_block": {"type": "text", "text": ""}
        }"#;
        let evt = parse(json);
        match evt {
            SseEvent::ContentBlockStart { index, content_block } => {
                assert_eq!(index, 0);
                assert!(matches!(content_block, SseContentBlock::Text { .. }));
            }
            _ => panic!("expected ContentBlockStart"),
        }
    }

    #[test]
    fn content_block_start_tool_use() {
        let json = r#"{
            "type": "content_block_start",
            "index": 1,
            "content_block": {"type": "tool_use", "id": "toolu_xyz", "name": "read_file"}
        }"#;
        let evt = parse(json);
        match evt {
            SseEvent::ContentBlockStart { content_block, .. } => match content_block {
                SseContentBlock::ToolUse { id, name } => {
                    assert_eq!(id, "toolu_xyz");
                    assert_eq!(name, "read_file");
                }
                _ => panic!("expected ToolUse"),
            },
            _ => panic!("expected ContentBlockStart"),
        }
    }

    #[test]
    fn content_block_delta_text() {
        let json = r#"{
            "type": "content_block_delta",
            "index": 0,
            "delta": {"type": "text_delta", "text": "Hello"}
        }"#;
        let evt = parse(json);
        match evt {
            SseEvent::ContentBlockDelta { delta, .. } => match delta {
                SseDelta::TextDelta { text } => assert_eq!(text, "Hello"),
                _ => panic!("expected TextDelta"),
            },
            _ => panic!("expected ContentBlockDelta"),
        }
    }

    #[test]
    fn content_block_delta_input_json() {
        let json = r#"{
            "type": "content_block_delta",
            "index": 1,
            "delta": {"type": "input_json_delta", "partial_json": "{\"path\""}
        }"#;
        let evt = parse(json);
        match evt {
            SseEvent::ContentBlockDelta { delta, .. } => match delta {
                SseDelta::InputJsonDelta { partial_json } => assert_eq!(partial_json, "{\"path\""),
                _ => panic!("expected InputJsonDelta"),
            },
            _ => panic!("expected ContentBlockDelta"),
        }
    }

    #[test]
    fn content_block_delta_thinking() {
        let json = r#"{
            "type": "content_block_delta",
            "index": 0,
            "delta": {"type": "thinking_delta", "thinking": "let me think"}
        }"#;
        let evt = parse(json);
        match evt {
            SseEvent::ContentBlockDelta { delta, .. } => match delta {
                SseDelta::ThinkingDelta { thinking } => assert_eq!(thinking, "let me think"),
                _ => panic!("expected ThinkingDelta"),
            },
            _ => panic!("expected ContentBlockDelta"),
        }
    }

    #[test]
    fn message_stop() {
        let json = r#"{"type": "message_stop"}"#;
        assert!(matches!(parse(json), SseEvent::MessageStop));
    }

    #[test]
    fn ping() {
        let json = r#"{"type": "ping"}"#;
        assert!(matches!(parse(json), SseEvent::Ping));
    }
}

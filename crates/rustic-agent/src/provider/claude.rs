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

        let mut body = json!({
            "model": config.model,
            "max_tokens": config.max_tokens,
            "stream": true,
            "messages": api_messages,
        });

        if let Some(sys) = system_msg {
            body["system"] = json!(sys);
        }

        if config.temperature != 0.7 {
            body["temperature"] = json!(config.temperature);
        }

        if !tools.is_empty() {
            let claude_tools: Vec<serde_json::Value> = tools
                .iter()
                .map(|t| {
                    json!({
                        "name": t.name,
                        "description": t.description,
                        "input_schema": t.parameters,
                    })
                })
                .collect();
            body["tools"] = json!(claude_tools);
        }

        let resp = self
            .client
            .post(url)
            .header("x-api-key", &config.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await?;
            return Err(anyhow::anyhow!("Claude API error {}: {}", status, text));
        }

        parse_sse_stream(resp, stream_cb).await
    }

    fn name(&self) -> &str {
        "Claude"
    }

    fn available_models(&self) -> Vec<ModelInfo> {
        vec![
            ModelInfo { id: "claude-sonnet-4-20250514".into(), name: "Claude Sonnet 4".into(), max_tokens: 8192 },
            ModelInfo { id: "claude-opus-4-20250514".into(), name: "Claude Opus 4".into(), max_tokens: 8192 },
            ModelInfo { id: "claude-haiku-4-20250514".into(), name: "Claude Haiku 4".into(), max_tokens: 8192 },
        ]
    }
}

// === SSE stream parsing ===

/// State for a single content block being accumulated from the stream.
enum BlockState {
    Text { text: String },
    Thinking { thinking: String },
    ToolUse { id: String, name: String, input_json: String },
}

async fn parse_sse_stream(
    resp: reqwest::Response,
    stream_cb: Option<StreamCallback>,
) -> Result<AiResponse> {
    let mut byte_stream = resp.bytes_stream();
    let mut buffer = String::new();

    // Accumulated state
    let mut blocks: HashMap<usize, BlockState> = HashMap::new();
    let mut block_order: Vec<usize> = Vec::new(); // insertion-order of block indices
    let mut input_tokens: u32 = 0;
    let mut output_tokens: u32 = 0;
    let mut cache_read_tokens: u32 = 0;
    let mut stop_reason = StopReason::EndTurn;

    while let Some(chunk) = byte_stream.next().await {
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
                            }
                        }

                        SseEvent::ContentBlockStart { index, content_block } => {
                            if !block_order.contains(&index) {
                                block_order.push(index);
                            }
                            let state = match content_block {
                                SseContentBlock::Text { .. } => BlockState::Text { text: String::new() },
                                SseContentBlock::Thinking { .. } => BlockState::Thinking { thinking: String::new() },
                                SseContentBlock::ToolUse { id, name } => BlockState::ToolUse {
                                    id,
                                    name,
                                    input_json: String::new(),
                                },
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
                                    if let Some(BlockState::Thinking { thinking: acc }) = blocks.get_mut(&index) {
                                        acc.push_str(&thinking);
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

                        SseEvent::ContentBlockStop { .. }
                        | SseEvent::MessageStop
                        | SseEvent::Ping => {}
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
                BlockState::Thinking { thinking } if !thinking.is_empty() => {
                    content.push(ContentBlock::Thinking { thinking });
                }
                BlockState::ToolUse { id, name, input_json } => {
                    let input: serde_json::Value =
                        serde_json::from_str(&input_json).unwrap_or(json!({}));
                    content.push(ContentBlock::ToolUse { id, name, input });
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
        },
        stop_reason,
    })
}

// === SSE event types ===

#[derive(Deserialize)]
#[serde(tag = "type")]
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
enum SseContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "thinking")]
    Thinking { thinking: String },
    #[serde(rename = "tool_use")]
    ToolUse { id: String, name: String },
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum SseDelta {
    #[serde(rename = "text_delta")]
    TextDelta { text: String },
    #[serde(rename = "thinking_delta")]
    ThinkingDelta { thinking: String },
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

fn convert_content_blocks(blocks: &[ContentBlock]) -> serde_json::Value {
    let parts: Vec<serde_json::Value> = blocks
        .iter()
        .map(|b| match b {
            ContentBlock::Text { text } => json!({ "type": "text", "text": text }),
            ContentBlock::ToolUse { id, name, input } => {
                json!({ "type": "tool_use", "id": id, "name": name, "input": input })
            }
            ContentBlock::ToolResult { tool_use_id, content, is_error } => {
                json!({
                    "type": "tool_result",
                    "tool_use_id": tool_use_id,
                    "content": content,
                    "is_error": is_error,
                })
            }
            ContentBlock::Thinking { thinking } => {
                // Must be re-sent to API when present in assistant turns
                json!({ "type": "thinking", "thinking": thinking })
            }
            // UI-only marker — filtered out before reaching the provider
            ContentBlock::ModelSwitch { .. } => json!(null),
        })
        .filter(|v| !v.is_null())
        .collect();
    json!(parts)
}

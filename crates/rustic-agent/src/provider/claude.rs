use super::{
    AiProvider, AiResponse, ContentBlock, Message, ModelInfo, ProviderConfig, Role, StopReason,
    TokenUsage, ToolDef,
};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

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
    ) -> Result<AiResponse> {
        let url = config
            .base_url
            .as_deref()
            .unwrap_or("https://api.anthropic.com/v1/messages");

        // Convert messages to Claude format
        let (system_msg, api_messages) = convert_messages(&messages);

        let mut body = json!({
            "model": config.model,
            "max_tokens": config.max_tokens,
            "messages": api_messages,
        });

        if let Some(sys) = &system_msg {
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
        let text = resp.text().await?;

        if !status.is_success() {
            return Err(anyhow::anyhow!("Claude API error {}: {}", status, text));
        }

        let api_resp: ClaudeResponse = serde_json::from_str(&text)?;
        Ok(convert_response(api_resp))
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

// === Claude API types ===

#[derive(Deserialize)]
struct ClaudeResponse {
    content: Vec<ClaudeContent>,
    usage: ClaudeUsage,
    stop_reason: Option<String>,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum ClaudeContent {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse { id: String, name: String, input: serde_json::Value },
}

#[derive(Deserialize)]
struct ClaudeUsage {
    input_tokens: u32,
    output_tokens: u32,
}

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
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                json!({
                    "type": "tool_result",
                    "tool_use_id": tool_use_id,
                    "content": content,
                    "is_error": is_error,
                })
            }
        })
        .collect();
    json!(parts)
}

fn convert_response(resp: ClaudeResponse) -> AiResponse {
    let content: Vec<ContentBlock> = resp
        .content
        .into_iter()
        .map(|c| match c {
            ClaudeContent::Text { text } => ContentBlock::Text { text },
            ClaudeContent::ToolUse { id, name, input } => ContentBlock::ToolUse { id, name, input },
        })
        .collect();

    let stop_reason = match resp.stop_reason.as_deref() {
        Some("end_turn") => StopReason::EndTurn,
        Some("tool_use") => StopReason::ToolUse,
        Some("max_tokens") => StopReason::MaxTokens,
        Some(other) => StopReason::Error(other.to_string()),
        None => StopReason::EndTurn,
    };

    AiResponse {
        content,
        usage: TokenUsage {
            input_tokens: resp.usage.input_tokens,
            output_tokens: resp.usage.output_tokens,
        },
        stop_reason,
    }
}

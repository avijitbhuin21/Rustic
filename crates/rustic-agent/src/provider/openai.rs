use super::{
    AiProvider, AiResponse, ContentBlock, Message, ModelInfo, ProviderConfig, Role, StopReason,
    StreamCallback, TokenUsage, ToolDef,
};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

pub struct OpenAiProvider {
    client: reqwest::Client,
}

impl OpenAiProvider {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl AiProvider for OpenAiProvider {
    async fn chat(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDef>,
        config: &ProviderConfig,
        _stream_cb: Option<StreamCallback>,
    ) -> Result<AiResponse> {
        let url = format!(
            "{}/chat/completions",
            config.base_url.as_deref().unwrap_or("https://api.openai.com/v1")
        );

        let mut api_messages = convert_messages(&messages);
        // Prepend system prompt if provided (and not already present)
        if let Some(sys) = &config.system_prompt {
            let has_system = api_messages.first()
                .map(|m| m.get("role").and_then(|r| r.as_str()) == Some("system"))
                .unwrap_or(false);
            if !has_system {
                api_messages.insert(0, json!({ "role": "system", "content": sys }));
            }
        }

        let mut body = json!({
            "model": config.model,
            "max_tokens": config.max_tokens,
            "temperature": config.temperature,
            "messages": api_messages,
        });

        if !tools.is_empty() {
            let oai_tools: Vec<serde_json::Value> = tools
                .iter()
                .map(|t| {
                    json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": t.parameters,
                        }
                    })
                })
                .collect();
            body["tools"] = json!(oai_tools);
        }

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", config.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;

        let status = resp.status();
        let text = resp.text().await?;

        if !status.is_success() {
            return Err(anyhow::anyhow!("OpenAI API error {}: {}", status, text));
        }

        let api_resp: OpenAiResponse = serde_json::from_str(&text)?;
        Ok(convert_response(api_resp))
    }

    fn name(&self) -> &str {
        "OpenAI"
    }

    fn available_models(&self) -> Vec<ModelInfo> {
        vec![
            ModelInfo { id: "gpt-4o".into(), name: "GPT-4o".into(), max_tokens: 4096 },
            ModelInfo { id: "gpt-4o-mini".into(), name: "GPT-4o Mini".into(), max_tokens: 4096 },
            ModelInfo { id: "o3-mini".into(), name: "o3-mini".into(), max_tokens: 4096 },
        ]
    }
}

// === OpenAI API types ===

#[derive(Deserialize)]
struct OpenAiResponse {
    choices: Vec<OpenAiChoice>,
    usage: Option<OpenAiUsage>,
}

#[derive(Deserialize)]
struct OpenAiChoice {
    message: OpenAiMessage,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct OpenAiMessage {
    content: Option<String>,
    tool_calls: Option<Vec<OpenAiToolCall>>,
}

#[derive(Deserialize)]
struct OpenAiToolCall {
    id: String,
    function: OpenAiFunction,
}

#[derive(Deserialize)]
struct OpenAiFunction {
    name: String,
    arguments: String, // JSON string
}

#[derive(Deserialize)]
struct OpenAiUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
}

fn convert_messages(messages: &[Message]) -> Vec<serde_json::Value> {
    let mut api_msgs = Vec::new();

    for msg in messages {
        let role = match msg.role {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
        };

        // Check for tool results (sent as separate "tool" role messages in OpenAI)
        for block in &msg.content {
            if let ContentBlock::ToolResult { tool_use_id, content, .. } = block {
                api_msgs.push(json!({
                    "role": "tool",
                    "tool_call_id": tool_use_id,
                    "content": content,
                }));
            }
        }

        // Build content for non-tool-result blocks
        let text_parts: Vec<&str> = msg
            .content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();

        let tool_calls: Vec<serde_json::Value> = msg
            .content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::ToolUse { id, name, input } => Some(json!({
                    "id": id,
                    "type": "function",
                    "function": {
                        "name": name,
                        "arguments": input.to_string(),
                    }
                })),
                _ => None,
            })
            .collect();

        if !text_parts.is_empty() || !tool_calls.is_empty() {
            let mut m = json!({ "role": role });
            if !text_parts.is_empty() {
                m["content"] = json!(text_parts.join("\n"));
            }
            if !tool_calls.is_empty() {
                m["tool_calls"] = json!(tool_calls);
            }
            api_msgs.push(m);
        }
    }

    api_msgs
}

fn convert_response(resp: OpenAiResponse) -> AiResponse {
    let choice = resp.choices.into_iter().next().unwrap_or(OpenAiChoice {
        message: OpenAiMessage {
            content: None,
            tool_calls: None,
        },
        finish_reason: None,
    });

    let mut content = Vec::new();

    if let Some(text) = choice.message.content {
        if !text.is_empty() {
            content.push(ContentBlock::Text { text });
        }
    }

    if let Some(tool_calls) = choice.message.tool_calls {
        for tc in tool_calls {
            let input: serde_json::Value =
                serde_json::from_str(&tc.function.arguments).unwrap_or(json!({}));
            content.push(ContentBlock::ToolUse {
                id: tc.id,
                name: tc.function.name,
                input,
            });
        }
    }

    let stop_reason = match choice.finish_reason.as_deref() {
        Some("stop") => StopReason::EndTurn,
        Some("tool_calls") => StopReason::ToolUse,
        Some("length") => StopReason::MaxTokens,
        Some(other) => StopReason::Error(other.to_string()),
        None => StopReason::EndTurn,
    };

    let usage = resp.usage.map(|u| TokenUsage {
        input_tokens: u.prompt_tokens,
        output_tokens: u.completion_tokens,
        cache_read_tokens: 0,
    }).unwrap_or(TokenUsage { input_tokens: 0, output_tokens: 0, cache_read_tokens: 0 });

    AiResponse {
        content,
        usage,
        stop_reason,
    }
}

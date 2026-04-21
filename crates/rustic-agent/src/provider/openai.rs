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
        let base = config.base_url.as_deref().unwrap_or("https://api.openai.com/v1");
        let base = base
            .trim_end_matches('/')
            .trim_end_matches("/chat/completions")
            .trim_end_matches("/completions")
            .trim_end_matches('/');
        let url = format!("{}/chat/completions", base);

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

        // GPT-5 family supports a `reasoning_effort` parameter. Derive the
        // effort level from the frontend-provided `thinking_budget`, clamped
        // to the levels this specific model actually supports.
        if config.thinking_budget > 0 && is_gpt5_family(&config.model) {
            let effort = budget_to_effort(config.thinking_budget, &config.model);
            body["reasoning_effort"] = json!(effort);
        }

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
        crate::model_registry::models_for_provider("OpenAi")
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
        let has_images = msg.content.iter().any(|b| matches!(b, ContentBlock::Image { .. }));

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

        // Collect text and image parts; if images present, use content-array format
        let text_parts: Vec<&str> = msg
            .content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();

        if !text_parts.is_empty() || has_images || !tool_calls.is_empty() {
            let mut m = json!({ "role": role });
            if has_images {
                // Use multi-part content array (required when images present)
                let mut parts: Vec<serde_json::Value> = text_parts
                    .iter()
                    .map(|t| json!({ "type": "text", "text": t }))
                    .collect();
                for b in &msg.content {
                    if let ContentBlock::Image { media_type, data } = b {
                        parts.push(json!({
                            "type": "image_url",
                            "image_url": { "url": format!("data:{};base64,{}", media_type, data) }
                        }));
                    }
                }
                m["content"] = json!(parts);
            } else if !text_parts.is_empty() {
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
            let input: serde_json::Value = if tc.function.arguments.trim().is_empty() {
                eprintln!("[openai] WARNING: tool '{}' (id={}) has empty arguments string",
                    tc.function.name, tc.id);
                json!({ "__parse_error": "Tool arguments were empty. Please retry the tool call with all required parameters." })
            } else {
                match serde_json::from_str(&tc.function.arguments) {
                    Ok(v) => v,
                    Err(e) => {
                        eprintln!("[openai] WARNING: tool '{}' (id={}) has malformed arguments: {} — raw: {:?}",
                            tc.function.name, tc.id, e,
                            &tc.function.arguments[..tc.function.arguments.len().min(200)]);
                        json!({ "__parse_error": format!("Failed to parse tool arguments: {}. Please retry with valid JSON.", e) })
                    }
                }
            };
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
        cache_write_tokens: 0,
    }).unwrap_or_default();

    AiResponse {
        content,
        usage,
        stop_reason,
    }
}

/// Returns true when `model` is part of the GPT-5 family (the only OpenAI
/// models this app surfaces, and the ones that accept `reasoning_effort`).
fn is_gpt5_family(model: &str) -> bool {
    let m = model.to_lowercase();
    m.starts_with("gpt-5") || m.starts_with("chatgpt-5")
}

/// Does this model accept `reasoning_effort: "minimal"`?
/// Only the base GPT-5 family and the original gpt-5-codex support it —
/// GPT-5.1+ and the dotted codex variants (5.2-codex, 5.3-codex…) do not.
fn supports_minimal(model: &str) -> bool {
    let m = model.to_lowercase();
    if m.starts_with("gpt-5.") || m.starts_with("chatgpt-5.") {
        return false;
    }
    m.starts_with("gpt-5") || m.starts_with("chatgpt-5")
}

/// Does this model accept `reasoning_effort: "xhigh"`?
/// GPT-5.4 and the dotted codex variants (5.2-codex, 5.3-codex…).
fn supports_xhigh(model: &str) -> bool {
    let m = model.to_lowercase();
    if m.starts_with("gpt-5.4") {
        return true;
    }
    // matches "gpt-5.2-codex", "gpt-5.3-codex", etc.
    if m.starts_with("gpt-5.") && m.contains("-codex") {
        return true;
    }
    false
}

/// Map the thinking-budget integer (set by the frontend) to OpenAI's
/// `reasoning_effort` string, clamped to the levels `model` accepts.
fn budget_to_effort(budget: u32, model: &str) -> &'static str {
    // Raw level based on budget alone.
    let raw = if budget <= 1000 {
        "minimal"
    } else if budget <= 5000 {
        "low"
    } else if budget <= 15000 {
        "medium"
    } else if budget <= 25000 {
        "high"
    } else {
        "xhigh"
    };

    // Clamp unsupported levels.
    match raw {
        "minimal" if !supports_minimal(model) => "low",
        "xhigh" if !supports_xhigh(model) => "high",
        other => other,
    }
}

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
use std::sync::Arc;
use std::sync::atomic::Ordering;

pub struct OpenAiProvider {
    client: reqwest::Client,
}

impl OpenAiProvider {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }

    async fn chat_via_responses(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDef>,
        config: &ProviderConfig,
        stream_cb: Option<StreamCallback>,
    ) -> Result<AiResponse> {
        let base = config.base_url.as_deref().unwrap_or("https://api.openai.com/v1");
        let base = base
            .trim_end_matches('/')
            .trim_end_matches("/responses")
            .trim_end_matches('/');
        let url = format!("{}/responses", base);

        let (extracted_instructions, input_items) = convert_messages_to_responses_api(&messages);

        let mut body = json!({
            "model": config.model,
            "max_output_tokens": config.max_tokens,
            "input": input_items,
            "stream": true,
        });

        if let Some(sys) = &config.system_prompt {
            body["instructions"] = json!(sys);
        } else if let Some(instr) = extracted_instructions {
            body["instructions"] = json!(instr);
        }

        if config.thinking_budget > 0 {
            let effort = budget_to_effort(config.thinking_budget, &config.model);
            // "summary": "auto" tells the API to return the reasoning summary text
            body["reasoning"] = json!({ "effort": effort, "summary": "auto" });
        }

        if !tools.is_empty() {
            let oai_tools: Vec<serde_json::Value> = tools
                .iter()
                .map(|t| {
                    json!({
                        "type": "function",
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters,
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
        if !status.is_success() {
            let text = resp.text().await?;
            return Err(anyhow::anyhow!("OpenAI API error {}: {}", status, text));
        }

        parse_responses_sse_stream(resp, stream_cb, config.cancel_token.clone()).await
    }
}

#[async_trait]
impl AiProvider for OpenAiProvider {
    async fn chat(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDef>,
        config: &ProviderConfig,
        stream_cb: Option<StreamCallback>,
    ) -> Result<AiResponse> {
        if is_gpt5_family(&config.model) {
            return self.chat_via_responses(messages, tools, config, stream_cb).await;
        }

        let base = config.base_url.as_deref().unwrap_or("https://api.openai.com/v1");
        let base = base
            .trim_end_matches('/')
            .trim_end_matches("/chat/completions")
            .trim_end_matches("/completions")
            .trim_end_matches('/');
        let url = format!("{}/chat/completions", base);

        let mut api_messages = convert_messages(&messages);
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
            "stream": true,
            // Request usage stats in the final streaming chunk
            "stream_options": { "include_usage": true },
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
        if !status.is_success() {
            let text = resp.text().await?;
            return Err(anyhow::anyhow!("OpenAI API error {}: {}", status, text));
        }

        parse_completions_sse_stream(resp, stream_cb, config.cancel_token.clone()).await
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

// === Chat Completions SSE streaming ===

async fn parse_completions_sse_stream(
    resp: reqwest::Response,
    stream_cb: Option<StreamCallback>,
    cancel_token: Option<Arc<std::sync::atomic::AtomicBool>>,
) -> Result<AiResponse> {
    let mut byte_stream = resp.bytes_stream();
    let mut buffer = String::new();

    let mut full_text = String::new();
    // index -> (id, name, accumulated_args)
    let mut tool_calls: HashMap<usize, (String, String, String)> = HashMap::new();
    let mut finish_reason: Option<String> = None;
    let mut prompt_tokens: u32 = 0;
    let mut completion_tokens: u32 = 0;

    'outer: while let Some(chunk) = byte_stream.next().await {
        if let Some(ref token) = cancel_token {
            if token.load(Ordering::SeqCst) {
                return Err(anyhow::anyhow!("Task cancelled"));
            }
        }

        let chunk = chunk?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));

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
                        break 'outer;
                    }

                    let v: serde_json::Value = match serde_json::from_str(data) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };

                    // Usage chunk (comes with include_usage: true, often has no choices)
                    if let Some(usage) = v.get("usage") {
                        prompt_tokens = usage.get("prompt_tokens").and_then(|u| u.as_u64()).unwrap_or(0) as u32;
                        completion_tokens = usage.get("completion_tokens").and_then(|u| u.as_u64()).unwrap_or(0) as u32;
                    }

                    let choices = match v.get("choices").and_then(|c| c.as_array()) {
                        Some(c) => c.clone(),
                        None => continue,
                    };

                    for choice in &choices {
                        if let Some(fr) = choice.get("finish_reason").and_then(|f| f.as_str()) {
                            if fr != "null" {
                                finish_reason = Some(fr.to_string());
                            }
                        }

                        let delta = match choice.get("delta") {
                            Some(d) => d,
                            None => continue,
                        };

                        // Text delta
                        if let Some(text) = delta.get("content").and_then(|c| c.as_str()) {
                            if !text.is_empty() {
                                if let Some(cb) = &stream_cb {
                                    cb(ProviderStreamEvent::TextDelta(text.to_string()));
                                }
                                full_text.push_str(text);
                            }
                        }

                        // Tool call deltas
                        if let Some(tcs) = delta.get("tool_calls").and_then(|t| t.as_array()) {
                            for tc in tcs {
                                let idx = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                                let entry = tool_calls.entry(idx).or_insert_with(|| (String::new(), String::new(), String::new()));
                                if let Some(id) = tc.get("id").and_then(|i| i.as_str()) {
                                    entry.0 = id.to_string();
                                }
                                if let Some(func) = tc.get("function") {
                                    if let Some(name) = func.get("name").and_then(|n| n.as_str()) {
                                        entry.1 = name.to_string();
                                    }
                                    if let Some(args) = func.get("arguments").and_then(|a| a.as_str()) {
                                        entry.2.push_str(args);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Build content blocks
    let mut content = Vec::new();
    if !full_text.is_empty() {
        content.push(ContentBlock::Text { text: full_text });
    }

    let mut sorted_tcs: Vec<_> = tool_calls.into_iter().collect();
    sorted_tcs.sort_by_key(|(idx, _)| *idx);
    for (_, (id, name, args)) in sorted_tcs {
        let input: serde_json::Value = if args.trim().is_empty() {
            json!({})
        } else {
            match serde_json::from_str(&args) {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(
                        "[openai] WARNING: tool '{}' (id={}) has malformed arguments: {} — raw: {:?}",
                        name, id, e, &args[..args.len().min(200)]
                    );
                    json!({ "__parse_error": format!("Failed to parse tool arguments: {}. Please retry with valid JSON.", e) })
                }
            }
        };
        content.push(ContentBlock::ToolUse { id, name, input, thought_signature: None });
    }

    let stop_reason = match finish_reason.as_deref() {
        Some("stop") => StopReason::EndTurn,
        Some("tool_calls") => StopReason::ToolUse,
        Some("length") => StopReason::MaxTokens,
        Some(other) => StopReason::Error(other.to_string()),
        None => StopReason::EndTurn,
    };

    Ok(AiResponse {
        content,
        usage: TokenUsage {
            input_tokens: prompt_tokens,
            output_tokens: completion_tokens,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        },
        stop_reason,
    })
}

// === Responses API SSE streaming ===

async fn parse_responses_sse_stream(
    resp: reqwest::Response,
    stream_cb: Option<StreamCallback>,
    cancel_token: Option<Arc<std::sync::atomic::AtomicBool>>,
) -> Result<AiResponse> {
    let mut byte_stream = resp.bytes_stream();
    let mut buffer = String::new();

    // Accumulate text per output_index (there's usually one message item)
    let mut text_by_output: HashMap<usize, String> = HashMap::new();
    // Accumulate reasoning summary text per output_index
    let mut thinking_by_output: HashMap<usize, String> = HashMap::new();
    // Track function call items: output_index -> (call_id, name, accumulated_args)
    let mut func_calls: HashMap<usize, (String, String, String)> = HashMap::new();

    let mut final_response: Option<ResponsesApiResponse> = None;

    'outer: while let Some(chunk) = byte_stream.next().await {
        if let Some(ref token) = cancel_token {
            if token.load(Ordering::SeqCst) {
                return Err(anyhow::anyhow!("Task cancelled"));
            }
        }

        let chunk = chunk?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));

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
                        break 'outer;
                    }

                    let v: serde_json::Value = match serde_json::from_str(data) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };

                    let event_type = match v.get("type").and_then(|t| t.as_str()) {
                        Some(t) => t,
                        None => continue,
                    };

                    match event_type {
                        "response.output_text.delta" => {
                            if let Some(delta) = v.get("delta").and_then(|d| d.as_str()) {
                                if !delta.is_empty() {
                                    if let Some(cb) = &stream_cb {
                                        cb(ProviderStreamEvent::TextDelta(delta.to_string()));
                                    }
                                    let idx = v.get("output_index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                                    text_by_output.entry(idx).or_default().push_str(delta);
                                }
                            }
                        }

                        "response.reasoning_summary_text.delta" => {
                            if let Some(delta) = v.get("delta").and_then(|d| d.as_str()) {
                                if !delta.is_empty() {
                                    if let Some(cb) = &stream_cb {
                                        cb(ProviderStreamEvent::ThinkingDelta(delta.to_string()));
                                    }
                                    let idx = v.get("output_index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                                    thinking_by_output.entry(idx).or_default().push_str(delta);
                                }
                            }
                        }

                        "response.output_item.added" => {
                            if let Some(item) = v.get("item") {
                                if item.get("type").and_then(|t| t.as_str()) == Some("function_call") {
                                    let idx = v.get("output_index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                                    let call_id = item.get("call_id").and_then(|c| c.as_str()).unwrap_or("").to_string();
                                    let name = item.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
                                    func_calls.insert(idx, (call_id, name, String::new()));
                                }
                            }
                        }

                        "response.function_call_arguments.delta" => {
                            let idx = v.get("output_index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                            if let Some(delta) = v.get("delta").and_then(|d| d.as_str()) {
                                if let Some(entry) = func_calls.get_mut(&idx) {
                                    entry.2.push_str(delta);
                                }
                            }
                        }

                        "response.completed" => {
                            if let Some(resp_val) = v.get("response") {
                                if let Ok(resp_obj) = serde_json::from_value::<ResponsesApiResponse>(resp_val.clone()) {
                                    final_response = Some(resp_obj);
                                }
                            }
                            break 'outer;
                        }

                        "error" => {
                            let msg = v.get("message").and_then(|m| m.as_str()).unwrap_or("unknown error");
                            return Err(anyhow::anyhow!("OpenAI Responses API stream error: {}", msg));
                        }

                        _ => {}
                    }
                }
            }
        }
    }

    // If we got a completed response, parse it (authoritative source for tool calls etc.)
    if let Some(api_resp) = final_response {
        return Ok(convert_responses_api_response(api_resp));
    }

    // Fallback: build from accumulated streaming state
    let mut content: Vec<ContentBlock> = Vec::new();

    // Emit thinking blocks (lower output indices come first)
    let mut thinking_idxs: Vec<usize> = thinking_by_output.keys().cloned().collect();
    thinking_idxs.sort_unstable();
    for idx in thinking_idxs {
        if let Some(thinking) = thinking_by_output.remove(&idx) {
            if !thinking.is_empty() {
                content.push(ContentBlock::Thinking { thinking, signature: None, duration_secs: None });
            }
        }
    }

    // Emit text blocks
    let mut text_idxs: Vec<usize> = text_by_output.keys().cloned().collect();
    text_idxs.sort_unstable();
    for idx in text_idxs {
        if let Some(text) = text_by_output.remove(&idx) {
            if !text.is_empty() {
                content.push(ContentBlock::Text { text });
            }
        }
    }

    // Emit function call blocks
    let mut fc_idxs: Vec<usize> = func_calls.keys().cloned().collect();
    fc_idxs.sort_unstable();
    let mut has_tool_calls = false;
    for idx in fc_idxs {
        if let Some((call_id, name, args)) = func_calls.remove(&idx) {
            has_tool_calls = true;
            let input: serde_json::Value = if args.trim().is_empty() {
                json!({})
            } else {
                serde_json::from_str(&args).unwrap_or_else(|e| {
                    tracing::warn!("[openai-responses] tool '{}' malformed args: {}", name, e);
                    json!({ "__parse_error": format!("Failed to parse tool arguments: {}. Please retry with valid JSON.", e) })
                })
            };
            content.push(ContentBlock::ToolUse { id: call_id, name, input, thought_signature: None });
        }
    }

    let stop_reason = if has_tool_calls { StopReason::ToolUse } else { StopReason::EndTurn };

    Ok(AiResponse {
        content,
        usage: TokenUsage::default(),
        stop_reason,
    })
}

// === Responses API types ===

#[derive(Deserialize)]
struct ResponsesApiResponse {
    output: Vec<ResponsesApiOutputItem>,
    usage: Option<ResponsesApiUsage>,
    status: Option<String>,
}

#[derive(Deserialize)]
struct ResponsesApiOutputItem {
    #[serde(rename = "type")]
    item_type: String,
    // message item fields
    content: Option<Vec<ResponsesApiContent>>,
    // function_call item fields
    call_id: Option<String>,
    name: Option<String>,
    arguments: Option<String>,
    // reasoning item fields — summary array of {type, text} parts
    #[serde(default)]
    summary: Vec<ResponsesApiContent>,
}

#[derive(Deserialize)]
struct ResponsesApiContent {
    #[serde(rename = "type")]
    content_type: String,
    text: Option<String>,
}

#[derive(Deserialize)]
struct ResponsesApiUsage {
    input_tokens: u32,
    output_tokens: u32,
}

// === Chat Completions message conversion ===

fn convert_messages(messages: &[Message]) -> Vec<serde_json::Value> {
    let mut api_msgs = Vec::new();

    for msg in messages {
        let role = match msg.role {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
        };

        // Tool results are sent as separate "tool" role messages in OpenAI
        for block in &msg.content {
            if let ContentBlock::ToolResult { tool_use_id, content, .. } = block {
                api_msgs.push(json!({
                    "role": "tool",
                    "tool_call_id": tool_use_id,
                    "content": content,
                }));
            }
        }

        let has_images = msg.content.iter().any(|b| matches!(b, ContentBlock::Image { .. }));

        let tool_calls: Vec<serde_json::Value> = msg
            .content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::ToolUse { id, name, input, .. } => Some(json!({
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

// === Responses API message conversion ===

/// Converts internal messages to the flat input-item array the Responses API expects.
/// Returns (instructions, input_items) where instructions is extracted from any System message.
fn convert_messages_to_responses_api(
    messages: &[Message],
) -> (Option<String>, Vec<serde_json::Value>) {
    let mut instructions: Option<String> = None;
    let mut items: Vec<serde_json::Value> = Vec::new();

    for msg in messages {
        match msg.role {
            Role::System => {
                let text: String = msg
                    .content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                if !text.is_empty() {
                    instructions = Some(text);
                }
            }
            Role::User => {
                // Tool results become function_call_output items
                for block in &msg.content {
                    if let ContentBlock::ToolResult { tool_use_id, content, .. } = block {
                        items.push(json!({
                            "type": "function_call_output",
                            "call_id": tool_use_id,
                            "output": content,
                        }));
                    }
                }
                // Text/image blocks become a single message item
                let has_text = msg.content.iter().any(|b| matches!(b, ContentBlock::Text { .. }));
                let has_images = msg.content.iter().any(|b| matches!(b, ContentBlock::Image { .. }));
                if has_text || has_images {
                    let content_parts: Vec<serde_json::Value> = msg
                        .content
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::Text { text } => Some(json!({
                                "type": "input_text",
                                "text": text,
                            })),
                            ContentBlock::Image { media_type, data } => Some(json!({
                                "type": "input_image",
                                "image_url": format!("data:{};base64,{}", media_type, data),
                            })),
                            _ => None,
                        })
                        .collect();
                    items.push(json!({
                        "type": "message",
                        "role": "user",
                        "content": content_parts,
                    }));
                }
            }
            Role::Assistant => {
                // Tool calls become standalone function_call items
                for block in &msg.content {
                    if let ContentBlock::ToolUse { id, name, input, .. } = block {
                        items.push(json!({
                            "type": "function_call",
                            "call_id": id,
                            "name": name,
                            "arguments": input.to_string(),
                        }));
                    }
                }
                // Text becomes a message item
                let text_parts: Vec<&str> = msg
                    .content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect();
                if !text_parts.is_empty() {
                    items.push(json!({
                        "type": "message",
                        "role": "assistant",
                        "content": [{
                            "type": "output_text",
                            "text": text_parts.join("\n"),
                        }],
                    }));
                }
            }
        }
    }

    (instructions, items)
}

// === Response converter for Responses API ===

fn convert_responses_api_response(resp: ResponsesApiResponse) -> AiResponse {
    let mut content = Vec::new();
    let mut has_tool_calls = false;

    for item in resp.output {
        match item.item_type.as_str() {
            "reasoning" => {
                // Collect summary text parts into a single thinking block
                let thinking: String = item
                    .summary
                    .iter()
                    .filter(|p| p.content_type == "summary_text")
                    .filter_map(|p| p.text.as_deref())
                    .collect::<Vec<_>>()
                    .join("\n");
                if !thinking.is_empty() {
                    content.push(ContentBlock::Thinking {
                        thinking,
                        signature: None,
                        duration_secs: None,
                    });
                }
            }
            "message" => {
                if let Some(parts) = item.content {
                    for part in parts {
                        if part.content_type == "output_text" {
                            if let Some(text) = part.text {
                                if !text.is_empty() {
                                    content.push(ContentBlock::Text { text });
                                }
                            }
                        }
                    }
                }
            }
            "function_call" => {
                has_tool_calls = true;
                let id = item.call_id.unwrap_or_default();
                let name = item.name.unwrap_or_default();
                let args = item.arguments.unwrap_or_default();
                let input: serde_json::Value = if args.trim().is_empty() {
                    json!({})
                } else {
                    match serde_json::from_str(&args) {
                        Ok(v) => v,
                        Err(e) => {
                            tracing::warn!(
                                "[openai-responses] WARNING: tool '{}' (id={}) has malformed arguments: {}",
                                name, id, e
                            );
                            json!({ "__parse_error": format!("Failed to parse tool arguments: {}. Please retry with valid JSON.", e) })
                        }
                    }
                };
                content.push(ContentBlock::ToolUse { id, name, input, thought_signature: None });
            }
            _ => {}
        }
    }

    let stop_reason = match resp.status.as_deref() {
        Some("completed") if has_tool_calls => StopReason::ToolUse,
        Some("completed") => StopReason::EndTurn,
        Some("incomplete") => StopReason::MaxTokens,
        Some(other) => StopReason::Error(other.to_string()),
        None => StopReason::EndTurn,
    };

    let usage = resp
        .usage
        .map(|u| TokenUsage {
            input_tokens: u.input_tokens,
            output_tokens: u.output_tokens,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        })
        .unwrap_or_default();

    AiResponse { content, usage, stop_reason }
}

// === Model helpers ===

/// Returns true when `model` is part of the GPT-5 family.
/// These models use the Responses API instead of Chat Completions.
fn is_gpt5_family(model: &str) -> bool {
    let m = model.to_lowercase();
    m.starts_with("gpt-5") || m.starts_with("chatgpt-5")
}

/// Does this model accept `reasoning_effort: "minimal"`?
/// Only the base GPT-5 and gpt-5-codex support it — GPT-5.1+ do not.
fn supports_minimal(model: &str) -> bool {
    let m = model.to_lowercase();
    if m.starts_with("gpt-5.") || m.starts_with("chatgpt-5.") {
        return false;
    }
    m.starts_with("gpt-5") || m.starts_with("chatgpt-5")
}

/// Does this model accept `reasoning_effort: "xhigh"`?
/// GPT-5.4 and dotted codex variants (5.2-codex, 5.3-codex…).
fn supports_xhigh(model: &str) -> bool {
    let m = model.to_lowercase();
    if m.starts_with("gpt-5.4") {
        return true;
    }
    if m.starts_with("gpt-5.") && m.contains("-codex") {
        return true;
    }
    false
}

/// Map the thinking-budget integer to OpenAI's `reasoning_effort` string,
/// clamped to the levels `model` accepts.
fn budget_to_effort(budget: u32, model: &str) -> &'static str {
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

    match raw {
        "minimal" if !supports_minimal(model) => "low",
        "xhigh" if !supports_xhigh(model) => "high",
        other => other,
    }
}

#[cfg(test)]
mod sse_snapshot_tests {
    //! Snapshot tests for OpenAI's API response shapes (Chat Completions
    //! streaming chunks + Responses API output items). Catches drift before
    //! users do — if OpenAI renames a field, deserialization here fails.
    use super::*;

    #[test]
    fn chat_completion_chunk_text_delta() {
        // Standard Chat Completions streaming chunk shape.
        let json = r#"{
            "id": "chatcmpl-x",
            "object": "chat.completion.chunk",
            "model": "gpt-5.4",
            "choices": [{
                "index": 0,
                "delta": {"content": "Hello"},
                "finish_reason": null
            }]
        }"#;
        let v: serde_json::Value = serde_json::from_str(json).expect("valid JSON");
        let delta = v["choices"][0]["delta"].clone();
        assert_eq!(delta["content"].as_str(), Some("Hello"));
    }

    #[test]
    fn chat_completion_chunk_tool_call_start() {
        let json = r#"{
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_abc",
                        "function": {"name": "read_file", "arguments": ""}
                    }]
                }
            }]
        }"#;
        let v: serde_json::Value = serde_json::from_str(json).expect("valid JSON");
        let tool = &v["choices"][0]["delta"]["tool_calls"][0];
        assert_eq!(tool["id"].as_str(), Some("call_abc"));
        assert_eq!(tool["function"]["name"].as_str(), Some("read_file"));
    }

    #[test]
    fn chat_completion_chunk_tool_call_arguments_delta() {
        let json = r#"{
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "function": {"arguments": "{\"pa"}
                    }]
                }
            }]
        }"#;
        let v: serde_json::Value = serde_json::from_str(json).expect("valid JSON");
        assert_eq!(
            v["choices"][0]["delta"]["tool_calls"][0]["function"]["arguments"].as_str(),
            Some("{\"pa")
        );
    }

    #[test]
    fn responses_api_message_output() {
        let json = r#"{
            "output": [{
                "type": "message",
                "content": [{"type": "output_text", "text": "answer"}]
            }],
            "usage": {"input_tokens": 50, "output_tokens": 100},
            "status": "completed"
        }"#;
        let resp: ResponsesApiResponse = serde_json::from_str(json).expect("ResponsesApi parses");
        assert_eq!(resp.output.len(), 1);
        assert_eq!(resp.output[0].item_type, "message");
        let usage = resp.usage.expect("usage present");
        assert_eq!(usage.input_tokens, 50);
        assert_eq!(usage.output_tokens, 100);
    }

    #[test]
    fn responses_api_function_call_output() {
        let json = r#"{
            "output": [{
                "type": "function_call",
                "call_id": "call_xyz",
                "name": "edit_file",
                "arguments": "{\"path\":\"a.rs\"}"
            }],
            "usage": {"input_tokens": 1, "output_tokens": 1}
        }"#;
        let resp: ResponsesApiResponse = serde_json::from_str(json).expect("ResponsesApi parses");
        let item = &resp.output[0];
        assert_eq!(item.item_type, "function_call");
        assert_eq!(item.call_id.as_deref(), Some("call_xyz"));
        assert_eq!(item.name.as_deref(), Some("edit_file"));
    }

    #[test]
    fn responses_api_reasoning_summary() {
        let json = r#"{
            "output": [{
                "type": "reasoning",
                "summary": [
                    {"type": "summary_text", "text": "Step 1"},
                    {"type": "summary_text", "text": "Step 2"}
                ]
            }]
        }"#;
        let resp: ResponsesApiResponse = serde_json::from_str(json).expect("ResponsesApi parses");
        assert_eq!(resp.output[0].summary.len(), 2);
    }
}

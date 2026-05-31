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

        if config.thinking_budget > 0 && config.supports_reasoning_effort {
            let effort = budget_to_effort(config.thinking_budget, &config.model);
            // "summary": "auto" tells the API to return the reasoning summary text
            body["reasoning"] = json!({ "effort": effort, "summary": "auto" });
        }

        // Server-side web_search lives on the Responses API as a built-in
        // tool — `{"type": "web_search"}`. When the user has enabled web
        // search, strip the client-side function-shaped `web_search` ToolDef
        // (so OpenAI doesn't see two tools with the same name) and inject
        // the built-in instead. The model invokes it without us round-
        // tripping the call; the results land as `web_search_call` items
        // plus `url_citation` annotations on the message, which
        // `convert_responses_api_response` lifts into a synthetic
        // `web_search` ToolUse + ToolResult pair — same UI treatment as
        // Anthropic's and Gemini's server-side web search.
        let mut saw_web_search = false;
        let function_tools: Vec<&ToolDef> = tools
            .iter()
            .filter(|t| {
                if t.name == "web_search" {
                    saw_web_search = true;
                    !config.web_search_enabled
                } else {
                    true
                }
            })
            .collect();
        let want_server_web_search = config.web_search_enabled || saw_web_search;

        if !function_tools.is_empty() || want_server_web_search {
            let mut oai_tools: Vec<serde_json::Value> = function_tools
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
            if want_server_web_search {
                oai_tools.push(json!({ "type": "web_search" }));
            }
            body["tools"] = json!(oai_tools);
        }

        let builder = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", config.api_key))
            .header("Content-Type", "application/json");
        let resp = super::send_json_with_retry(builder, &body, "OpenAI").await?;
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

        // Some Compatible-provider hosts reject `temperature` on specific
        // models (e.g. Claude Opus 4.7 returns 400 "temperature not allowed"
        // when proxied through certain OpenAI-compatible gateways). Honour
        // the per-model capability flag so the user can opt out.
        let mut body = json!({
            "model": config.model,
            "max_tokens": config.max_tokens,
            "messages": api_messages,
            "stream": true,
            "stream_options": { "include_usage": true },
        });
        if config.supports_temperature {
            body["temperature"] = json!(config.temperature);
        }

        // Ask OpenRouter / OpenAI-compatible reasoning-enabled providers to
        // surface chain-of-thought tokens. OpenRouter accepts
        // `reasoning: { effort: "<level>" }` on /chat/completions and returns
        // reasoning chunks as `choices[0].delta.reasoning` (string) and/or
        // `delta.reasoning_details[*]`. Without sending this field, providers
        // like DeepSeek-R1, Sonnet-via-OpenRouter, GPT-5-via-OpenRouter etc.
        // generate reasoning internally but do NOT stream it out, so the
        // user's "thinking" panel stays blank.
        //
        // Direct openai.com /chat/completions doesn't accept `reasoning` (the
        // field moved to the Responses API for that vendor). We gate on
        // base_url so a stray request to api.openai.com on a non-GPT-5 model
        // doesn't 400 — the GPT-5 family is already redirected to
        // chat_via_responses higher up, so reaching this path on openai.com
        // means an older model that has no reasoning anyway.
        if config.thinking_budget > 0 && config.supports_reasoning_effort {
            let is_openai_native = config
                .base_url
                .as_deref()
                .map(|u| u.contains("api.openai.com"))
                .unwrap_or(true); // None defaults to openai.com — see line above.
            if !is_openai_native {
                let effort = budget_to_effort(config.thinking_budget, &config.model);
                body["reasoning"] = json!({ "effort": effort });
            }
        }

        // OpenRouter provider routing: restrict to the user's selected providers
        // for this model, in their chosen priority order. `order` lists the
        // providers to try first-to-last; `allow_fallbacks: false` makes it a
        // strict allow-list — OpenRouter won't route outside the list, and fails
        // if none of them can serve the request rather than using a deselected
        // provider. Empty / None → no `provider` field, so OpenRouter routes
        // across all providers as usual. Gated on the OpenRouter base URL so the
        // field never leaks to other OpenAI-compatible hosts that would reject it.
        if let Some(providers) = config.allowed_providers.as_ref().filter(|p| !p.is_empty()) {
            let is_openrouter = config
                .base_url
                .as_deref()
                .map(|u| u.contains("openrouter.ai"))
                .unwrap_or(false);
            if is_openrouter {
                body["provider"] = json!({
                    "order": providers,
                    "allow_fallbacks": false,
                });
            }
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

        let builder = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", config.api_key))
            .header("Content-Type", "application/json");
        let resp = super::send_json_with_retry(builder, &body, "OpenAI").await?;
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
    // OpenRouter and a few other OpenAI-compatible providers stream
    // chain-of-thought as `choices[0].delta.reasoning` (string) and/or
    // `choices[0].delta.reasoning_details[*]` (array of typed parts).
    // We accumulate every chunk here so the final response can carry one
    // `ContentBlock::Thinking` block alongside the text — same shape the
    // Anthropic / Responses-API paths produce, so downstream
    // `TaskEvent::ThinkingDelta` / `ContentBlock::Thinking` consumers don't
    // need to know which API surface produced the reasoning.
    let mut full_thinking = String::new();
    // index -> (id, name, accumulated_args)
    let mut tool_calls: HashMap<usize, (String, String, String)> = HashMap::new();
    // Set of tool_call indices for which ToolUseStart has already fired —
    // prevents re-emit if id/name appear in multiple chunks.
    let mut tool_starts_emitted: std::collections::HashSet<usize> = std::collections::HashSet::new();
    let mut finish_reason: Option<String> = None;
    let mut prompt_tokens: u32 = 0;
    let mut completion_tokens: u32 = 0;
    let mut cache_read_tokens: u32 = 0;
    // OpenRouter reports the authoritative request cost (`usage.cost`) and the
    // upstream provider that served it (top-level `provider`). Captured here so
    // cost tracking uses OpenRouter's figure rather than tokens × price, which
    // is wrong when a model routes to different-priced providers.
    let mut actual_cost_usd: Option<f64> = None;
    let mut served_provider: Option<String> = None;

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
                        // OpenRouter (and some OpenAI-compatible hosts) report cached
                        // prompt tokens and an authoritative USD cost in the usage chunk.
                        if let Some(cached) = usage
                            .get("prompt_tokens_details")
                            .and_then(|d| d.get("cached_tokens"))
                            .and_then(|u| u.as_u64())
                        {
                            cache_read_tokens = cached as u32;
                        }
                        if let Some(cost) = usage.get("cost").and_then(|c| c.as_f64()) {
                            actual_cost_usd = Some(cost);
                        }
                    }
                    // Top-level `provider` names the upstream that served the request
                    // (present on OpenRouter chunks). Keep the last non-empty value.
                    if let Some(p) = v.get("provider").and_then(|p| p.as_str()) {
                        if !p.is_empty() {
                            served_provider = Some(p.to_string());
                        }
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

                        // Reasoning / thinking delta — OpenRouter chat
                        // completions surfaces it under `delta.reasoning`
                        // (string). Some providers additionally / instead
                        // emit `delta.reasoning_details` as an array of
                        // typed parts (`{ type: "reasoning.text", text: "…" }`,
                        // or encrypted shapes we currently can't render);
                        // we extract any `text` field we find. This is a
                        // best-effort union — unknown shapes are skipped
                        // rather than erroring.
                        if let Some(reason) = delta.get("reasoning").and_then(|r| r.as_str()) {
                            if !reason.is_empty() {
                                if let Some(cb) = &stream_cb {
                                    cb(ProviderStreamEvent::ThinkingDelta(reason.to_string()));
                                }
                                full_thinking.push_str(reason);
                            }
                        }
                        if let Some(parts) = delta.get("reasoning_details").and_then(|d| d.as_array()) {
                            for part in parts {
                                if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                                    if !text.is_empty() {
                                        if let Some(cb) = &stream_cb {
                                            cb(ProviderStreamEvent::ThinkingDelta(text.to_string()));
                                        }
                                        full_thinking.push_str(text);
                                    }
                                }
                            }
                        }

                        // Tool call deltas. OpenAI streams the id + function.name
                        // on the first chunk for a given index, then subsequent
                        // chunks carry argument fragments. Some OpenRouter routes
                        // (DeepSeek, Qwen, Kimi, etc.) buffer differently — id may
                        // arrive a chunk later than the name, or the first chunk
                        // may already carry a partial argument. We fire ToolUseStart
                        // the moment we have BOTH id and name, and dedupe via the
                        // `started` HashSet so re-emits don't reset the UI card.
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

                                // Emit ToolUseStart on the first chunk for this idx
                                // where both id and name are now known.
                                if !entry.0.is_empty()
                                    && !entry.1.is_empty()
                                    && tool_starts_emitted.insert(idx)
                                {
                                    if let Some(cb) = &stream_cb {
                                        cb(ProviderStreamEvent::ToolUseStart {
                                            id: entry.0.clone(),
                                            name: entry.1.clone(),
                                        });
                                    }
                                }

                                // Forward argument fragments only after Start has
                                // fired (so the UI has somewhere to put them).
                                if tool_starts_emitted.contains(&idx) {
                                    if let Some(func) = tc.get("function") {
                                        if let Some(args) = func.get("arguments").and_then(|a| a.as_str()) {
                                            if !args.is_empty() {
                                                if let Some(cb) = &stream_cb {
                                                    cb(ProviderStreamEvent::ToolUseInputDelta {
                                                        id: entry.0.clone(),
                                                        partial_json: args.to_string(),
                                                    });
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Build content blocks. Thinking goes first so the order matches the
    // Claude / Responses-API paths (thinking → text → tool_use); downstream
    // renderers stop scanning once they hit a Text block, which would hide a
    // trailing Thinking block on resume from history.
    let mut content = Vec::new();
    if !full_thinking.is_empty() {
        content.push(ContentBlock::Thinking {
            thinking: full_thinking,
            signature: None,
            duration_secs: None,
        });
    }
    if !full_text.is_empty() {
        content.push(ContentBlock::Text { text: full_text });
    }

    let mut sorted_tcs: Vec<_> = tool_calls.into_iter().collect();
    sorted_tcs.sort_by_key(|(idx, _)| *idx);

    // Fire ToolUseStop for every tool that we previously announced via
    // ToolUseStart. Chat-completions has no per-tool stop event — we close
    // them once when the stream is over but before the AiResponse returns,
    // so the UI flips out of "args streaming" before execution begins.
    if let Some(cb) = &stream_cb {
        for (_, (id, _, _)) in &sorted_tcs {
            if !id.is_empty() {
                cb(ProviderStreamEvent::ToolUseStop { id: id.clone() });
            }
        }
    }

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
            cache_read_tokens,
            cache_write_tokens: 0,
        },
        stop_reason,
        actual_cost_usd,
        served_provider,
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
                                    // Fire ToolUseStart immediately so the UI can
                                    // show name + spinner before any argument
                                    // fragments arrive. call_id may be empty if
                                    // the API delivers it later — we skip the
                                    // emit in that rare case (the post-stream
                                    // TaskEvent::ToolUse from the executor still
                                    // covers it).
                                    if !call_id.is_empty() && !name.is_empty() {
                                        if let Some(cb) = &stream_cb {
                                            cb(ProviderStreamEvent::ToolUseStart {
                                                id: call_id.clone(),
                                                name: name.clone(),
                                            });
                                        }
                                    }
                                    func_calls.insert(idx, (call_id, name, String::new()));
                                }
                            }
                        }

                        "response.function_call_arguments.delta" => {
                            let idx = v.get("output_index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                            if let Some(delta) = v.get("delta").and_then(|d| d.as_str()) {
                                if let Some(entry) = func_calls.get_mut(&idx) {
                                    if !delta.is_empty() && !entry.0.is_empty() {
                                        if let Some(cb) = &stream_cb {
                                            cb(ProviderStreamEvent::ToolUseInputDelta {
                                                id: entry.0.clone(),
                                                partial_json: delta.to_string(),
                                            });
                                        }
                                    }
                                    entry.2.push_str(delta);
                                }
                            }
                        }

                        "response.function_call_arguments.done" | "response.output_item.done" => {
                            // Fire ToolUseStop the moment the API tells us this
                            // function_call has finished streaming. .done arrives
                            // for *every* output item so we must filter to
                            // function_call entries; chat-completions-style fall-
                            // back covered separately by the post-loop emit below.
                            let idx = v.get("output_index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                            let is_func_call = match event_type {
                                "response.function_call_arguments.done" => true,
                                "response.output_item.done" => v
                                    .get("item")
                                    .and_then(|i| i.get("type"))
                                    .and_then(|t| t.as_str())
                                    == Some("function_call"),
                                _ => false,
                            };
                            if is_func_call {
                                if let Some(entry) = func_calls.get(&idx) {
                                    if !entry.0.is_empty() {
                                        if let Some(cb) = &stream_cb {
                                            cb(ProviderStreamEvent::ToolUseStop { id: entry.0.clone() });
                                        }
                                    }
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
        actual_cost_usd: None,
        served_provider: None,
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
    // web_search_call item fields — server-side search invocation. The
    // `action` sub-object carries `{ type: "search", query: "..." }` so we
    // can surface the query the model used in the synthetic tool card.
    id: Option<String>,
    #[serde(default)]
    action: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct ResponsesApiContent {
    #[serde(rename = "type")]
    content_type: String,
    text: Option<String>,
    /// Inline citations attached to `output_text` parts. Populated when the
    /// built-in `web_search` tool runs and the model cites pages it browsed.
    /// Each entry typically carries `{ type: "url_citation", url, title,
    /// start_index, end_index }`.
    #[serde(default)]
    annotations: Vec<serde_json::Value>,
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

        // Server-resolved tool pairs (synthesized as inline ToolUse + ToolResult
        // in the SAME assistant message — e.g. OpenAI `web_search_call` with a
        // `ws_*` id) must not be replayed on the chat-completions wire. The
        // call/result is stateful provider-side; emitting it as a `tool_calls`
        // entry would require a matching `role: tool` message that doesn't
        // belong to a regular client-side call. Skip ToolUse blocks whose id
        // has an inline ToolResult in the same message, and skip the inline
        // ToolResult emission. Only matters when a task that started on the
        // Responses API gets serialized through this chat-completions path
        // (e.g. model switch); pure chat-completions sessions have no
        // server-resolved built-in tools to begin with.
        let inline_resolved_ids: std::collections::HashSet<String> =
            if matches!(msg.role, Role::Assistant) {
                let uses: std::collections::HashSet<&str> = msg
                    .content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::ToolUse { id, .. } => Some(id.as_str()),
                        _ => None,
                    })
                    .collect();
                msg.content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::ToolResult { tool_use_id, .. }
                            if uses.contains(tool_use_id.as_str()) =>
                        {
                            Some(tool_use_id.clone())
                        }
                        _ => None,
                    })
                    .collect()
            } else {
                std::collections::HashSet::new()
            };

        // Tool results are sent as separate "tool" role messages in OpenAI
        for block in &msg.content {
            if let ContentBlock::ToolResult { tool_use_id, content, .. } = block {
                if inline_resolved_ids.contains(tool_use_id) {
                    continue;
                }
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
                ContentBlock::ToolUse { id, name, input, .. }
                    if !inline_resolved_ids.contains(id) =>
                {
                    Some(json!({
                        "id": id,
                        "type": "function",
                        "function": {
                            "name": name,
                            "arguments": input.to_string(),
                        }
                    }))
                }
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
                        // F-15: cap data URL size. OpenAI accepts ~20 MB
                        // images; anything larger is either a model error
                        // (attached the wrong thing) or a prompt-injection
                        // attempt to exhaust memory by building a giant
                        // request body. 32 MiB of base64 is ~24 MB of binary.
                        if data.len() > 32 * 1024 * 1024 {
                            continue;
                        }
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
                            // F-15: same 32 MiB cap as the chat-completions
                            // path. Oversized images are silently dropped
                            // rather than balloon the request.
                            ContentBlock::Image { media_type, data }
                                if data.len() <= 32 * 1024 * 1024 =>
                            {
                                Some(json!({
                                    "type": "input_image",
                                    "image_url": format!("data:{};base64,{}", media_type, data),
                                }))
                            }
                            ContentBlock::Image { .. } => None,
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
                // Server-side tools (OpenAI `web_search_call`, etc.) arrive as a
                // ToolUse + ToolResult pair inside the SAME assistant message —
                // we synthesize the pair so the chat UI renders a tool card, but
                // the call is stateful on OpenAI's side and must NOT be replayed
                // as a regular `function_call` (which then requires a matching
                // `function_call_output` item that doesn't exist, producing
                // "No tool output found for function call ws_*"). Skip any
                // ToolUse whose id already has an inline ToolResult in this
                // message — the model's text answer (sent below) carries the
                // search context the next turn needs.
                let inline_resolved: std::collections::HashSet<&str> = msg
                    .content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::ToolResult { tool_use_id, .. } => {
                            Some(tool_use_id.as_str())
                        }
                        _ => None,
                    })
                    .collect();
                // Tool calls become standalone function_call items
                for block in &msg.content {
                    if let ContentBlock::ToolUse { id, name, input, .. } = block {
                        if inline_resolved.contains(id.as_str()) {
                            continue;
                        }
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

    // Server-side web_search bookkeeping. The Responses API emits a
    // `web_search_call` item for each search the model ran, separately from
    // the message it produced — and the citations land as annotations on
    // the message's `output_text` parts. We collect both, then synthesize a
    // single `web_search` ToolUse + ToolResult pair so the chat UI renders
    // the same tool card it shows for Anthropic / Gemini server-side search.
    let mut pending_search_calls: Vec<(String, String)> = Vec::new(); // (call_id, query)
    let mut collected_citations: Vec<serde_json::Value> = Vec::new();

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
            "web_search_call" => {
                // The model just ran a server-side search. Capture id + the
                // query from the action sub-object so we can stamp them onto
                // the synthetic ToolUse below.
                let call_id = item
                    .id
                    .clone()
                    .unwrap_or_else(|| format!("ws_{}", pending_search_calls.len()));
                let query = item
                    .action
                    .as_ref()
                    .and_then(|a| a.get("query"))
                    .and_then(|q| q.as_str())
                    .unwrap_or("")
                    .to_string();
                pending_search_calls.push((call_id, query));
            }
            "message" => {
                if let Some(parts) = item.content {
                    for part in parts {
                        if part.content_type == "output_text" {
                            // Harvest url_citation annotations before we
                            // drop the part. The model interleaves citations
                            // through the prose; we surface all of them
                            // together in the synthetic ToolResult.
                            for ann in &part.annotations {
                                if ann.get("type").and_then(|t| t.as_str()) == Some("url_citation") {
                                    collected_citations.push(ann.clone());
                                }
                            }
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

    // If the server ran web_search, synthesize a `web_search` ToolUse +
    // ToolResult pair so the chat card lights up. We prepend the pair so
    // the search shows up *before* the assistant text that cited it — same
    // visual order Anthropic and Gemini use. When the model ran multiple
    // searches we fold them into one card with all queries listed, because
    // the citations come back unattributed (annotations don't say which
    // search call produced each URL).
    if !pending_search_calls.is_empty() {
        let (call_id, first_query) = pending_search_calls[0].clone();
        let all_queries: Vec<String> = pending_search_calls
            .iter()
            .map(|(_, q)| q.clone())
            .filter(|q| !q.is_empty())
            .collect();

        let mut results_arr: Vec<serde_json::Value> = Vec::new();
        let mut seen_urls = std::collections::HashSet::new();
        for ann in &collected_citations {
            let url = ann.get("url").and_then(|u| u.as_str()).unwrap_or("");
            if url.is_empty() || !seen_urls.insert(url.to_string()) {
                continue;
            }
            let title = ann.get("title").and_then(|t| t.as_str()).unwrap_or(url);
            results_arr.push(json!({
                "url": url,
                "title": title,
            }));
        }

        let input = if all_queries.len() > 1 {
            json!({ "query": first_query, "queries": all_queries })
        } else {
            json!({ "query": first_query })
        };

        let result_payload = json!({
            "queries": all_queries,
            "results": results_arr,
        });

        let synthetic_use = ContentBlock::ToolUse {
            id: call_id.clone(),
            name: "web_search".to_string(),
            input,
            thought_signature: None,
        };
        let synthetic_result = ContentBlock::ToolResult {
            tool_use_id: call_id,
            content: serde_json::to_string(&result_payload).unwrap_or_else(|_| "{}".to_string()),
            is_error: false,
        };

        // Insert before the first Text block so the card precedes the prose.
        let insert_at = content
            .iter()
            .position(|b| matches!(b, ContentBlock::Text { .. }))
            .unwrap_or(content.len());
        content.insert(insert_at, synthetic_result);
        content.insert(insert_at, synthetic_use);
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

    AiResponse { content, usage, stop_reason, actual_cost_usd: None, served_provider: None }
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

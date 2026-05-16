//! Google Gemini provider — SSE streaming via `:streamGenerateContent?alt=sse`.
//!
//! Tool integration:
//! - Regular function tools are sent under `tools[].functionDeclarations[]`.
//! - `web_search` becomes `tools: [{"google_search": {}}]` (server-side grounding).
//!   When the model uses it, we synthesize a paired ToolUse + ToolResult in the
//!   assistant response content so the UI renders it identically to the
//!   Anthropic/OpenAI paths.
//! - `web_fetch` becomes `tools: [{"url_context": {}}]` likewise.
//! - Thinking (2.5+/3.x): thought parts stream as `ThinkingDelta` events.
//!   The `thoughtSignature` on function-call parts is preserved for echo-back.

use super::{
    AiProvider, AiResponse, ContentBlock, Message, ModelInfo, ProviderConfig, ProviderStreamEvent,
    Role, StopReason, StreamCallback, TokenUsage, ToolDef,
};
use anyhow::Result;
use async_trait::async_trait;
use futures::StreamExt;
use serde::Deserialize;
use serde_json::{json, Value};

pub struct GeminiProvider {
    client: reqwest::Client,
}

impl GeminiProvider {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl AiProvider for GeminiProvider {
    async fn chat(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDef>,
        config: &ProviderConfig,
        stream_cb: Option<StreamCallback>,
    ) -> Result<AiResponse> {
        let base = config
            .base_url
            .as_deref()
            .unwrap_or("https://generativelanguage.googleapis.com")
            .trim_end_matches('/');

        // Use the SSE streaming endpoint so thinking/text tokens arrive in real
        // time rather than all at once after the full response completes.
        //
        // F-05: the API key now goes in the `x-goog-api-key` header rather
        // than the URL query string. Query-string secrets routinely leak via
        // reverse-proxy access logs, MITM TLS proxies in corporate networks,
        // crash-reporter URL captures, and Referer headers on follow-on
        // requests. Google's own guidance is to use the header form.
        let url = format!(
            "{}/v1beta/models/{}:streamGenerateContent?alt=sse",
            base, config.model
        );

        // Partition tools: custom function declarations vs Gemini server-side built-ins.
        let mut function_decls: Vec<Value> = Vec::new();
        let mut saw_web_search = false;
        let mut saw_web_fetch = false;
        for t in &tools {
            match t.name.as_str() {
                "web_search" => saw_web_search = true,
                "web_fetch" => saw_web_fetch = true,
                _ => {
                    function_decls.push(json!({
                        "name": t.name,
                        "description": t.description,
                        "parameters": sanitize_schema(&t.parameters),
                    }));
                }
            }
        }
        let want_grounding = saw_web_search || config.web_search_enabled;
        let want_url_context = saw_web_fetch || config.web_fetch_enabled;
        let has_server_tools = want_grounding || want_url_context;

        // Build the tools array.
        // When mixing built-in server-side tools with custom functionDeclarations,
        // Gemini requires toolConfig.includeServerSideToolInvocations = true.
        let mut tool_groups: Vec<Value> = Vec::new();
        if !function_decls.is_empty() {
            tool_groups.push(json!({ "functionDeclarations": function_decls }));
        }
        if want_grounding {
            tool_groups.push(json!({ "google_search": {} }));
        }
        if want_url_context {
            tool_groups.push(json!({ "url_context": {} }));
        }

        let contents = convert_messages(&messages);

        let mut generation_config = json!({
            "maxOutputTokens": config.max_tokens,
        });
        if config.supports_temperature {
            generation_config["temperature"] = json!(config.temperature);
        }
        // Gemini 2.5+ supports thinking. When no budget is configured (0),
        // default to dynamic mode (-1) for models that advertise thinking support
        // so thought tokens are returned. Older models (2.0, 1.5) ignore this
        // field. When the user sets an explicit budget, forward it as-is.
        //
        // The per-model `supports_reasoning_effort` flag suppresses the entire
        // thinkingConfig — useful for Gemini-compatible gateways that 400 on
        // an unknown field, or when the user has registered a model that
        // claims to be Gemini-flavoured but doesn't actually accept it.
        let thinking_budget: i32 = if !config.supports_reasoning_effort {
            0
        } else if config.thinking_budget > 0 {
            config.thinking_budget as i32
        } else if model_supports_thinking(&config.model) {
            -1 // dynamic — model decides how much to think
        } else {
            0
        };
        if thinking_budget != 0 {
            generation_config["thinkingConfig"] = json!({
                "thinkingBudget": thinking_budget,
                // Without this, Gemini computes thoughts internally but does
                // NOT stream them back as `thought: true` parts — the user's
                // "thinking" panel stays blank. Required for the agent's
                // ThinkingDelta path on every Gemini 2.5+ model.
                "includeThoughts": true,
            });
        }

        let mut body = json!({
            "contents": contents,
            "generationConfig": generation_config,
        });
        if let Some(sys) = &config.system_prompt {
            body["systemInstruction"] = json!({
                "parts": [{ "text": sys }]
            });
        }
        if !tool_groups.is_empty() {
            body["tools"] = json!(tool_groups);
        }
        // Mixing built-in server tools with custom function declarations requires
        // this flag — without it Gemini returns a 400 INVALID_ARGUMENT.
        if has_server_tools && !function_decls.is_empty() {
            body["toolConfig"] = json!({
                "includeServerSideToolInvocations": true
            });
        }

        let builder = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("x-goog-api-key", &config.api_key);
        let resp = super::send_json_with_retry(builder, &body, "Gemini").await?;

        // ── SSE stream loop ───────────────────────────────────────────────────
        // Each `data:` line is a complete GenerateContentResponse JSON object
        // containing the deltas for that chunk. We accumulate them into a
        // single AiResponse while emitting TextDelta / ThinkingDelta events
        // in real time so the UI renders tokens as they arrive.

        let mut byte_stream = resp.bytes_stream();
        let mut buffer = String::new();

        let mut thinking_acc = String::new();
        let mut text_acc = String::new();
        // (tool_name, args, thought_signature)
        // (id, name, args, thought_signature) — id is generated up-front so
        // the streaming events and the final ContentBlock share identity.
        let mut fn_calls: Vec<(String, String, Value, Option<String>)> = Vec::new();
        let mut grounding_acc: Option<GroundingMetadata> = None;
        let mut usage = TokenUsage::default();
        let mut thoughts_count: u32 = 0; // tracked separately so we can synthesize a block
        let mut stop_reason = StopReason::EndTurn;

        'stream: while let Some(chunk) = byte_stream.next().await {
            if let Some(ref token) = config.cancel_token {
                if token.load(std::sync::atomic::Ordering::SeqCst) {
                    return Err(anyhow::anyhow!("Task cancelled"));
                }
            }
            buffer.push_str(&String::from_utf8_lossy(&chunk?));

            loop {
                let Some(nl) = buffer.find('\n') else { break };
                let line = buffer[..nl].trim_end_matches('\r').to_string();
                buffer = buffer[nl + 1..].to_string();

                if line.trim() == "data: [DONE]" {
                    break 'stream;
                }
                let data = match line.strip_prefix("data: ") {
                    Some(d) => d,
                    None => continue, // skip event:/id: lines
                };

                let chunk_resp: GeminiResponse = match serde_json::from_str(data) {
                    Ok(r) => r,
                    Err(_) => continue,
                };

                if let Some(u) = chunk_resp.usage {
                    thoughts_count = u.thoughts;
                    usage = TokenUsage {
                        input_tokens: u.prompt.saturating_sub(u.cache_read),
                        output_tokens: u.output.saturating_add(u.thoughts),
                        cache_read_tokens: u.cache_read,
                        cache_write_tokens: 0,
                    };
                }

                let Some(candidate) = chunk_resp.candidates.into_iter().next() else { continue };

                if let Some(fin) = candidate.finish_reason.as_deref() {
                    stop_reason = match fin {
                        "STOP" => StopReason::EndTurn,
                        "MAX_TOKENS" => StopReason::MaxTokens,
                        "TOOL_CALLS" => StopReason::ToolUse,
                        _ => stop_reason,
                    };
                }
                if let Some(g) = candidate.grounding {
                    grounding_acc = Some(g);
                }
                let parts = candidate.content.map(|c| c.parts).unwrap_or_default();
                for part in parts {
                    if part.thought.unwrap_or(false) {
                        if let Some(text) = part.text {
                            if !text.is_empty() {
                                if let Some(cb) = &stream_cb {
                                    cb(ProviderStreamEvent::ThinkingDelta(text.clone()));
                                }
                                thinking_acc.push_str(&text);
                            }
                        }
                    } else if let Some(fc) = part.function_call {
                        // Gemini does not stream argument fragments — the entire
                        // `args` object arrives in one shot. Generate the id
                        // *here* (rather than in the post-loop block-build pass)
                        // so the streaming events and the eventual ContentBlock
                        // share an identity, otherwise the frontend would render
                        // a duplicate card. Uniqueness across many calls in one
                        // response is guaranteed by including the running fn_calls
                        // length plus a UUID suffix to defend against same-name
                        // calls firing in rapid succession.
                        let id = format!(
                            "gem_call_{}_{}_{}",
                            fc.name,
                            fn_calls.len(),
                            uuid::Uuid::new_v4().simple()
                        );
                        if let Some(cb) = &stream_cb {
                            cb(ProviderStreamEvent::ToolUseStart {
                                id: id.clone(),
                                name: fc.name.clone(),
                            });
                            let args_json = serde_json::to_string(&fc.args)
                                .unwrap_or_else(|_| "{}".to_string());
                            cb(ProviderStreamEvent::ToolUseInputDelta {
                                id: id.clone(),
                                partial_json: args_json,
                            });
                            cb(ProviderStreamEvent::ToolUseStop { id: id.clone() });
                        }
                        fn_calls.push((id, fc.name, fc.args, part.thought_signature));
                        stop_reason = StopReason::ToolUse;
                    } else if let Some(text) = part.text {
                        if !text.is_empty() {
                            if let Some(cb) = &stream_cb {
                                cb(ProviderStreamEvent::TextDelta(text.clone()));
                            }
                            text_acc.push_str(&text);
                        }
                    }
                }
            }
        }

        // ── Build final AiResponse from accumulated state ─────────────────────
        let mut content: Vec<ContentBlock> = Vec::new();

        // Gemini 2.5+ and 3.x think internally but do NOT expose thinking text
        // via the API — only thoughtsTokenCount in usage reflects it. When we
        // detect that (thoughts > 0, no streamed thought parts), synthesize a
        // minimal thinking block so the UI shows that thinking happened and
        // emits a ThinkingDelta so the indicator appears correctly.
        if thinking_acc.is_empty() && thoughts_count > 0 {
            let placeholder = format!(
                "(Gemini thought for {} tokens — thinking text is not exposed by this model's API)",
                thoughts_count
            );
            if let Some(cb) = &stream_cb {
                cb(ProviderStreamEvent::ThinkingDelta(placeholder.clone()));
            }
            thinking_acc = placeholder;
        }

        if !thinking_acc.is_empty() {
            content.push(ContentBlock::Thinking {
                thinking: thinking_acc,
                signature: None,
                duration_secs: None,
            });
        }
        if !text_acc.is_empty() {
            content.push(ContentBlock::Text { text: text_acc });
        }
        for (id, name, args, thought_sig) in fn_calls {
            content.push(ContentBlock::ToolUse {
                id,
                name,
                input: args,
                thought_signature: thought_sig,
            });
        }

        // Grounding → synthetic web_search ToolUse + ToolResult pair.
        if let Some(g) = grounding_acc {
            if !g.grounding_chunks.is_empty() || !g.web_search_queries.is_empty() {
                let query = g.web_search_queries.first().cloned().unwrap_or_default();
                let results: Vec<Value> = g
                    .grounding_chunks
                    .iter()
                    .filter_map(|c| c.web.as_ref())
                    .map(|w| json!({ "title": w.title.clone().unwrap_or_default(), "url": w.uri.clone().unwrap_or_default() }))
                    .collect();
                let use_id = format!("gem_search_{}", content.len());
                content.push(ContentBlock::ToolUse {
                    id: use_id.clone(),
                    name: "web_search".to_string(),
                    input: json!({ "query": query }),
                    thought_signature: None,
                });
                let stringified = serde_json::to_string(&json!({
                    "queries": g.web_search_queries,
                    "results": results,
                })).unwrap_or_default();
                content.push(ContentBlock::ToolResult {
                    tool_use_id: use_id,
                    content: stringified,
                    is_error: false,
                });
            }
        }

        Ok(AiResponse { content, usage, stop_reason })
    }

    fn name(&self) -> &str {
        "Gemini"
    }

    fn available_models(&self) -> Vec<ModelInfo> {
        crate::model_registry::models_for_provider("Gemini")
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

// === Gemini API types ===

#[derive(Deserialize)]
struct GeminiResponse {
    #[serde(default)]
    candidates: Vec<GeminiCandidate>,
    #[serde(default, rename = "usageMetadata")]
    usage: Option<GeminiUsage>,
}

#[derive(Deserialize)]
struct GeminiCandidate {
    #[serde(default)]
    content: Option<GeminiContent>,
    #[serde(default, rename = "finishReason")]
    finish_reason: Option<String>,
    #[serde(default, rename = "groundingMetadata")]
    grounding: Option<GroundingMetadata>,
}

#[derive(Deserialize)]
struct GeminiContent {
    #[serde(default)]
    parts: Vec<GeminiPart>,
}

#[derive(Deserialize)]
struct GeminiPart {
    #[serde(default)]
    text: Option<String>,
    #[serde(default, rename = "functionCall")]
    function_call: Option<GeminiFunctionCall>,
    /// Gemini 2.5+ emits structured thinking parts. We surface them via the
    /// existing Thinking block variant so the UI renders them the same way.
    #[serde(default)]
    thought: Option<bool>,
    /// Opaque signature that must be echoed back with the thinking part in
    /// subsequent requests when function calls follow thinking. Required by
    /// Gemini when thinking + tool use are combined.
    #[serde(default, rename = "thoughtSignature")]
    thought_signature: Option<String>,
}

#[derive(Deserialize)]
struct GeminiFunctionCall {
    name: String,
    #[serde(default)]
    args: Value,
}

#[derive(Deserialize)]
struct GroundingMetadata {
    #[serde(default, rename = "webSearchQueries")]
    web_search_queries: Vec<String>,
    #[serde(default, rename = "groundingChunks")]
    grounding_chunks: Vec<GroundingChunk>,
}

#[derive(Deserialize)]
struct GroundingChunk {
    #[serde(default)]
    web: Option<GroundingWeb>,
}

#[derive(Deserialize)]
struct GroundingWeb {
    #[serde(default)]
    uri: Option<String>,
    #[serde(default)]
    title: Option<String>,
}

#[derive(Deserialize)]
struct GeminiUsage {
    #[serde(default, rename = "promptTokenCount")]
    prompt: u32,
    #[serde(default, rename = "candidatesTokenCount")]
    output: u32,
    #[serde(default, rename = "cachedContentTokenCount")]
    cache_read: u32,
    /// Thinking tokens are billed at the output rate but reported separately.
    #[serde(default, rename = "thoughtsTokenCount")]
    thoughts: u32,
}

// === Conversion: Rustic Message[] → Gemini contents[] ===

fn convert_messages(messages: &[Message]) -> Vec<Value> {
    // Pre-build a map from tool_use_id → tool_name so that functionResponse
    // can reference the original function name, not the synthetic call ID.
    // Gemini matches functionResponse.name against the preceding functionCall.name.
    let mut id_to_name: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
    for msg in messages {
        for block in &msg.content {
            if let ContentBlock::ToolUse { id, name, .. } = block {
                id_to_name.insert(id.as_str(), name.as_str());
            }
        }
    }

    let mut contents: Vec<Value> = Vec::new();
    for msg in messages {
        match msg.role {
            Role::System => {} // handled via systemInstruction on the request
            Role::User | Role::Assistant => {
                let role = match msg.role {
                    Role::Assistant => "model",
                    _ => "user",
                };
                let mut parts: Vec<Value> = Vec::new();
                for block in &msg.content {
                    match block {
                        ContentBlock::Text { text } => {
                            if !text.is_empty() {
                                parts.push(json!({ "text": text }));
                            }
                        }
                        ContentBlock::ToolUse { name, input, thought_signature, .. } => {
                            // Assistant → functionCall part.
                            // Echo thought_signature at the Part level if present —
                            // Gemini requires it to reconstruct its internal reasoning.
                            let mut fn_part = json!({
                                "functionCall": { "name": name, "args": input }
                            });
                            if let Some(sig) = thought_signature {
                                fn_part["thoughtSignature"] = json!(sig);
                            }
                            parts.push(fn_part);
                        }
                        ContentBlock::ToolResult { tool_use_id, content, .. } => {
                            // Gemini requires functionResponse.name to match the
                            // original functionCall.name, not the call ID.
                            let fn_name = id_to_name
                                .get(tool_use_id.as_str())
                                .copied()
                                .unwrap_or(tool_use_id.as_str());

                            // functionResponse.response must be a JSON object (Struct).
                            // Tool outputs can be JSON arrays or plain strings, so wrap
                            // anything that isn't already an object.
                            let parsed: Value = serde_json::from_str(content)
                                .unwrap_or(Value::Null);
                            let response = if parsed.is_object() {
                                parsed
                            } else {
                                json!({ "output": if parsed.is_null() { Value::String(content.clone()) } else { parsed } })
                            };

                            parts.push(json!({
                                "functionResponse": {
                                    "name": fn_name,
                                    "response": response,
                                }
                            }));
                        }
                        ContentBlock::Image { media_type, data } => {
                            parts.push(json!({
                                "inlineData": {
                                    "mimeType": media_type,
                                    "data": data,
                                }
                            }));
                        }
                        _ => {}
                    }
                }
                if !parts.is_empty() {
                    contents.push(json!({ "role": role, "parts": parts }));
                }
            }
        }
    }
    contents
}


/// Returns true when the model is known to support extended thinking
/// (Gemini 2.5+ and 3.x families). Older models (2.0, 1.5) silently ignore
/// thinkingConfig but we avoid sending it unnecessarily.
fn model_supports_thinking(model: &str) -> bool {
    // Match 2.5-*, 3-*, 3.*-* families. gemini-2.0-* and gemini-1.5-* don't think.
    let m = model.to_lowercase();
    m.contains("gemini-2.5")
        || m.contains("gemini-3")
}

/// Gemini's schema format (OpenAPI subset) rejects several standard JSON Schema
/// fields. Strip them recursively before sending function declarations.
fn sanitize_schema(schema: &Value) -> Value {
    // Fields unsupported by the Gemini function-declaration schema.
    const STRIP: &[&str] = &[
        "additionalProperties",
        "$schema",
        "$defs",
        "definitions",
        "examples",
        "exclusiveMinimum",
        "exclusiveMaximum",
        "if",
        "then",
        "else",
        "contains",
        "patternProperties",
        "unevaluatedProperties",
        "unevaluatedItems",
        "prefixItems",
        "propertyNames",
        "contentEncoding",
        "contentMediaType",
        "$comment",
    ];

    match schema {
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (k, v) in map {
                if STRIP.contains(&k.as_str()) {
                    continue;
                }
                out.insert(k.clone(), sanitize_schema(v));
            }
            Value::Object(out)
        }
        Value::Array(arr) => Value::Array(arr.iter().map(sanitize_schema).collect()),
        other => other.clone(),
    }
}

#[cfg(test)]
mod sse_snapshot_tests {
    //! Snapshot tests for Gemini's streaming response shapes. Catches drift
    //! before users do.
    use super::*;

    #[test]
    fn text_response() {
        let json = r#"{
            "candidates": [{
                "content": {"parts": [{"text": "Hello"}]},
                "finishReason": "STOP"
            }],
            "usageMetadata": {
                "promptTokenCount": 10,
                "candidatesTokenCount": 20
            }
        }"#;
        let resp: GeminiResponse = serde_json::from_str(json).expect("parses");
        assert_eq!(resp.candidates.len(), 1);
        let part = &resp.candidates[0].content.as_ref().unwrap().parts[0];
        assert_eq!(part.text.as_deref(), Some("Hello"));
        let usage = resp.usage.expect("usage");
        assert_eq!(usage.prompt, 10);
    }

    #[test]
    fn function_call_response() {
        let json = r#"{
            "candidates": [{
                "content": {
                    "parts": [{
                        "functionCall": {
                            "name": "read_file",
                            "args": {"path": "src/main.rs"}
                        }
                    }]
                }
            }]
        }"#;
        let resp: GeminiResponse = serde_json::from_str(json).expect("parses");
        let part = &resp.candidates[0].content.as_ref().unwrap().parts[0];
        let fc = part.function_call.as_ref().expect("function_call");
        assert_eq!(fc.name, "read_file");
        assert_eq!(fc.args["path"], "src/main.rs");
    }

    #[test]
    fn thinking_part() {
        let json = r#"{
            "candidates": [{
                "content": {
                    "parts": [{
                        "text": "Let me think...",
                        "thought": true,
                        "thoughtSignature": "sig_xyz"
                    }]
                }
            }]
        }"#;
        let resp: GeminiResponse = serde_json::from_str(json).expect("parses");
        let part = &resp.candidates[0].content.as_ref().unwrap().parts[0];
        assert_eq!(part.thought, Some(true));
        assert_eq!(part.thought_signature.as_deref(), Some("sig_xyz"));
    }

    #[test]
    fn grounding_metadata() {
        let json = r#"{
            "candidates": [{
                "content": {"parts": [{"text": "answer"}]},
                "groundingMetadata": {
                    "webSearchQueries": ["test query"],
                    "groundingChunks": [{
                        "web": {"uri": "https://example.com", "title": "Example"}
                    }]
                }
            }]
        }"#;
        let resp: GeminiResponse = serde_json::from_str(json).expect("parses");
        let g = resp.candidates[0].grounding.as_ref().expect("grounding");
        assert_eq!(g.web_search_queries, vec!["test query".to_string()]);
        assert_eq!(g.grounding_chunks.len(), 1);
        assert_eq!(
            g.grounding_chunks[0].web.as_ref().unwrap().uri.as_deref(),
            Some("https://example.com")
        );
    }
}


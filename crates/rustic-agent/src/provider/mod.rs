pub mod claude;
pub mod compatible;
pub mod freebuff;
pub mod gemini;
pub mod openai;

pub use freebuff::FreeBuffProvider;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Validate a user-supplied `base_url`. Rejects control chars, non-parseable URLs,
/// and non-HTTPS schemes (except `http://localhost`).
pub fn validate_provider_base_url(url: &str) -> Result<()> {
    if url.is_empty() {
        return Ok(()); // None / empty means "use the provider default"
    }
    if url.chars().any(|c| matches!(c, '\r' | '\n' | '\0')) {
        return Err(anyhow!(
            "base_url contains control characters (newline / CR / NUL); refusing"
        ));
    }
    let parsed =
        reqwest::Url::parse(url).map_err(|e| anyhow!("base_url is not a valid URL: {}", e))?;
    let scheme = parsed.scheme();
    if scheme == "https" {
        return Ok(());
    }
    if scheme == "http" {
        let is_local = matches!(
            parsed.host_str(),
            Some("localhost") | Some("127.0.0.1") | Some("::1")
        );
        if is_local {
            return Ok(());
        }
        return Err(anyhow!(
            "base_url uses http:// for a non-localhost host; refusing — the API key would travel in plaintext"
        ));
    }
    Err(anyhow!(
        "base_url scheme `{}` is not supported (use https:// or http://localhost)",
        scheme
    ))
}

// === Message types ===

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Role {
    #[serde(rename = "user")]
    User,
    #[serde(rename = "assistant")]
    Assistant,
    #[serde(rename = "system")]
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
        /// Opaque thought signature from Gemini — must be echoed back with the
        /// functionCall part in subsequent requests. Always None for other providers.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thought_signature: Option<String>,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
    /// Extended thinking block — shown as collapsible in the UI.
    #[serde(rename = "thinking")]
    Thinking {
        thinking: String,
        /// Opaque signature returned by the API; must be echoed back in subsequent requests.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
        /// Duration in seconds the model spent thinking (stamped after turn completes).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_secs: Option<u64>,
    },
    /// Redacted thinking block — Anthropic safety system stripped the reasoning
    /// text but still produced a signed opaque payload that *must* round-trip
    /// verbatim. If we drop these the API returns 400 "thinking or
    /// redacted_thinking blocks in the latest assistant message cannot be
    /// modified" on the very next turn.
    #[serde(rename = "redacted_thinking")]
    RedactedThinking { data: String },
    /// Base64-encoded image attached by the user.
    #[serde(rename = "image")]
    Image { media_type: String, data: String },
    /// UI-only marker injected when the user switches model mid-chat.
    /// Never serialized to the API — filtered out by the executor before every provider call.
    #[serde(rename = "model_switch")]
    ModelSwitch {
        from_model: String,
        to_model: String,
    },
}

/// Events emitted by a provider as it streams a response.
pub enum ProviderStreamEvent {
    TextDelta(String),
    ThinkingDelta(String),
    /// A client-side tool_use block just opened — name and id are known but
    /// `input` hasn't streamed yet. UI uses this to render the tool card with
    /// a spinner *during* streaming, before the args arrive. Mirrors
    /// Anthropic's `content_block_start` for `tool_use`.
    ToolUseStart {
        id: String,
        name: String,
    },
    /// A fragment of the tool's `input` JSON. Concatenate raw — never parse
    /// per-chunk. Frontend may attempt a tolerant parse for live preview.
    /// Mirrors Anthropic's `input_json_delta`.
    ToolUseInputDelta {
        id: String,
        partial_json: String,
    },
    /// The tool's input is finalized. Execution will follow shortly.
    /// Mirrors Anthropic's `content_block_stop` for tool_use blocks.
    ToolUseStop {
        id: String,
    },
    /// A server-executed tool call (e.g. Anthropic `web_search`) just finished
    /// streaming its input. Emitted at content_block_stop so `input` is
    /// complete. The executor forwards this to the UI as `TaskEvent::ToolUse`.
    ServerToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    /// A server-executed tool's result block just arrived (content is final
    /// at content_block_start, not streamed). The executor forwards this to
    /// the UI as `TaskEvent::ToolResult`.
    ServerToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
    /// Cumulative output-token count reported by the provider mid-stream.
    /// Used purely as a diagnostic by the P0.1 stall watchdog so it can tell
    /// "model produced nothing" (genuine hang) apart from "model produced N
    /// tokens but is just buffering" (Anthropic burst-flush behaviour). Only
    /// Claude emits this today; other providers leave the watchdog's count
    /// at 0, which is fine — the watchdog just won't have the extra signal.
    PartialUsage {
        output_tokens: u32,
    },
}

/// Callback invoked for each streaming token from the provider.
pub type StreamCallback = Arc<dyn Fn(ProviderStreamEvent) + Send + Sync>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value, // JSON Schema
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_read_tokens: u32,
    /// Tokens written to the prompt cache on this request (Anthropic: cache_creation_input_tokens).
    #[serde(default)]
    pub cache_write_tokens: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    /// Anthropic paused a long-running turn (typically mid `web_search` /
    /// `web_fetch` server-tool execution). The assistant message so far ends
    /// with a `server_tool_use` whose result has NOT arrived yet; the caller
    /// must resubmit the conversation unchanged so Anthropic resumes and
    /// streams back the tool result. See the executor's pause-turn handling.
    PauseTurn,
    /// Claude Fable 5 (and other Claude 5 models with safety classifiers)
    /// declined the request. The API returns this as a successful HTTP 200 with
    /// `stop_reason: "refusal"`; the optional String is the classifier category
    /// that fired (e.g. "cyber", "bio", "reasoning_extraction") when reported.
    /// The executor surfaces this to the UI so the user can retry on a fallback
    /// model such as Claude Opus 4.8.
    Refusal(Option<String>),
    Error(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiResponse {
    pub content: Vec<ContentBlock>,
    pub usage: TokenUsage,
    pub stop_reason: StopReason,
    /// Authoritative per-request cost in USD, when the provider reports it
    /// (OpenRouter returns `usage.cost`). None for providers that don't — cost
    /// is then computed from tokens × price. OpenRouter routes one model id to
    /// different-priced upstreams, so this is the only correct figure there.
    #[serde(default)]
    pub actual_cost_usd: Option<f64>,
    /// Upstream provider that actually served the request, when reported
    /// (OpenRouter top-level `provider`). Display-only.
    #[serde(default)]
    pub served_provider: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub max_tokens: u32,
    /// Cost per 1M input tokens (USD). 0.0 if unknown.
    #[serde(default)]
    pub input_cost_per_m: f64,
    /// Cost per 1M output tokens (USD). 0.0 if unknown.
    #[serde(default)]
    pub output_cost_per_m: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub api_key: String,
    pub model: String,
    pub max_tokens: u32,
    pub temperature: f32,
    pub base_url: Option<String>,
    /// System prompt injected into every API call (not stored in messages history).
    pub system_prompt: Option<String>,
    /// Budget for extended thinking tokens. When > 0, the provider enables thinking.
    #[serde(default)]
    pub thinking_budget: u32,
    /// Context window size for this model in tokens. Used for context condensing.
    /// When 0, condensing is disabled.
    #[serde(default)]
    pub context_window: u32,
    /// Whether the user has enabled web_search. Consumed by provider adapters
    /// that support server-side tools (Claude, Gemini) to inject the native
    /// tool declaration. Client-side providers ignore this flag — the tool
    /// executor itself gates registration on the frontend config.
    #[serde(default)]
    pub web_search_enabled: bool,
    /// Same for web_fetch.
    #[serde(default)]
    pub web_fetch_enabled: bool,
    /// True (default) → request includes `temperature`. False → it's omitted
    /// so the model can apply its own default. Pulled from the per-model
    /// capability overrides on `AiConfig`. Some Compatible-provider routers
    /// reject `temperature` on certain models (e.g. Claude Opus 4.7 on some
    /// hosts), so the user can flip this off without losing the model.
    #[serde(default = "default_true")]
    pub supports_temperature: bool,
    /// True → when `thinking_budget > 0`, the adapter sends the
    /// provider-specific reasoning parameter (Claude `thinking`, Gemini
    /// `thinkingConfig`, OpenAI / OpenRouter `reasoning`). False → the
    /// field is suppressed so models that 400 on an unknown reasoning
    /// field don't break. Mirrored from
    /// `ModelCapabilities::supports_reasoning_effort`.
    #[serde(default = "default_true")]
    pub supports_reasoning_effort: bool,
    /// True → for Claude models, use adaptive thinking API (`thinking: {type: "adaptive"}`).
    /// False → use manual budget_tokens (`thinking: {type: "enabled", budget_tokens: N}`).
    /// Defaults to false for backward compatibility. Set to true for Claude 4.6+ models.
    #[serde(default)]
    pub supports_adaptive_thinking: bool,
    /// Cancellation token — checked during streaming to abort early.
    #[serde(skip)]
    pub cancel_token: Option<Arc<std::sync::atomic::AtomicBool>>,
    /// Custom pricing overrides from user configuration. When set, these
    /// override the static model registry for cost calculation.
    #[serde(default)]
    pub custom_input_cost: Option<f64>,
    #[serde(default)]
    pub custom_output_cost: Option<f64>,
    #[serde(default)]
    pub custom_cache_read_cost: Option<f64>,
    #[serde(default)]
    pub custom_cache_write_cost: Option<f64>,
    /// OpenRouter provider allow-list: lowercase provider slugs the user ticked
    /// for this model. When `Some` and non-empty, the OpenRouter request is
    /// restricted to these providers with `allow_fallbacks=false`. `None` or
    /// empty → no restriction (OpenRouter's default routing across all).
    #[serde(default)]
    pub allowed_providers: Option<Vec<String>>,
}

fn default_true() -> bool {
    true
}

// === Transient-failure retry ===

/// Up to 3 attempts with 0.5s→1s backoff. Retries only transient errors
/// (408/429/5xx, connect/timeout); surfaces deterministic failures immediately.
pub async fn send_with_retry(
    builder: reqwest::RequestBuilder,
    provider_name: &str,
) -> Result<reqwest::Response> {
    const MAX_ATTEMPTS: u32 = 3;
    const INITIAL_BACKOFF_MS: u64 = 500;

    let mut last_err: Option<anyhow::Error> = None;
    for attempt in 0..MAX_ATTEMPTS {
        if attempt > 0 {
            let backoff_ms = INITIAL_BACKOFF_MS << (attempt - 1);
            tracing::info!(
                target: "rustic::stream",
                provider = provider_name,
                attempt = attempt + 1,
                max_attempts = MAX_ATTEMPTS,
                backoff_ms,
                "[stream] retrying after backoff"
            );
            tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;
        }
        let req = builder.try_clone().ok_or_else(|| {
            anyhow::anyhow!(
                "{}: request body is not cloneable; cannot retry",
                provider_name
            )
        })?;
        let send_start = std::time::Instant::now();
        tracing::info!(
            target: "rustic::stream",
            provider = provider_name,
            attempt = attempt + 1,
            max_attempts = MAX_ATTEMPTS,
            "[stream] sending request"
        );
        match req.send().await {
            Ok(resp) => {
                let status = resp.status();
                tracing::info!(
                    target: "rustic::stream",
                    provider = provider_name,
                    attempt = attempt + 1,
                    status = %status,
                    send_ms = send_start.elapsed().as_millis() as u64,
                    "[stream] response headers received"
                );
                if status.is_success() {
                    return Ok(resp);
                }
                let transient = matches!(status.as_u16(), 408 | 429 | 500 | 502 | 503 | 504 | 529);
                let text = resp.text().await.unwrap_or_default();
                if !transient || attempt + 1 == MAX_ATTEMPTS {
                    return Err(anyhow::anyhow!(
                        "{} API error {}: {}",
                        provider_name,
                        status,
                        text
                    ));
                }
                last_err = Some(anyhow::anyhow!(
                    "{} API error {} (attempt {}/{}): {}",
                    provider_name,
                    status,
                    attempt + 1,
                    MAX_ATTEMPTS,
                    text
                ));
            }
            Err(e) => {
                let transient = e.is_timeout() || e.is_connect();
                tracing::warn!(
                    target: "rustic::stream",
                    provider = provider_name,
                    attempt = attempt + 1,
                    send_ms = send_start.elapsed().as_millis() as u64,
                    is_timeout = e.is_timeout(),
                    is_connect = e.is_connect(),
                    is_request = e.is_request(),
                    is_body = e.is_body(),
                    transient,
                    error = %e,
                    "[stream] transport error"
                );
                if !transient || attempt + 1 == MAX_ATTEMPTS {
                    return Err(anyhow::anyhow!("{} request failed: {}", provider_name, e));
                }
                last_err = Some(anyhow::anyhow!(
                    "{} request failed (attempt {}/{}): {}",
                    provider_name,
                    attempt + 1,
                    MAX_ATTEMPTS,
                    e
                ));
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("{}: all retry attempts failed", provider_name)))
}

/// Like [`send_with_retry`] but takes the JSON body separately to log it on 4xx errors.
pub async fn send_json_with_retry<T: serde::Serialize + ?Sized>(
    builder: reqwest::RequestBuilder,
    body: &T,
    provider_name: &str,
) -> Result<reqwest::Response> {
    let body_value = serde_json::to_value(body).unwrap_or(serde_json::Value::Null);
    log_outgoing_request(provider_name, &body_value);
    let req = builder.json(body);
    let result = send_with_retry(req, provider_name).await;
    if let Err(ref err) = result {
        log_provider_error(provider_name, &body_value, err);
    }
    result
}

/// One-line debug summary of an outbound provider request — model,
/// message count, role + content-block types of each message. Cheap
/// enough to emit on every call at `debug` level.
fn log_outgoing_request(provider_name: &str, body: &serde_json::Value) {
    let model = body
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("<unset>");
    let messages = body.get("messages").and_then(|v| v.as_array());
    let msg_count = messages.map(|m| m.len()).unwrap_or(0);

    let tool_count = body
        .get("tools")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    let thinking = body
        .get("thinking")
        .cloned()
        .unwrap_or(serde_json::Value::Null);

    tracing::debug!(
        provider = provider_name,
        model = model,
        messages = msg_count,
        tools = tool_count,
        thinking = %thinking,
        "[provider] outgoing request"
    );

    if let Some(messages) = messages {
        let summary = summarize_messages(messages);
        tracing::debug!(
            provider = provider_name,
            "[provider] message shape:\n{}",
            summary
        );
    }
}

/// Render a one-line-per-message shape summary — `[i] role: type1,type2,...`.
/// Used in both the always-on debug log and the 4xx error dump so the
/// path to `messages.<n>.content.<m>` in any provider error is greppable.
fn summarize_messages(messages: &[serde_json::Value]) -> String {
    let mut out = String::new();
    for (i, msg) in messages.iter().enumerate() {
        let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("?");
        let content = msg.get("content");
        let blocks_desc = match content {
            Some(serde_json::Value::String(s)) => {
                format!("text({}ch)", s.chars().count())
            }
            Some(serde_json::Value::Array(arr)) => {
                let parts: Vec<String> = arr
                    .iter()
                    .enumerate()
                    .map(|(j, b)| {
                        let ty = b.get("type").and_then(|v| v.as_str()).unwrap_or("?");
                        let extra = match ty {
                            "text" => b
                                .get("text")
                                .and_then(|v| v.as_str())
                                .map(|s| format!("({}ch)", s.chars().count()))
                                .unwrap_or_default(),
                            "thinking" => {
                                let len = b
                                    .get("thinking")
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.chars().count())
                                    .unwrap_or(0);
                                let sig = b
                                    .get("signature")
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.chars().count())
                                    .unwrap_or(0);
                                format!("({}ch, sig={}ch)", len, sig)
                            }
                            "redacted_thinking" => {
                                let dlen = b
                                    .get("data")
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.chars().count())
                                    .unwrap_or(0);
                                format!("(data={}ch)", dlen)
                            }
                            "tool_use" | "server_tool_use" => {
                                let id = b.get("id").and_then(|v| v.as_str()).unwrap_or("?");
                                let name = b.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                                format!("({} id={})", name, id)
                            }
                            "tool_result" | "web_search_tool_result" | "web_fetch_tool_result" => {
                                let id =
                                    b.get("tool_use_id").and_then(|v| v.as_str()).unwrap_or("?");
                                format!("(id={})", id)
                            }
                            _ => String::new(),
                        };
                        format!("[{}]{}{}", j, ty, extra)
                    })
                    .collect();
                parts.join(", ")
            }
            _ => "<no content>".to_string(),
        };
        out.push_str(&format!("  [{:>2}] {}: {}\n", i, role, blocks_desc));
    }
    out
}

/// True for deterministic 4xx errors; used by the executor to skip retries.
pub fn is_provider_client_error(err: &anyhow::Error) -> bool {
    let err_str = err.to_string();
    err_str.contains("API error 400")
        || err_str.contains("API error 401")
        || err_str.contains("API error 403")
        || err_str.contains("API error 404")
        || err_str.contains("API error 413")
        || err_str.contains("API error 415")
        || err_str.contains("API error 422")
        || err_str.contains("invalid_request_error")
        || err_str.contains("invalid request error")
        || err_str.contains("Bad Request")
}

/// F-06: walk `body` and replace string leaves under message-content /
/// tool-result / system fields with a `[redacted, N chars]` placeholder.
/// Keeps types, lengths, and block ordering so the dump still diagnoses
/// shape problems (which is the whole point of logging it on 4xx) without
/// persisting the user's prose / pasted secrets / file contents to disk for
/// the 7-day log retention window.
fn redact_provider_body(body: &serde_json::Value) -> serde_json::Value {
    fn redact_string(s: &str) -> serde_json::Value {
        serde_json::Value::String(format!("[redacted, {} chars]", s.len()))
    }
    fn walk(v: &serde_json::Value, in_sensitive: bool) -> serde_json::Value {
        match v {
            serde_json::Value::String(s) if in_sensitive => redact_string(s),
            serde_json::Value::Array(items) => {
                serde_json::Value::Array(items.iter().map(|i| walk(i, in_sensitive)).collect())
            }
            serde_json::Value::Object(map) => {
                let mut out = serde_json::Map::with_capacity(map.len());
                for (k, val) in map {
                    let child_sensitive = in_sensitive
                        || matches!(
                            k.as_str(),
                            "text" | "content" | "input" | "output" | "system"
                        );
                    out.insert(k.clone(), walk(val, child_sensitive));
                }
                serde_json::Value::Object(out)
            }
            _ => v.clone(),
        }
    }
    walk(body, false)
}

fn log_provider_error(provider_name: &str, body: &serde_json::Value, err: &anyhow::Error) {
    if !is_provider_client_error(err) {
        return;
    }
    let err_str = err.to_string();

    // F-06: redact user prose / file contents before serialising.
    let redacted = redact_provider_body(body);
    let body_pretty =
        serde_json::to_string_pretty(&redacted).unwrap_or_else(|_| "<unserializable>".to_string());
    let messages = body
        .get("messages")
        .and_then(|v| v.as_array())
        .map(|a| a.as_slice())
        .unwrap_or(&[]);
    let summary = summarize_messages(messages);
    let model = body
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("<unset>");

    tracing::error!(
        provider = provider_name,
        model = model,
        messages = messages.len(),
        body_chars = body_pretty.len(),
        error = %err_str,
        "[provider] 4xx / invalid_request_error — dumping full request for diagnosis"
    );
    tracing::error!(
        provider = provider_name,
        "[provider] === MESSAGE SHAPE ===\n{}=== END MESSAGE SHAPE ===",
        summary
    );
    tracing::error!(
        provider = provider_name,
        "[provider] === REQUEST BODY ({} chars) ===\n{}\n=== END REQUEST BODY ===",
        body_pretty.len(),
        body_pretty
    );
    tracing::error!(
        provider = provider_name,
        "[provider] === SERVER RESPONSE ===\n{}\n=== END SERVER RESPONSE ===",
        err_str
    );
}

// === Provider trait ===

#[async_trait]
pub trait AiProvider: Send + Sync {
    /// Send a chat request. If `stream_cb` is Some, the provider should call it
    /// with each text/thinking delta as the response streams in.
    async fn chat(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDef>,
        config: &ProviderConfig,
        stream_cb: Option<StreamCallback>,
    ) -> Result<AiResponse>;

    fn name(&self) -> &str;
    fn available_models(&self) -> Vec<ModelInfo>;
}

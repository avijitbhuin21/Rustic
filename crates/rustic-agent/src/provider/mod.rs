pub mod claude;
pub mod compatible;
pub mod gemini;
pub mod openai;

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

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
    RedactedThinking {
        data: String,
    },
    /// Base64-encoded image attached by the user.
    #[serde(rename = "image")]
    Image {
        media_type: String,
        data: String,
    },
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
    Error(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiResponse {
    pub content: Vec<ContentBlock>,
    pub usage: TokenUsage,
    pub stop_reason: StopReason,
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
    /// Cancellation token — checked during streaming to abort early.
    #[serde(skip)]
    pub cancel_token: Option<Arc<std::sync::atomic::AtomicBool>>,
}

fn default_true() -> bool { true }

// === Transient-failure retry ===

/// Send a request with up to 3 attempts, backing off 0.5s → 1s between tries.
///
/// Retries only on transient classes — HTTP 408/429/500/502/503/504/529 and
/// reqwest connect/timeout errors. Anything else (4xx malformed-request, auth,
/// TLS misconfig, etc.) is a deterministic failure where retrying just delays
/// the real error reaching the user, so we surface it immediately.
///
/// The returned `Response` is the live HTTP response on the first successful
/// attempt — the caller still drives streaming/SSE parsing on it. We do NOT
/// retry once the response body has started being consumed; mid-stream
/// disconnects need a different strategy (and would risk duplicating output).
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
                let transient =
                    matches!(status.as_u16(), 408 | 429 | 500 | 502 | 503 | 504 | 529);
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
    Err(last_err.unwrap_or_else(|| {
        anyhow::anyhow!("{}: all retry attempts failed", provider_name)
    }))
}

/// Same retry behavior as [`send_with_retry`] but takes the JSON body
/// separately so that on a 4xx / `invalid_request_error` we can dump
/// the full request body plus a structural summary into the logs.
///
/// Use this for every provider call so that we never lose visibility
/// into the wire payload that triggered a provider-side validation
/// failure (e.g. Anthropic's "thinking blocks must remain as they were
/// in the original response", OpenAI's "No tool output found for
/// function call", Gemini's "function response without function call",
/// etc.).
pub async fn send_json_with_retry<T: serde::Serialize + ?Sized>(
    builder: reqwest::RequestBuilder,
    body: &T,
    provider_name: &str,
) -> Result<reqwest::Response> {
    let body_value = serde_json::to_value(body)
        .unwrap_or(serde_json::Value::Null);
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
    let thinking = body.get("thinking").cloned().unwrap_or(serde_json::Value::Null);

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
        tracing::debug!(provider = provider_name, "[provider] message shape:\n{}", summary);
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
                        let ty = b
                            .get("type")
                            .and_then(|v| v.as_str())
                            .unwrap_or("?");
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
                                let id = b
                                    .get("id")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("?");
                                let name = b
                                    .get("name")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("?");
                                format!("({} id={})", name, id)
                            }
                            "tool_result" | "web_search_tool_result"
                            | "web_fetch_tool_result" => {
                                let id = b
                                    .get("tool_use_id")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("?");
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

/// Emit a full diagnostic dump when the provider returned a deterministic
/// client-side error (4xx / `invalid_request_error` / `Bad Request`).
///
/// The dump includes:
///   * the full request JSON body (pretty-printed)
///   * a per-message structural summary
///   * the error string itself (already includes the server's response body)
///
/// All emitted at `error` level — easy to grep, and only when something
/// is actually wrong.
/// True when the provider returned a deterministic 4xx-class error
/// (auth, malformed request, model-not-found, etc). Used by the
/// retry loop in [`crate::task::executor`] to skip wasted retries on
/// errors that aren't transient — re-issuing the same bad request 3
/// more times burns 90s before surfacing the real error to the user.
///
/// Heuristic substring match on the error string; the existing 5xx /
/// transient retry already runs inside `client::send_with_retry`, so
/// what bubbles up here is either a 4xx or a post-stream-start
/// failure (mid-stream disconnect, parse error). Only the 4xx case
/// matches.
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

fn log_provider_error(
    provider_name: &str,
    body: &serde_json::Value,
    err: &anyhow::Error,
) {
    if !is_provider_client_error(err) {
        return;
    }
    let err_str = err.to_string();

    let body_pretty = serde_json::to_string_pretty(body)
        .unwrap_or_else(|_| "<unserializable>".to_string());
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

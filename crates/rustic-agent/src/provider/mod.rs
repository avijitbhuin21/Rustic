pub mod claude;
pub mod compatible;
pub mod freebuff;
pub mod gemini;
pub mod gemini_cache;
pub mod openai;
pub mod request_overrides;

pub use request_overrides::{
    apply_request_overrides, suggested_replacement_param, unsupported_param_from_error, BodyDialect,
};

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
    /// Image attached by the user or returned by a tool.
    ///
    /// `data` is base64. It is empty for blocks loaded from history whose
    /// payload lives in the media store — `path` then names the file and
    /// `media_store::hydrate` refills `data` before any provider call.
    #[serde(rename = "image")]
    Image {
        media_type: String,
        #[serde(default)]
        data: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
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
    /// Per-model request-body overrides (send / omit / value per parameter),
    /// applied by every adapter just before the HTTP send.
    #[serde(default)]
    pub request_overrides: crate::config::RequestParamOverrides,
}

fn default_true() -> bool {
    true
}

// === Transient-failure retry ===

/// Byte-level SSE line splitter: decodes only complete `\n`-terminated lines so
/// a multi-byte UTF-8 character split across two network chunks is never
/// mangled by a per-chunk lossy decode, and exposes the trailing partial line
/// at EOF so a gateway that closes the stream without a final newline doesn't
/// lose the last delta (observed as tool arguments truncated mid-JSON).
#[derive(Default)]
pub struct SseLineBuffer {
    /// Bytes not yet returned as a complete line. Always starts at a line
    /// boundary: complete lines are cut off once per `push`, never per line.
    buf: Vec<u8>,
}

fn decode_line(bytes: &[u8]) -> String {
    let bytes = bytes.strip_suffix(b"\r").unwrap_or(bytes);
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_owned(),
        Err(_) => String::from_utf8_lossy(bytes).into_owned(),
    }
}

impl SseLineBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends `chunk` and returns every complete line (CR stripped) now available.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        // Only the new bytes can contain a newline the previous push didn't see.
        let scan_from = self.buf.len();
        self.buf.extend_from_slice(chunk);
        let mut lines = Vec::new();
        let mut start = 0usize;
        let mut cursor = scan_from;
        while let Some(off) = self.buf[cursor..].iter().position(|b| *b == b'\n') {
            let end = cursor + off;
            lines.push(decode_line(&self.buf[start..end]));
            start = end + 1;
            cursor = start;
        }
        if start > 0 {
            self.buf.drain(..start);
        }
        lines
    }

    /// Returns the unterminated remainder (if any) as a final line.
    pub fn flush(&mut self) -> Option<String> {
        if self.buf.is_empty() {
            return None;
        }
        let rest = std::mem::take(&mut self.buf);
        let s = decode_line(&rest);
        (!s.trim().is_empty()).then_some(s)
    }
}

/// One classified SSE line, shared by every streaming adapter.
#[derive(Debug, PartialEq)]
pub enum SseLine<'a> {
    /// Not a `data:` line (event:/id:/comments/blank) — ignore.
    Skip,
    /// The OpenAI-style `data: [DONE]` terminator.
    Done,
    /// The payload after `data:` (one optional leading space stripped).
    Data(&'a str),
}

/// Classify a line from [`SseLineBuffer`]. Accepts both `data: x` and the
/// spec-legal `data:x` form.
pub fn classify_sse_line(line: &str) -> SseLine<'_> {
    let Some(rest) = line.strip_prefix("data:") else {
        return SseLine::Skip;
    };
    let data = rest.strip_prefix(' ').unwrap_or(rest);
    if data.trim_end() == "[DONE]" {
        SseLine::Done
    } else if data.trim().is_empty() {
        SseLine::Skip
    } else {
        SseLine::Data(data)
    }
}

/// Parsed form of a `data:` JSON line.
pub enum SseJson<T> {
    Skip,
    Done,
    Value(T),
    /// The data line was not valid JSON for `T`; already logged at debug.
    Invalid,
}

/// Classify `line` and deserialize its `data:` payload as `T`. Unparseable
/// payloads are logged at debug level with `tag` and reported as `Invalid`
/// so callers can count them.
pub fn parse_sse_json<T: serde::de::DeserializeOwned>(line: &str, tag: &str) -> SseJson<T> {
    match classify_sse_line(line) {
        SseLine::Skip => SseJson::Skip,
        SseLine::Done => SseJson::Done,
        SseLine::Data(data) => match serde_json::from_str(data) {
            Ok(v) => SseJson::Value(v),
            Err(e) => {
                let mut cut = data.len().min(200);
                while !data.is_char_boundary(cut) {
                    cut -= 1;
                }
                tracing::debug!(
                    error = %e,
                    preview = %&data[..cut],
                    "[{}] dropped unparseable SSE data line",
                    tag
                );
                SseJson::Invalid
            }
        },
    }
}

/// Tool definitions in deterministic name order. Tools sit at the very start
/// of every provider's cacheable prefix, so a reordering (e.g. an MCP server
/// listing its tools in a different order after a reconnect) would otherwise
/// invalidate the whole prompt cache.
pub fn sorted_tools(tools: &[ToolDef]) -> Vec<&ToolDef> {
    let mut v: Vec<&ToolDef> = tools.iter().collect();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v
}

/// Stable per-conversation prompt-cache routing key (OpenAI `prompt_cache_key`,
/// OpenRouter sticky routing). Derived from the model, system prompt and the
/// first message so it stays constant across every iteration of a task
/// without needing a task id threaded through `ProviderConfig`, and differs
/// between conversations that share the same system prompt.
pub fn conversation_cache_key(config: &ProviderConfig, messages: &[Message]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(config.model.as_bytes());
    h.update([0]);
    h.update(config.system_prompt.as_deref().unwrap_or("").as_bytes());
    h.update([0]);
    if let Some(first) = messages.first() {
        for block in &first.content {
            if let ContentBlock::Text { text } = block {
                h.update(text.as_bytes());
                h.update([0]);
            }
        }
    }
    let digest = h.finalize();
    format!("rustic-{}", hex::encode(&digest[..12]))
}

/// True when `base_url` targets OpenRouter.
pub fn is_openrouter(config: &ProviderConfig) -> bool {
    config
        .base_url
        .as_deref()
        .map(|u| u.contains("openrouter.ai"))
        .unwrap_or(false)
}

/// One info line per completed request so cache-hit ratios can be measured
/// from the logs (`grep "\[usage\]"`).
pub fn log_request_usage(provider: &str, model: &str, usage: &TokenUsage) {
    let prompt_total = usage.input_tokens as u64
        + usage.cache_read_tokens as u64
        + usage.cache_write_tokens as u64;
    let hit_pct = if prompt_total > 0 {
        (usage.cache_read_tokens as u64 * 100) / prompt_total
    } else {
        0
    };
    tracing::info!(
        target: "rustic::usage",
        provider,
        model,
        input = usage.input_tokens,
        cache_read = usage.cache_read_tokens,
        cache_write = usage.cache_write_tokens,
        output = usage.output_tokens,
        cache_hit_pct = hit_pct,
        "[usage] request completed"
    );
}

/// Builds the reqwest client used for provider calls: bounded TCP connect (dead hosts fail in 20s instead of the OS default ~3 min), no overall timeout because SSE streams are long-lived, and a per-read idle timeout so a silently stalled stream errors (and retries) instead of hanging.
pub fn provider_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(20))
        .read_timeout(std::time::Duration::from_secs(300))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// Up to 4 attempts with 1s→2s→4s backoff (longer when the server sends
/// `Retry-After`). Retries only transient errors (408/429/5xx, connect/timeout);
/// surfaces deterministic failures immediately.
pub async fn send_with_retry(
    builder: reqwest::RequestBuilder,
    provider_name: &str,
) -> Result<reqwest::Response> {
    const MAX_ATTEMPTS: u32 = 4;
    const INITIAL_BACKOFF_MS: u64 = 1_000;
    const MAX_RETRY_AFTER_MS: u64 = 30_000;

    let mut last_err: Option<anyhow::Error> = None;
    let mut server_retry_after_ms: Option<u64> = None;
    for attempt in 0..MAX_ATTEMPTS {
        if attempt > 0 {
            let mut backoff_ms = INITIAL_BACKOFF_MS << (attempt - 1);
            if let Some(ra) = server_retry_after_ms.take() {
                backoff_ms = backoff_ms.max(ra).min(MAX_RETRY_AFTER_MS);
            }
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
                server_retry_after_ms = resp
                    .headers()
                    .get(reqwest::header::RETRY_AFTER)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.trim().parse::<f64>().ok())
                    .map(|secs| (secs * 1000.0) as u64);
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

/// Like [`send_with_retry`] but takes the JSON body separately: applies the
/// per-model request overrides for `dialect`, then logs the body on 4xx errors.
pub async fn send_json_with_retry(
    builder: reqwest::RequestBuilder,
    body: &mut serde_json::Value,
    provider_name: &str,
    config: &ProviderConfig,
    dialect: BodyDialect,
) -> Result<reqwest::Response> {
    apply_request_overrides(body, &config.request_overrides, dialect);
    log_outgoing_request(provider_name, body);
    let req = builder.json(&*body);
    let result = send_with_retry(req, provider_name).await;
    if let Err(ref err) = result {
        log_provider_error(provider_name, body, err);
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
        // Gemini surfaces deterministic 4xx rejections as INVALID_ARGUMENT in
        // the response body — sometimes re-wrapped by upstream proxies without
        // the "API error 400" prefix.
        || err_str.contains("INVALID_ARGUMENT")
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
        "[provider] 4xx / invalid_request_error"
    );
    tracing::warn!(
        provider = provider_name,
        "[provider] === MESSAGE SHAPE ===\n{}=== END MESSAGE SHAPE ===",
        summary
    );
    if std::env::var_os("RUSTIC_DEBUG_PROVIDER_BODY").is_some() {
        tracing::warn!(
            provider = provider_name,
            "[provider] === REQUEST BODY ({} chars) ===\n{}\n=== END REQUEST BODY ===",
            body_pretty.len(),
            body_pretty
        );
    }
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
        messages: &[Message],
        tools: &[ToolDef],
        config: &ProviderConfig,
        stream_cb: Option<StreamCallback>,
    ) -> Result<AiResponse>;

    fn name(&self) -> &str;
    fn available_models(&self) -> Vec<ModelInfo>;
}

#[cfg(test)]
mod helper_tests {
    use super::*;

    fn cfg() -> ProviderConfig {
        serde_json::from_value(serde_json::json!({
            "api_key": "k", "model": "m", "max_tokens": 10, "temperature": 0.7,
            "base_url": null, "system_prompt": "sys"
        }))
        .unwrap()
    }

    #[test]
    fn sse_buffer_splits_lines_across_chunks_and_flushes_tail() {
        let mut b = SseLineBuffer::new();
        assert!(b.push(b"data: a").is_empty());
        assert_eq!(b.push(b"bc\r\ndata: x\n\ndata: tai"), vec!["data: abc", "data: x", ""]);
        assert_eq!(b.push(b"l"), Vec::<String>::new());
        assert_eq!(b.flush().as_deref(), Some("data: tail"));
        assert_eq!(b.flush(), None);
    }

    #[test]
    fn sse_buffer_keeps_multibyte_char_split_across_chunks() {
        let s = "data: héllo\n".as_bytes();
        let (a, c) = s.split_at(8); // splits inside 'é'
        let mut b = SseLineBuffer::new();
        assert!(b.push(a).is_empty());
        assert_eq!(b.push(c), vec!["data: héllo"]);
        let mut bad = SseLineBuffer::new();
        assert_eq!(bad.push(b"data: \xff\n"), vec!["data: \u{fffd}"]);
    }

    #[test]
    fn classify_sse_lines() {
        assert_eq!(classify_sse_line("event: ping"), SseLine::Skip);
        assert_eq!(classify_sse_line(""), SseLine::Skip);
        assert_eq!(classify_sse_line("data: [DONE]"), SseLine::Done);
        assert_eq!(classify_sse_line("data:[DONE] "), SseLine::Done);
        assert_eq!(classify_sse_line("data: {\"a\":1}"), SseLine::Data("{\"a\":1}"));
        assert_eq!(classify_sse_line("data:{}"), SseLine::Data("{}"));
        assert!(matches!(parse_sse_json::<serde_json::Value>("data: {nope", "t"), SseJson::Invalid));
        assert!(matches!(parse_sse_json::<serde_json::Value>("data: {\"a\":1}", "t"), SseJson::Value(v) if v["a"] == 1));
    }

    #[test]
    fn tools_sorted_by_name() {
        let t = |n: &str| ToolDef { name: n.into(), description: String::new(), parameters: serde_json::json!({}) };
        let tools = vec![t("read_file"), t("mcp__z"), t("edit_file")];
        let names: Vec<&str> = sorted_tools(&tools).iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["edit_file", "mcp__z", "read_file"]);
    }

    #[test]
    fn conversation_cache_key_stable_per_conversation() {
        let c = cfg();
        let m = |s: &str| Message { role: Role::User, content: vec![ContentBlock::Text { text: s.into() }] };
        let a = vec![m("task one")];
        let mut a_later = a.clone();
        a_later.push(m("follow-up"));
        let k = conversation_cache_key(&c, &a);
        assert!(k.starts_with("rustic-") && k.len() == 7 + 24);
        assert_eq!(k, conversation_cache_key(&c, &a_later));
        assert_ne!(k, conversation_cache_key(&c, &[m("task two")]));
    }
}
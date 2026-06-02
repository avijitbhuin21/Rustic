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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

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

/// Models that accept `thinking: {type: "adaptive"}`. Opus 4.7 *only* accepts
/// adaptive; Opus 4.6 / Sonnet 4.6 accept both but adaptive is the recommended
/// path and the manual form is deprecated there. Everything older (4.5 and
/// below) has to stay on the manual path.
///
/// Match is done against the model id — covers bare aliases like `claude-opus-4-7`
/// and dated snapshots like `claude-opus-4-7-20260501`.
fn supports_adaptive_thinking(model: &str) -> bool {
    // Opus 4.7+ and Mythos ONLY support adaptive thinking
    // Opus/Sonnet 4.6 support both but adaptive is recommended
    // Older models only support manual budget_tokens
    model.contains("opus-4-7")
        || model.contains("opus-4-8") 
        || model.contains("opus-4-9")
        || model.contains("opus-4-6")
        || model.contains("sonnet-4-6")
        || model.contains("mythos")
}

/// Map the user-facing `thinking_budget` token count onto an adaptive-mode
/// effort level. The breakpoints roughly mirror the common manual budgets
/// users pick today (2k/8k/20k/40k) so behavior stays recognizable across the
/// switch. Only called when `thinking_budget > 0`.
fn budget_to_effort(budget: u32) -> &'static str {
    match budget {
        0..=4_000 => "low",
        4_001..=16_000 => "medium",
        16_001..=32_000 => "high",
        _ => "max",
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

        // When thinking is enabled the API requires max_tokens > budget_tokens.
        // Clamp upward so a registry miss (unknown short-form model ID) can never
        // cause a 400 error; the model itself enforces the real upper limit.
        let effective_max_tokens = if config.thinking_budget > 0 {
            config.max_tokens.max(config.thinking_budget + 1024)
        } else {
            config.max_tokens
        };

        // Apply prompt caching: mark the last user-turn content block with
        // cache_control so the full prefix (system + tools + all prior turns)
        // is cached. Anthropic's cache has a 5-minute TTL — turn-by-turn work
        // inside the TTL window reads from cache at 10% of the input cost.
        let api_messages = apply_message_cache_breakpoint(api_messages);

        let mut body = json!({
            "model": config.model,
            "max_tokens": effective_max_tokens,
            "stream": true,
            "messages": api_messages,
        });

        if let Some(sys) = system_msg {
            // System prompt as a cached text block — same 5-minute TTL.
            body["system"] = json!([
                {
                    "type": "text",
                    "text": sys,
                    "cache_control": { "type": "ephemeral" }
                }
            ]);
        }

        // Extended thinking: when enabled, temperature must not be set.
        //
        // Opus 4.7+ only accept `type: "adaptive"` and reject the legacy
        // `{type: "enabled", budget_tokens}` form with a 400 error
        // ("thinking.type.enabled is not supported for this model"). Opus 4.6
        // and Sonnet 4.6 accept both but adaptive is Anthropic's recommended
        // path — the manual form is deprecated there. Everything older (4.5
        // and below) only understands the manual form.
        //
        // `config.supports_adaptive_thinking` is a user-set per-model override
        // (the "Register Model" checkbox) that defaults to `false` on a miss —
        // so a freshly-selected model like Opus 4.8 with no stored capability
        // entry would otherwise fall onto the rejected manual path. We OR it
        // with the model-id check so every model the API requires adaptive for
        // routes there regardless of what's stored. The model id is the source
        // of truth; the config flag only *adds* models the static list misses.
        //
        // We translate the user-facing `thinking_budget` (a token count) into
        // an effort level for adaptive mode. Setting it to 0 disables thinking
        // entirely regardless of model.
        let use_adaptive =
            config.supports_adaptive_thinking || supports_adaptive_thinking(&config.model);
        if config.thinking_budget > 0 && config.supports_reasoning_effort {
            tracing::info!(
                "[claude] thinking mode for {}: {} (config flag={}, model-id match={})",
                config.model,
                if use_adaptive { "adaptive" } else { "manual/enabled" },
                config.supports_adaptive_thinking,
                supports_adaptive_thinking(&config.model),
            );
            if use_adaptive {
                body["thinking"] = json!({
                    "type": "adaptive",
                    // Opus 4.7 defaults `display` to "omitted" (thinking field
                    // empty, signature still present). Force summarized so the
                    // UI can render thinking blocks like on older models.
                    "display": "summarized",
                });
                body["output_config"] = json!({
                    "effort": budget_to_effort(config.thinking_budget),
                });
            } else {
                body["thinking"] = json!({
                    "type": "enabled",
                    "budget_tokens": config.thinking_budget,
                });
            }
        } else if config.temperature != 0.7 && config.supports_temperature {
            body["temperature"] = json!(config.temperature);
        }

        // Anthropic exposes web_search / web_fetch as SERVER-side tools. When
        // the user has enabled either, we swap the generic ToolDef (if the
        // executor added one) out of the request body and inject the
        // server-side declaration instead. This way the model only sees one
        // version of each tool and Anthropic executes the network call.
        let mut claude_tools: Vec<serde_json::Value> = Vec::new();
        let mut saw_web_search = false;
        let mut saw_web_fetch = false;
        for t in &tools {
            match t.name.as_str() {
                "web_search" => { saw_web_search = true; }
                "web_fetch" => { saw_web_fetch = true; }
                _ => {
                    claude_tools.push(json!({
                        "name": t.name,
                        "description": t.description,
                        "input_schema": t.parameters,
                    }));
                }
            }
        }
        // Server-side declarations. The `name` field for these must match what
        // the model uses in `server_tool_use.name` — Anthropic uses the
        // short alias ("web_search" / "web_fetch") by convention.
        if saw_web_search || config.web_search_enabled {
            claude_tools.push(json!({
                "type": "web_search_20250305",
                "name": "web_search",
                "max_uses": 8,
            }));
        }
        if saw_web_fetch || config.web_fetch_enabled {
            claude_tools.push(json!({
                "type": "web_fetch_20250910",
                "name": "web_fetch",
                "max_uses": 8,
            }));
        }

        if !claude_tools.is_empty() {
            // Mark the last tool with cache_control — caches the entire tool
            // definitions block (which rarely changes between turns).
            let last_idx = claude_tools.len() - 1;
            for (i, t) in claude_tools.iter_mut().enumerate() {
                if i == last_idx {
                    if let Some(obj) = t.as_object_mut() {
                        obj.insert("cache_control".to_string(), json!({ "type": "ephemeral" }));
                    }
                }
            }
            body["tools"] = json!(claude_tools);
        }

        let mut request = self
            .client
            .post(url)
            .header("x-api-key", &config.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json");

        // Build comma-separated anthropic-beta header from all enabled betas.
        let mut betas: Vec<&str> = Vec::new();
        if config.thinking_budget > 0 && !supports_adaptive_thinking(&config.model) {
            // Interleaved thinking is only valid on the manual-mode path for
            // pre-4.6 models; adaptive mode enables it automatically and the
            // beta header is not valid there.
            betas.push("interleaved-thinking-2025-05-14");
        }
        if saw_web_search || config.web_search_enabled {
            betas.push("web-search-2025-03-05");
        }
        if saw_web_fetch || config.web_fetch_enabled {
            betas.push("web-fetch-2025-09-10");
        }
        if !betas.is_empty() {
            request = request.header("anthropic-beta", betas.join(","));
        }

        // Log the request shape BEFORE we make the call — this is the only
        // place we can see what's about to be sent. If the stream stalls
        // before the first byte, the only thing the log will show is this
        // entry followed by the watchdog's "STALL" warnings, which is enough
        // to pinpoint that Anthropic itself hasn't started responding.
        let approx_body_bytes = serde_json::to_string(&body)
            .map(|s| s.len())
            .unwrap_or(0);
        let approx_system_chars = system_msg.map(|s| s.chars().count()).unwrap_or(0);
        tracing::info!(
            target: "rustic::stream",
            model = %config.model,
            messages = messages.len(),
            tools = tools.len(),
            thinking_budget = config.thinking_budget,
            max_tokens = effective_max_tokens,
            approx_body_bytes,
            approx_system_chars,
            url = %url,
            "[stream] claude request about to be sent"
        );
        let request_start = std::time::Instant::now();

        let resp = super::send_json_with_retry(request, &body, "Claude").await?;
        tracing::info!(
            target: "rustic::stream",
            status = %resp.status(),
            send_ms = request_start.elapsed().as_millis() as u64,
            "[stream] claude HTTP headers returned — beginning SSE parse"
        );
        parse_sse_stream(resp, stream_cb, config.cancel_token.clone()).await
    }

    fn name(&self) -> &str {
        "Claude"
    }

    fn available_models(&self) -> Vec<ModelInfo> {
        crate::model_registry::models_for_provider("Claude")
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

// === SSE stream parsing ===

/// State for a single content block being accumulated from the stream.
enum BlockState {
    Text { text: String },
    Thinking { thinking: String, signature: Option<String> },
    /// Captured at content_block_start; carries an opaque `data` string that
    /// must be echoed back to the API verbatim. No streaming deltas.
    RedactedThinking { data: String },
    ToolUse { id: String, name: String, input_json: String },
    /// Anthropic server-side web_search_tool_result block. Content is attached
    /// to content_block_start (no streaming delta events); we just carry it
    /// through to the final ContentBlock list.
    WebSearchToolResult { tool_use_id: String, content: serde_json::Value },
    /// Anthropic server-side web_fetch_tool_result block.
    WebFetchToolResult { tool_use_id: String, content: serde_json::Value },
}

async fn parse_sse_stream(
    resp: reqwest::Response,
    stream_cb: Option<StreamCallback>,
    cancel_token: Option<Arc<AtomicBool>>,
) -> Result<AiResponse> {
    let mut byte_stream = resp.bytes_stream();
    let mut buffer = String::new();

    // ── Stream watchdog ──────────────────────────────────────────────────
    // The user-visible symptom is "the stream stalls when spawning a sub-agent
    // on a large delegated task." To diagnose, we emit a heartbeat log every
    // few seconds with chunk count, total bytes and ms-since-last-chunk so we
    // can see in the rolling log whether bytes have actually stopped flowing
    // from Anthropic, or whether we're stuck inside the parser. All counters
    // are atomics so the watchdog task reads them without locking the parser
    // loop. The watchdog stops itself when `watchdog_done` flips on exit.
    let stream_start = Instant::now();
    let chunk_count = Arc::new(AtomicU64::new(0));
    let byte_count = Arc::new(AtomicU64::new(0));
    let last_chunk_at_ms = Arc::new(AtomicU64::new(0)); // ms since stream_start
    let event_count = Arc::new(AtomicU64::new(0));
    let watchdog_done = Arc::new(AtomicBool::new(false));
    let watchdog_id = uuid::Uuid::new_v4().to_string()[..8].to_string();
    {
        let chunk_count = Arc::clone(&chunk_count);
        let byte_count = Arc::clone(&byte_count);
        let last_chunk_at_ms = Arc::clone(&last_chunk_at_ms);
        let event_count = Arc::clone(&event_count);
        let done = Arc::clone(&watchdog_done);
        let id = watchdog_id.clone();
        tracing::info!(
            target: "rustic::stream",
            stream_id = %id,
            "[stream] claude SSE stream opened — watchdog armed (heartbeat 5s, stall threshold 10s)"
        );
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(5));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            // Skip the immediate first tick — it would fire before any byte
            // has been received and produce a misleading "stalled at 0ms" log.
            tick.tick().await;
            loop {
                tick.tick().await;
                if done.load(Ordering::Relaxed) {
                    break;
                }
                let elapsed_ms = stream_start.elapsed().as_millis() as u64;
                let last_ms = last_chunk_at_ms.load(Ordering::Relaxed);
                let since_last_ms = elapsed_ms.saturating_sub(last_ms);
                let chunks = chunk_count.load(Ordering::Relaxed);
                let bytes = byte_count.load(Ordering::Relaxed);
                let events = event_count.load(Ordering::Relaxed);
                if since_last_ms >= 10_000 || chunks == 0 {
                    // Either no chunk has arrived yet, or >10s since the last
                    // one. This is the smoking gun for a stalled stream.
                    tracing::warn!(
                        target: "rustic::stream",
                        stream_id = %id,
                        elapsed_ms,
                        since_last_chunk_ms = since_last_ms,
                        chunks,
                        bytes,
                        events,
                        "[stream] STALL — no SSE bytes in over 10s (or none yet received)"
                    );
                } else {
                    tracing::info!(
                        target: "rustic::stream",
                        stream_id = %id,
                        elapsed_ms,
                        since_last_chunk_ms = since_last_ms,
                        chunks,
                        bytes,
                        events,
                        "[stream] heartbeat"
                    );
                }
            }
        });
    }

    // Accumulated state
    let mut blocks: HashMap<usize, BlockState> = HashMap::new();
    let mut block_order: Vec<usize> = Vec::new(); // insertion-order of block indices
    // Diagnostic: tally every `content_block_start` we see, by type. Compared
    // against the final emitted block count at end of stream so we can spot
    // silently-dropped block types (e.g. an unrecognized variant making the
    // whole event fail to deserialize). See the WARN at the bottom.
    let mut block_start_counts: std::collections::HashMap<&'static str, usize> =
        std::collections::HashMap::new();
    let mut input_tokens: u32 = 0;
    let mut output_tokens: u32 = 0;
    let mut cache_read_tokens: u32 = 0;
    let mut cache_write_tokens: u32 = 0;
    let mut stop_reason = StopReason::EndTurn;

    // Drop-guard: whatever exit path we take (Ok, ?, early return on cancel,
    // panic-unwind during await), flipping `watchdog_done` stops the heartbeat
    // task on its next 5s tick. Without this an early-return path would leave
    // the watchdog logging "stalled" forever against a stream that's already
    // closed.
    struct WatchdogGuard(Arc<AtomicBool>, String);
    impl Drop for WatchdogGuard {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Relaxed);
            tracing::info!(
                target: "rustic::stream",
                stream_id = %self.1,
                "[stream] watchdog disarmed"
            );
        }
    }
    let _watchdog_guard = WatchdogGuard(Arc::clone(&watchdog_done), watchdog_id.clone());

    while let Some(chunk) = byte_stream.next().await {
        // Check cancellation token to abort streaming early
        if let Some(ref token) = cancel_token {
            if token.load(Ordering::SeqCst) {
                return Err(anyhow::anyhow!("Task cancelled"));
            }
        }

        let chunk = chunk?;
        // Update watchdog counters — these are read by the heartbeat task so
        // it can spot a stalled stream (no chunks for >10s).
        let now_ms = stream_start.elapsed().as_millis() as u64;
        let prev_chunks = chunk_count.fetch_add(1, Ordering::Relaxed);
        byte_count.fetch_add(chunk.len() as u64, Ordering::Relaxed);
        last_chunk_at_ms.store(now_ms, Ordering::Relaxed);
        if prev_chunks == 0 {
            // First-byte latency is the single most useful number for
            // diagnosing "the stream stalls before any text appears".
            tracing::info!(
                target: "rustic::stream",
                stream_id = %watchdog_id,
                first_byte_ms = now_ms,
                first_chunk_bytes = chunk.len(),
                "[stream] first chunk received from Claude"
            );
        }
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

                    event_count.fetch_add(1, Ordering::Relaxed);
                    let event: SseEvent = match serde_json::from_str(data) {
                        Ok(e) => e,
                        Err(e) => {
                            // Was: silent `continue`. That hid the
                            // redacted_thinking-drop bug for months — when an
                            // SseContentBlock variant we didn't list arrived,
                            // the whole content_block_start event failed to
                            // parse and the block disappeared, which then
                            // corrupted the assistant message and produced the
                            // "thinking or redacted_thinking blocks ... cannot
                            // be modified" 400 on the next turn. Log the raw
                            // payload (truncated) so future drops are visible.
                            let preview = &data[..data.len().min(500)];
                            tracing::warn!(
                                "[claude] SSE event failed to deserialize: {} — raw: {:?}",
                                e, preview
                            );
                            continue;
                        }
                    };

                    match event {
                        SseEvent::MessageStart { message } => {
                            if let Some(u) = message.usage {
                                input_tokens += u.input_tokens.unwrap_or(0);
                                cache_read_tokens += u.cache_read_input_tokens;
                                cache_write_tokens += u.cache_creation_input_tokens;
                            }
                        }

                        SseEvent::ContentBlockStart { index, content_block } => {
                            if !block_order.contains(&index) {
                                block_order.push(index);
                            }
                            let state = match content_block {
                                SseContentBlock::Text { .. } => {
                                    *block_start_counts.entry("text").or_insert(0) += 1;
                                    BlockState::Text { text: String::new() }
                                }
                                SseContentBlock::Thinking { .. } => {
                                    *block_start_counts.entry("thinking").or_insert(0) += 1;
                                    BlockState::Thinking { thinking: String::new(), signature: None }
                                }
                                SseContentBlock::RedactedThinking { data } => {
                                    // Final at content_block_start — no streaming deltas.
                                    // Just preserve the opaque `data` so we can echo it
                                    // back unchanged on the next request.
                                    *block_start_counts.entry("redacted_thinking").or_insert(0) += 1;
                                    BlockState::RedactedThinking { data }
                                }
                                SseContentBlock::ToolUse { id, name } => {
                                    *block_start_counts.entry("tool_use").or_insert(0) += 1;
                                    // Fire the live "tool starting" event so the UI
                                    // can render a tool card with name + spinner
                                    // immediately, before any input_json_delta
                                    // events arrive. server_tool_use is intentionally
                                    // NOT included here — its UI emit happens at
                                    // content_block_stop (below) so the inline
                                    // server-tool-result + text ordering is preserved.
                                    if let Some(cb) = &stream_cb {
                                        cb(ProviderStreamEvent::ToolUseStart {
                                            id: id.clone(),
                                            name: name.clone(),
                                        });
                                    }
                                    BlockState::ToolUse { id, name, input_json: String::new() }
                                }
                                // Anthropic server_tool_use behaves identically to client-side
                                // tool_use on the wire — same streaming pattern for input JSON.
                                // We reuse BlockState::ToolUse so the rest of the parser is
                                // unchanged; the executor distinguishes server-side calls by
                                // finding a paired ToolResult in the same response.
                                SseContentBlock::ServerToolUse { id, name } => {
                                    *block_start_counts.entry("server_tool_use").or_insert(0) += 1;
                                    BlockState::ToolUse {
                                        id,
                                        name,
                                        input_json: String::new(),
                                    }
                                }
                                SseContentBlock::WebSearchToolResult { tool_use_id, content } => {
                                    *block_start_counts.entry("web_search_tool_result").or_insert(0) += 1;
                                    // Result content is final at block_start (no streaming
                                    // deltas). Emit the UI event immediately so the tool card
                                    // appears in-place, before any subsequent text deltas.
                                    if let Some(cb) = &stream_cb {
                                        let stringified = serde_json::to_string(&content)
                                            .unwrap_or_else(|_| content.to_string());
                                        cb(ProviderStreamEvent::ServerToolResult {
                                            tool_use_id: tool_use_id.clone(),
                                            content: stringified,
                                            is_error: false,
                                        });
                                    }
                                    BlockState::WebSearchToolResult { tool_use_id, content }
                                }
                                SseContentBlock::WebFetchToolResult { tool_use_id, content } => {
                                    *block_start_counts.entry("web_fetch_tool_result").or_insert(0) += 1;
                                    if let Some(cb) = &stream_cb {
                                        let stringified = serde_json::to_string(&content)
                                            .unwrap_or_else(|_| content.to_string());
                                        cb(ProviderStreamEvent::ServerToolResult {
                                            tool_use_id: tool_use_id.clone(),
                                            content: stringified,
                                            is_error: false,
                                        });
                                    }
                                    BlockState::WebFetchToolResult { tool_use_id, content }
                                }
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
                                    if let Some(BlockState::Thinking { thinking: acc, .. }) = blocks.get_mut(&index) {
                                        acc.push_str(&thinking);
                                    }
                                }
                                SseDelta::SignatureDelta { signature } => {
                                    if let Some(BlockState::Thinking { signature: sig, .. }) = blocks.get_mut(&index) {
                                        let s = sig.get_or_insert_with(String::new);
                                        s.push_str(&signature);
                                    }
                                }
                                SseDelta::InputJsonDelta { partial_json } => {
                                    if let Some(BlockState::ToolUse { id, name, input_json }) = blocks.get_mut(&index) {
                                        // Forward the fragment to the UI so it can
                                        // animate the input filling in. Skip server-side
                                        // tools — their card is rendered as a single
                                        // unit at content_block_stop alongside the
                                        // server-executed result.
                                        if name != "web_search" && name != "web_fetch" {
                                            if let Some(cb) = &stream_cb {
                                                cb(ProviderStreamEvent::ToolUseInputDelta {
                                                    id: id.clone(),
                                                    partial_json: partial_json.clone(),
                                                });
                                            }
                                        }
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
                                // Anthropic pauses long-running server-tool turns
                                // (web_search / web_fetch) and expects the caller
                                // to resubmit so it can resume and return the
                                // result. Surface it distinctly so the executor
                                // re-sends instead of treating the orphan
                                // server_tool_use as a local tool to run.
                                Some("pause_turn") => StopReason::PauseTurn,
                                Some(other) => StopReason::Error(other.to_string()),
                                None => StopReason::EndTurn,
                            };
                            if let Some(u) = usage {
                                output_tokens += u.output_tokens.unwrap_or(0);
                                // Forward to the executor's stall watchdog so a
                                // stall log can distinguish "model produced
                                // nothing" from "model produced N tokens but is
                                // buffering". Anthropic emits message_delta with
                                // usage periodically during a long response, so
                                // this gives the watchdog a live signal.
                                if let Some(cb) = &stream_cb {
                                    cb(ProviderStreamEvent::PartialUsage {
                                        output_tokens,
                                    });
                                }
                            }
                        }

                        SseEvent::Error { error } => {
                            return Err(anyhow::anyhow!("Claude stream error: {}", error.message));
                        }

                        SseEvent::ContentBlockStop { index } => {
                            if let Some(BlockState::ToolUse { id, name, input_json }) = blocks.get(&index) {
                                if (name == "web_search" || name == "web_fetch")
                                    && stream_cb.is_some()
                                {
                                    // Server-side tools (web_search / web_fetch):
                                    // emit ServerToolUse so the inline card appears
                                    // before subsequent text deltas, in the order
                                    // the model emitted it.
                                    let input: serde_json::Value = if input_json.trim().is_empty() {
                                        json!({})
                                    } else {
                                        serde_json::from_str(input_json).unwrap_or_else(|_| json!({}))
                                    };
                                    if let Some(cb) = &stream_cb {
                                        cb(ProviderStreamEvent::ServerToolUse {
                                            id: id.clone(),
                                            name: name.clone(),
                                            input,
                                        });
                                    }
                                } else if let Some(cb) = &stream_cb {
                                    // Client-side tool_use finalized: tell the UI
                                    // streaming is done so it can flip the card
                                    // out of "args streaming" state. Execution will
                                    // be triggered by the executor after the full
                                    // response returns.
                                    cb(ProviderStreamEvent::ToolUseStop { id: id.clone() });
                                }
                            }
                        }
                        SseEvent::MessageStop | SseEvent::Ping => {}
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
                // Keep thinking blocks even when the visible text is empty —
                // adaptive `display: summarized` legitimately produces a block
                // with no summary text but a real signature, and the API
                // requires it to round-trip on the next turn.
                BlockState::Thinking { thinking, signature } if !thinking.is_empty() || signature.is_some() => {
                    content.push(ContentBlock::Thinking { thinking, signature, duration_secs: None });
                }
                BlockState::RedactedThinking { data } => {
                    content.push(ContentBlock::RedactedThinking { data });
                }
                BlockState::ToolUse { id, name, input_json } => {
                    // Empty input_json is legitimate for tools that take no
                    // parameters (e.g. list_projects, list_active_agents).
                    // Claude sends no input_json_delta events in that case,
                    // which is correct per the streaming spec. Treat it as
                    // an empty object so downstream dispatch doesn't choke.
                    let input: serde_json::Value = if input_json.trim().is_empty() {
                        json!({})
                    } else {
                        match serde_json::from_str(&input_json) {
                            Ok(v) => v,
                            Err(e) => {
                                tracing::warn!("[claude] WARNING: tool '{}' (id={}) has malformed input_json: {} — raw: {:?}",
                                    name, id, e, &input_json[..input_json.len().min(200)]);
                                json!({ "__parse_error": format!("Failed to parse tool arguments: {}. Please retry the tool call with valid JSON parameters.", e) })
                            }
                        }
                    };
                    content.push(ContentBlock::ToolUse { id, name, input, thought_signature: None });
                }
                BlockState::WebSearchToolResult { tool_use_id, content: result_content } => {
                    // Stringify the result array for transport through the
                    // generic ContentBlock::ToolResult. The round-trip
                    // serializer (convert_content_blocks) detects web_search
                    // pairs and re-inflates the JSON back to the server
                    // result-array shape before sending to the API.
                    let stringified = serde_json::to_string(&result_content)
                        .unwrap_or_else(|_| result_content.to_string());
                    content.push(ContentBlock::ToolResult {
                        tool_use_id,
                        content: stringified,
                        is_error: false,
                    });
                }
                BlockState::WebFetchToolResult { tool_use_id, content: result_content } => {
                    let stringified = serde_json::to_string(&result_content)
                        .unwrap_or_else(|_| result_content.to_string());
                    content.push(ContentBlock::ToolResult {
                        tool_use_id,
                        content: stringified,
                        is_error: false,
                    });
                }
                _ => {}
            }
        }
    }

    // Stream summary — compares blocks the server announced (`content_block_start`
    // tallies) against what we actually emitted. If a thinking-family block went
    // missing (most likely a `redacted_thinking` we failed to deserialize), the
    // counts diverge and we surface a WARN with the breakdown. This is the
    // hook for diagnosing future "thinking blocks cannot be modified" 400s:
    // when the error fires, the preceding stream summary will tell you whether
    // a block was silently dropped.
    let total_announced: usize = block_start_counts.values().sum();
    let total_emitted = content.len();
    let mismatch = total_announced != total_emitted;
    let thinking_announced = *block_start_counts.get("thinking").unwrap_or(&0);
    let redacted_announced = *block_start_counts.get("redacted_thinking").unwrap_or(&0);
    let mut emitted_kinds: std::collections::HashMap<&'static str, usize> =
        std::collections::HashMap::new();
    for b in &content {
        let key = match b {
            ContentBlock::Text { .. } => "text",
            ContentBlock::Thinking { .. } => "thinking",
            ContentBlock::RedactedThinking { .. } => "redacted_thinking",
            ContentBlock::ToolUse { .. } => "tool_use",
            ContentBlock::ToolResult { .. } => "tool_result",
            ContentBlock::Image { .. } => "image",
            ContentBlock::ModelSwitch { .. } => "model_switch",
        };
        *emitted_kinds.entry(key).or_insert(0) += 1;
    }
    if mismatch {
        tracing::warn!(
            "[claude] stream block-count mismatch — announced {:?}, emitted {:?}. \
             A redacted_thinking or other unrecognized block likely got dropped; \
             the next API call will probably fail with 'thinking or redacted_thinking \
             blocks ... cannot be modified'.",
            block_start_counts, emitted_kinds
        );
    } else if redacted_announced > 0 {
        tracing::info!(
            "[claude] stream complete with redacted_thinking — announced {:?}, emitted {:?}",
            block_start_counts, emitted_kinds
        );
    } else {
        tracing::debug!(
            "[claude] stream complete — announced {:?}, emitted {:?} (thinking={})",
            block_start_counts, emitted_kinds, thinking_announced
        );
    }

    let total_ms = stream_start.elapsed().as_millis() as u64;
    tracing::info!(
        target: "rustic::stream",
        stream_id = %watchdog_id,
        total_ms,
        chunks = chunk_count.load(Ordering::Relaxed),
        bytes = byte_count.load(Ordering::Relaxed),
        events = event_count.load(Ordering::Relaxed),
        blocks_emitted = total_emitted,
        input_tokens,
        output_tokens,
        cache_read = cache_read_tokens,
        cache_write = cache_write_tokens,
        stop_reason = ?stop_reason,
        "[stream] claude SSE stream finished"
    );

    Ok(AiResponse {
        content,
        usage: TokenUsage {
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_write_tokens,
        },
        stop_reason,
        actual_cost_usd: None,
        served_provider: None,
    })
}

// === SSE event types ===

#[derive(Deserialize)]
#[serde(tag = "type")]
#[allow(dead_code)]
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
#[allow(dead_code)]
enum SseContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "thinking")]
    Thinking { thinking: String },
    /// Redacted reasoning — Anthropic safety stripped the text but the signed
    /// `data` payload MUST round-trip unchanged on the next request, otherwise
    /// the API rejects with "thinking or redacted_thinking blocks in the
    /// latest assistant message cannot be modified". See [`BlockState`].
    #[serde(rename = "redacted_thinking")]
    RedactedThinking { data: String },
    #[serde(rename = "tool_use")]
    ToolUse { id: String, name: String },
    /// Anthropic server-side tool call (web_search / web_fetch). Streams input
    /// via input_json_delta like a normal tool_use.
    #[serde(rename = "server_tool_use")]
    ServerToolUse { id: String, name: String },
    /// Anthropic-executed web_search result. Content is fully present at
    /// block_start — no streaming deltas.
    #[serde(rename = "web_search_tool_result")]
    WebSearchToolResult {
        tool_use_id: String,
        content: serde_json::Value,
    },
    /// Anthropic-executed web_fetch result.
    #[serde(rename = "web_fetch_tool_result")]
    WebFetchToolResult {
        tool_use_id: String,
        content: serde_json::Value,
    },
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum SseDelta {
    #[serde(rename = "text_delta")]
    TextDelta { text: String },
    #[serde(rename = "thinking_delta")]
    ThinkingDelta { thinking: String },
    #[serde(rename = "signature_delta")]
    SignatureDelta { signature: String },
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
    #[serde(default)]
    cache_creation_input_tokens: u32,
}

#[derive(Deserialize)]
struct SseError {
    message: String,
}

// === Message conversion ===

/// Drop orphan Anthropic server-tool blocks (`srvtoolu_*` ids) as a final
/// safety net at the serialization boundary.
///
/// Legit server-tool calls (web_search / web_fetch) are emitted INLINE: the
/// `server_tool_use` and its `web_*_tool_result` share one assistant message
/// (we model both as ToolUse/ToolResult carrying the same `srvtoolu_` id). Any
/// `srvtoolu_` block that is NOT half of such a same-message pair is an orphan:
///   - a stale local `tool_result` with a `srvtoolu_` id, persisted before the
///     executor learned never to run server tools locally, or
///   - a `server_tool_use` whose result was lost (stream cut / paused turn).
/// Sending either orphans the id and earns a 400 ("tool_result ... must have a
/// corresponding tool_use in the previous message"). Stripping them here keeps
/// the request valid AND heals conversations persisted before the fix.
///
/// `is_char_boundary`-free and allocation-light: only messages that actually
/// contain a `srvtoolu_` block are rebuilt.
fn drop_orphan_server_tool_blocks(content: &[ContentBlock]) -> std::borrow::Cow<'_, [ContentBlock]> {
    use std::collections::HashSet;
    let has_server_block = content.iter().any(|b| match b {
        ContentBlock::ToolUse { id, .. } => id.starts_with("srvtoolu_"),
        ContentBlock::ToolResult { tool_use_id, .. } => tool_use_id.starts_with("srvtoolu_"),
        _ => false,
    });
    if !has_server_block {
        return std::borrow::Cow::Borrowed(content);
    }
    // ids that form a complete inline pair within THIS message.
    let mut uses: HashSet<&str> = HashSet::new();
    let mut results: HashSet<&str> = HashSet::new();
    for b in content {
        match b {
            ContentBlock::ToolUse { id, .. } if id.starts_with("srvtoolu_") => {
                uses.insert(id.as_str());
            }
            ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id.starts_with("srvtoolu_") => {
                results.insert(tool_use_id.as_str());
            }
            _ => {}
        }
    }
    let kept: Vec<ContentBlock> = content
        .iter()
        .filter(|b| match b {
            ContentBlock::ToolUse { id, .. } if id.starts_with("srvtoolu_") => {
                results.contains(id.as_str())
            }
            ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id.starts_with("srvtoolu_") => {
                uses.contains(tool_use_id.as_str())
            }
            _ => true,
        })
        .cloned()
        .collect();
    std::borrow::Cow::Owned(kept)
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
                let sanitized = drop_orphan_server_tool_blocks(&msg.content);
                let content = convert_content_blocks(&sanitized);
                // A message left empty after stripping orphans is invalid to
                // send — skip it rather than 400 on "content cannot be empty".
                if content.as_array().map(|a| a.is_empty()).unwrap_or(false) {
                    continue;
                }
                api_msgs.push(json!({ "role": "user", "content": content }));
            }
            Role::Assistant => {
                let sanitized = drop_orphan_server_tool_blocks(&msg.content);
                let content = convert_content_blocks(&sanitized);
                if content.as_array().map(|a| a.is_empty()).unwrap_or(false) {
                    continue;
                }
                api_msgs.push(json!({ "role": "assistant", "content": content }));
            }
        }
    }

    (system, api_msgs)
}

/// Stamp `cache_control: ephemeral` on a "rolling stable prefix" breakpoint —
/// specifically, the last block of the *second-to-last* message. We also
/// place one on the very last message so the current turn seeds the next
/// request's cache.
///
/// Why two breakpoints: Anthropic allows up to 4 cache_control markers and
/// matches the longest cached prefix. By marking N-1 as well as N, the prefix
/// through N-1 stays stable across turns (it doesn't change between requests),
/// so request N+1 gets a cache hit on everything up through turn N-1 — which
/// would otherwise be invalidated because the "last message" on every request
/// is always different.
///
/// On a single-message conversation, only the last gets marked.
///
/// **CRITICAL**: cache_control must NOT be attached to a `thinking` or
/// `redacted_thinking` block. Anthropic treats *any* field addition on those
/// blocks as a modification and rejects the request with
/// "thinking or redacted_thinking blocks in the latest assistant message
/// cannot be modified" (HTTP 400). This bites whenever an assistant message
/// ends with a thinking block — common in adaptive-thinking mode (Opus 4.7
/// default) when the model finishes its turn with a thinking block as the
/// last content, or whenever safety-redacted thinking is appended after the
/// visible output. We walk the block list *backwards* and attach the marker
/// to the most recent non-thinking block instead. If every block in the
/// message is thinking/redacted_thinking (rare but possible), we skip the
/// marker on that message entirely — one cache breakpoint lost, but the API
/// call still succeeds.
fn apply_message_cache_breakpoint(mut msgs: Vec<serde_json::Value>) -> Vec<serde_json::Value> {
    let len = msgs.len();
    let mark = |msg: &mut serde_json::Value| {
        let Some(content) = msg.get_mut("content").and_then(|c| c.as_array_mut()) else {
            return;
        };
        // Walk backwards looking for the latest block that is safe to stamp.
        // Skip thinking / redacted_thinking — Anthropic counts any added field
        // on those as a modification of the assistant message and 400s.
        for block in content.iter_mut().rev() {
            let is_thinking = block
                .get("type")
                .and_then(|t| t.as_str())
                .map(|t| t == "thinking" || t == "redacted_thinking")
                .unwrap_or(false);
            if is_thinking {
                continue;
            }
            if let Some(obj) = block.as_object_mut() {
                obj.insert(
                    "cache_control".to_string(),
                    json!({ "type": "ephemeral" }),
                );
            }
            return;
        }
        // All blocks were thinking/redacted_thinking — skip the breakpoint.
        // Losing one cache marker is far cheaper than a hard 400.
        tracing::debug!(
            "[claude] apply_message_cache_breakpoint: message has only thinking blocks; skipping cache_control to avoid \
             'thinking blocks cannot be modified' 400"
        );
    };
    if len >= 2 {
        // Rolling prefix: covers everything through the previous turn.
        mark(&mut msgs[len - 2]);
    }
    if len >= 1 {
        // Seed for the NEXT request's cache.
        mark(&mut msgs[len - 1]);
    }
    msgs
}

fn convert_content_blocks(blocks: &[ContentBlock]) -> serde_json::Value {
    // Build a map of tool_use_id → tool_name for the blocks in THIS message.
    // When an assistant turn contains a ToolUse named web_search/web_fetch
    // paired with a ToolResult for the same id, both blocks came from
    // Anthropic's server-side execution and must be re-serialized using the
    // native server_tool_use / web_*_tool_result shapes. Client-side results
    // (produced locally for OpenAI/Compatible) always arrive in a separate
    // user turn, so they never match by same-message pairing.
    let mut server_tool_ids: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for b in blocks {
        if let ContentBlock::ToolUse { id, name, .. } = b {
            if name == "web_search" || name == "web_fetch" {
                server_tool_ids.insert(id.clone(), name.clone());
            }
        }
    }

    let parts: Vec<serde_json::Value> = blocks
        .iter()
        .map(|b| match b {
            ContentBlock::Text { text } => json!({ "type": "text", "text": text }),
            ContentBlock::ToolUse { id, name, input, .. } => {
                let block_type = if name == "web_search" || name == "web_fetch" {
                    "server_tool_use"
                } else {
                    "tool_use"
                };
                json!({ "type": block_type, "id": id, "name": name, "input": input })
            }
            ContentBlock::ToolResult { tool_use_id, content, is_error } => {
                if let Some(server_name) = server_tool_ids.get(tool_use_id) {
                    // Server-side result — re-inflate the stringified content
                    // back to the original JSON array/object so Anthropic sees
                    // the exact shape it emitted last turn.
                    let result_type = if server_name == "web_search" {
                        "web_search_tool_result"
                    } else {
                        "web_fetch_tool_result"
                    };
                    let content_val: serde_json::Value = serde_json::from_str(content)
                        .unwrap_or_else(|_| json!(content));
                    json!({
                        "type": result_type,
                        "tool_use_id": tool_use_id,
                        "content": content_val,
                    })
                } else {
                    json!({
                        "type": "tool_result",
                        "tool_use_id": tool_use_id,
                        "content": content,
                        "is_error": is_error,
                    })
                }
            }
            ContentBlock::Thinking { thinking, signature, .. } => {
                // Must be re-sent to API when present in assistant turns
                let mut obj = json!({ "type": "thinking", "thinking": thinking });
                if let Some(sig) = signature {
                    obj["signature"] = json!(sig);
                }
                obj
            }
            ContentBlock::RedactedThinking { data } => {
                // Round-trips opaque. Same rule as thinking blocks — the API
                // rejects any modification (or omission) of these in the
                // latest assistant message.
                json!({ "type": "redacted_thinking", "data": data })
            }
            ContentBlock::Image { media_type, data } => {
                json!({
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": media_type,
                        "data": data,
                    }
                })
            }
            // UI-only marker — filtered out before reaching the provider
            ContentBlock::ModelSwitch { .. } => json!(null),
        })
        .filter(|v| !v.is_null())
        .collect();
    json!(parts)
}

#[cfg(test)]
mod orphan_server_tool_tests {
    //! Guards the `srvtoolu_*` orphan-stripping safety net that prevents the
    //! Anthropic 400 "tool_result ... must have a corresponding tool_use in the
    //! previous message" from a server-tool (web_fetch / web_search) call whose
    //! result wasn't inlined.
    use super::*;

    fn srv_use(id: &str, name: &str) -> ContentBlock {
        ContentBlock::ToolUse {
            id: id.to_string(),
            name: name.to_string(),
            input: json!({}),
            thought_signature: None,
        }
    }
    fn result(id: &str, content: &str) -> ContentBlock {
        ContentBlock::ToolResult {
            tool_use_id: id.to_string(),
            content: content.to_string(),
            is_error: false,
        }
    }
    fn text(t: &str) -> ContentBlock {
        ContentBlock::Text { text: t.to_string() }
    }

    #[test]
    fn inline_server_pair_is_preserved() {
        // A legit server-tool turn: server_tool_use + result in ONE assistant
        // message. Both must survive and round-trip to native shapes.
        let blocks = vec![
            text("let me look that up"),
            srv_use("srvtoolu_A", "web_fetch"),
            result("srvtoolu_A", "{\"type\":\"web_fetch_result\"}"),
        ];
        let kept = drop_orphan_server_tool_blocks(&blocks);
        assert_eq!(kept.len(), 3, "inline pair must not be stripped");
        let json = convert_content_blocks(&kept);
        let arr = json.as_array().unwrap();
        assert_eq!(arr[1]["type"], "server_tool_use");
        assert_eq!(arr[2]["type"], "web_fetch_tool_result");
    }

    #[test]
    fn orphan_server_tool_use_is_dropped() {
        // Paused / lost result: assistant ends with a lone server_tool_use.
        let blocks = vec![text("searching"), srv_use("srvtoolu_B", "web_fetch")];
        let kept = drop_orphan_server_tool_blocks(&blocks);
        assert_eq!(kept.len(), 1);
        assert!(matches!(kept[0], ContentBlock::Text { .. }));
    }

    #[test]
    fn orphan_server_result_in_user_message_is_dropped() {
        // The exact poisoned shape: a stale client-produced tool_result carrying
        // a srvtoolu_ id sits alone in a user message (no same-message use).
        let user = vec![result("srvtoolu_C", "locally fetched body")];
        let kept = drop_orphan_server_tool_blocks(&user);
        assert!(kept.is_empty(), "orphan server result must be stripped");
    }

    #[test]
    fn convert_messages_skips_message_emptied_by_stripping() {
        // assistant: lone orphan server_tool_use → becomes empty → skipped.
        // user: lone orphan server result → becomes empty → skipped.
        // Only the surrounding real turns survive.
        let messages = vec![
            Message { role: Role::User, content: vec![text("fetch example.com")] },
            Message { role: Role::Assistant, content: vec![srv_use("srvtoolu_D", "web_fetch")] },
            Message { role: Role::User, content: vec![result("srvtoolu_D", "stale local body")] },
            Message { role: Role::Assistant, content: vec![text("done")] },
        ];
        let (_sys, api) = convert_messages(&messages);
        assert_eq!(api.len(), 2, "the two orphan-only messages must be skipped");
        assert_eq!(api[0]["role"], "user");
        assert_eq!(api[1]["role"], "assistant");
        // No generic tool_result with a srvtoolu_ id leaked through.
        let dump = serde_json::to_string(&api).unwrap();
        assert!(!dump.contains("srvtoolu_D"));
    }

    #[test]
    fn client_tool_results_are_untouched() {
        // Regular client tools (toolu_ ids) must pass through unchanged.
        let user = vec![result("toolu_normal", "ok"), text("and more")];
        let kept = drop_orphan_server_tool_blocks(&user);
        assert_eq!(kept.len(), 2);
    }
}

#[cfg(test)]
mod sse_snapshot_tests {
    //! Snapshot tests for the on-the-wire shape of Anthropic's SSE events.
    //! Catches upstream API drift before users do — if Anthropic renames a
    //! field or adds a required one, deserialization here fails.
    use super::*;

    fn parse(s: &str) -> SseEvent {
        serde_json::from_str(s).expect("SseEvent should deserialize")
    }

    #[test]
    fn message_start_with_usage() {
        let json = r#"{
            "type": "message_start",
            "message": {
                "usage": {
                    "input_tokens": 100,
                    "output_tokens": 1,
                    "cache_creation_input_tokens": 200,
                    "cache_read_input_tokens": 300
                }
            }
        }"#;
        let evt = parse(json);
        match evt {
            SseEvent::MessageStart { message } => {
                let u = message.usage.expect("usage present");
                assert_eq!(u.input_tokens, Some(100));
                assert_eq!(u.cache_read_input_tokens, 300);
                assert_eq!(u.cache_creation_input_tokens, 200);
            }
            _ => panic!("expected MessageStart"),
        }
    }

    #[test]
    fn content_block_start_text() {
        let json = r#"{
            "type": "content_block_start",
            "index": 0,
            "content_block": {"type": "text", "text": ""}
        }"#;
        let evt = parse(json);
        match evt {
            SseEvent::ContentBlockStart { index, content_block } => {
                assert_eq!(index, 0);
                assert!(matches!(content_block, SseContentBlock::Text { .. }));
            }
            _ => panic!("expected ContentBlockStart"),
        }
    }

    #[test]
    fn content_block_start_redacted_thinking() {
        // Anthropic's safety system can return a `redacted_thinking` block
        // with an opaque `data` payload that MUST round-trip unchanged.
        // Before this variant existed in the parser, the whole event silently
        // failed to deserialize and the block disappeared, which corrupted
        // the assistant message and produced a 400 "thinking or
        // redacted_thinking blocks ... cannot be modified" on the next turn.
        let json = r#"{
            "type": "content_block_start",
            "index": 0,
            "content_block": {"type": "redacted_thinking", "data": "EncRYPt3dPay10AD=="}
        }"#;
        let evt = parse(json);
        match evt {
            SseEvent::ContentBlockStart { index, content_block } => {
                assert_eq!(index, 0);
                match content_block {
                    SseContentBlock::RedactedThinking { data } => {
                        assert_eq!(data, "EncRYPt3dPay10AD==");
                    }
                    _ => panic!("expected RedactedThinking variant"),
                }
            }
            _ => panic!("expected ContentBlockStart"),
        }
    }

    #[test]
    fn redacted_thinking_round_trips_through_convert() {
        use crate::provider::Message;
        let block = ContentBlock::RedactedThinking {
            data: "EncRYPt3dPay10AD==".to_string(),
        };
        let serialized = convert_content_blocks(&[block]);
        let arr = serialized.as_array().expect("array");
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["type"], "redacted_thinking");
        assert_eq!(arr[0]["data"], "EncRYPt3dPay10AD==");
        // Suppress unused warning on Message import in older builds.
        let _ = std::mem::size_of::<Message>();
    }

    #[test]
    fn content_block_start_tool_use() {
        let json = r#"{
            "type": "content_block_start",
            "index": 1,
            "content_block": {"type": "tool_use", "id": "toolu_xyz", "name": "read_file"}
        }"#;
        let evt = parse(json);
        match evt {
            SseEvent::ContentBlockStart { content_block, .. } => match content_block {
                SseContentBlock::ToolUse { id, name } => {
                    assert_eq!(id, "toolu_xyz");
                    assert_eq!(name, "read_file");
                }
                _ => panic!("expected ToolUse"),
            },
            _ => panic!("expected ContentBlockStart"),
        }
    }

    #[test]
    fn content_block_delta_text() {
        let json = r#"{
            "type": "content_block_delta",
            "index": 0,
            "delta": {"type": "text_delta", "text": "Hello"}
        }"#;
        let evt = parse(json);
        match evt {
            SseEvent::ContentBlockDelta { delta, .. } => match delta {
                SseDelta::TextDelta { text } => assert_eq!(text, "Hello"),
                _ => panic!("expected TextDelta"),
            },
            _ => panic!("expected ContentBlockDelta"),
        }
    }

    #[test]
    fn content_block_delta_input_json() {
        let json = r#"{
            "type": "content_block_delta",
            "index": 1,
            "delta": {"type": "input_json_delta", "partial_json": "{\"path\""}
        }"#;
        let evt = parse(json);
        match evt {
            SseEvent::ContentBlockDelta { delta, .. } => match delta {
                SseDelta::InputJsonDelta { partial_json } => assert_eq!(partial_json, "{\"path\""),
                _ => panic!("expected InputJsonDelta"),
            },
            _ => panic!("expected ContentBlockDelta"),
        }
    }

    #[test]
    fn content_block_delta_thinking() {
        let json = r#"{
            "type": "content_block_delta",
            "index": 0,
            "delta": {"type": "thinking_delta", "thinking": "let me think"}
        }"#;
        let evt = parse(json);
        match evt {
            SseEvent::ContentBlockDelta { delta, .. } => match delta {
                SseDelta::ThinkingDelta { thinking } => assert_eq!(thinking, "let me think"),
                _ => panic!("expected ThinkingDelta"),
            },
            _ => panic!("expected ContentBlockDelta"),
        }
    }

    #[test]
    fn message_stop() {
        let json = r#"{"type": "message_stop"}"#;
        assert!(matches!(parse(json), SseEvent::MessageStop));
    }

    #[test]
    fn cache_breakpoint_skips_trailing_thinking_block() {
        // Regression test for the "thinking or redacted_thinking blocks in the
        // latest assistant message cannot be modified" 400. cache_control
        // counts as a field modification on a thinking block, so when an
        // assistant message ends with thinking, the marker must move to an
        // earlier non-thinking block (or be omitted entirely).
        let msgs = vec![
            json!({
                "role": "user",
                "content": [{"type": "text", "text": "hi"}]
            }),
            // Latest assistant — ends with a thinking block.
            json!({
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "let me work on this"},
                    {"type": "tool_use", "id": "t1", "name": "read_file", "input": {}},
                    {"type": "thinking", "thinking": "reasoning...", "signature": "sig=="}
                ]
            }),
        ];
        let out = apply_message_cache_breakpoint(msgs);

        // The thinking block must not have cache_control on it.
        let last_msg = &out[1];
        let content = last_msg["content"].as_array().expect("content array");
        let thinking_block = content
            .iter()
            .find(|b| b["type"] == "thinking")
            .expect("thinking block");
        assert!(
            thinking_block.get("cache_control").is_none(),
            "cache_control must NOT be added to a thinking block — Anthropic rejects \
             it as a modification. Got: {}",
            thinking_block
        );

        // The marker should have moved back to the tool_use block (the latest
        // non-thinking block).
        let tool_block = content
            .iter()
            .find(|b| b["type"] == "tool_use")
            .expect("tool_use block");
        assert!(
            tool_block.get("cache_control").is_some(),
            "cache_control should fall back to the most recent non-thinking block"
        );

        // The second-to-last message marker on a 2-message conversation lands
        // on the user message (a normal text block).
        let user_msg = &out[0];
        let user_content = user_msg["content"].as_array().expect("user content");
        assert!(
            user_content[0].get("cache_control").is_some(),
            "user message's text block should still be marked"
        );
    }

    #[test]
    fn cache_breakpoint_skips_trailing_redacted_thinking_block() {
        // Same rule applies to redacted_thinking — opaque safety-redacted
        // reasoning whose JSON shape must round-trip exactly.
        let msgs = vec![json!({
            "role": "assistant",
            "content": [
                {"type": "text", "text": "ok"},
                {"type": "redacted_thinking", "data": "OpAqUe=="}
            ]
        })];
        let out = apply_message_cache_breakpoint(msgs);
        let content = out[0]["content"].as_array().unwrap();
        let redacted = &content[1];
        assert_eq!(redacted["type"], "redacted_thinking");
        assert!(
            redacted.get("cache_control").is_none(),
            "cache_control must NOT be added to a redacted_thinking block"
        );
        let text = &content[0];
        assert!(
            text.get("cache_control").is_some(),
            "marker should fall back to the preceding text block"
        );
    }

    #[test]
    fn cache_breakpoint_when_only_thinking_blocks_skips_marker() {
        // Degenerate case: the entire assistant message is thinking blocks.
        // Better to lose the cache breakpoint than to corrupt the request.
        let msgs = vec![json!({
            "role": "assistant",
            "content": [
                {"type": "thinking", "thinking": "a", "signature": "s1"},
                {"type": "redacted_thinking", "data": "r1"}
            ]
        })];
        let out = apply_message_cache_breakpoint(msgs);
        let content = out[0]["content"].as_array().unwrap();
        for block in content {
            assert!(
                block.get("cache_control").is_none(),
                "no block in an all-thinking message should receive cache_control"
            );
        }
    }

    #[test]
    fn ping() {
        let json = r#"{"type": "ping"}"#;
        assert!(matches!(parse(json), SseEvent::Ping));
    }
}

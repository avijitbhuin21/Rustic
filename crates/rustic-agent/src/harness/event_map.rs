//! Translate Claude Code's stream-json NDJSON envelopes into `HarnessEvent`.
//!
//! Handles: `system/init` (session_id), `stream_event` (partial text/thinking
//! streaming), `assistant` (tool_use blocks; text skipped to avoid double-render),
//! `user` (tool results), and `result` (usage + turn complete).

use crate::harness::HarnessEvent;
use serde_json::Value;

/// Decode a single envelope. Returns an empty Vec for envelope shapes we
/// don't translate yet (so the caller drops them on the floor rather than
/// aborting the session). One envelope can fan out into multiple events —
/// e.g. a `result` envelope produces both `Usage` and `TurnComplete`.
pub fn translate_claude_envelope(env: &Value) -> Vec<HarnessEvent> {
    let Some(kind) = env.get("type").and_then(Value::as_str) else {
        return Vec::new();
    };

    match kind {
        "system" => translate_system(env),
        "stream_event" => translate_stream_event(env),
        "assistant" => translate_assistant_consolidated(env),
        "user" => translate_user(env),
        "result" => translate_result(env),
        // The CLI uses `control_request` to ask the host for permission
        // before running a tool. We translate the `can_use_tool` subtype to
        // a `PermissionRequest` event; everything else (initialize replies,
        // set_permission_mode acknowledgements, ...) is silently ignored —
        // we don't issue those control_requests, so we shouldn't see them.
        "control_request" => translate_control_request(env),
        // Telemetry-only kinds (`tool_progress`, `auth_status`, ...) — these
        // never need user interaction so we drop them silently.
        "tool_progress" | "auth_status" | "rate_limit_update" => Vec::new(),
        // Anything else: surface as UnknownPrompt instead of dropping (P0.9).
        // The CLI surface includes new envelope types over time (MCP elicit,
        // hook_callback, ExitPlanMode approval, ...). If we don't have a
        // typed handler yet, fail loud — the user sees a generic dialog
        // with the raw payload — rather than fail silent and leave the
        // agent waiting on input that never arrives.
        other => translate_unknown_envelope("claude", other, env),
    }
}

/// P0.9 catch-all: synthesize an `UnknownPrompt` event from an unrecognized
/// envelope. Only emits if the envelope looks interactive (carries a
/// `request_id` or a recognizable user-message field). Non-interactive
/// envelopes (telemetry, lifecycle echoes) return an empty Vec so the agent
/// loop isn't polluted with spurious dialogs.
pub(crate) fn translate_unknown_envelope(
    source: &str,
    envelope_type: &str,
    env: &Value,
) -> Vec<HarnessEvent> {
    // Heuristics for "this looks like a prompt the user needs to answer":
    // a request_id field, a question/message/prompt field, or a known
    // approval/elicitation subtype.
    let request_id = env
        .get("request_id")
        .and_then(Value::as_str)
        .or_else(|| env.pointer("/request/request_id").and_then(Value::as_str))
        .map(str::to_string);
    let subtype = env
        .get("subtype")
        .and_then(Value::as_str)
        .or_else(|| env.pointer("/request/subtype").and_then(Value::as_str));
    let looks_interactive = request_id.is_some()
        || matches!(
            subtype,
            Some("elicitation") | Some("hook_callback") | Some("approval")
        )
        || env.get("question").is_some()
        || env.get("prompt").is_some()
        || env.get("message").is_some();
    if !looks_interactive {
        tracing::debug!(
            source,
            envelope_type,
            "harness: ignoring non-interactive envelope"
        );
        return Vec::new();
    }
    // Best-effort summary: prefer a question/prompt/message field, else stringify a short slice.
    let summary = env
        .get("question")
        .and_then(Value::as_str)
        .or_else(|| env.get("prompt").and_then(Value::as_str))
        .or_else(|| env.get("message").and_then(Value::as_str))
        .or_else(|| {
            env.pointer("/request/message")
                .and_then(Value::as_str)
        })
        .map(str::to_string)
        .unwrap_or_else(|| {
            format!(
                "The agent ({}) is requesting input via an envelope type Rustic does not yet render \
                 with a custom dialog ('{}'). Review the raw payload and either reply or abort.",
                source, envelope_type
            )
        });
    tracing::warn!(
        source,
        envelope_type,
        ?request_id,
        "harness: unhandled interactive envelope — surfacing as UnknownPrompt (P0.9). \
         Add a typed handler in event_map* to render this with a proper dialog."
    );
    vec![HarnessEvent::UnknownPrompt {
        envelope_type: envelope_type.to_string(),
        request_id,
        summary,
        raw: env.clone(),
    }]
}

/// Detect a `can_use_tool` permission prompt and surface it as a
/// `HarnessEvent::PermissionRequest`. Also handles `user_question` prompts
/// and surfaces them as `HarnessEvent::UserQuestion`. Envelope shapes:
///
/// ```json
/// { "type": "control_request",
///   "request_id": "uuid",
///   "request": {
///     "subtype": "can_use_tool",
///     "tool_name": "Bash",
///     "input": { ... },
///     "tool_use_id": "toolu_..."
///   } }
/// ```
///
/// ```json
/// { "type": "control_request",
///   "request_id": "uuid",
///   "subtype": "user_question",
///   "question": "...",
///   "choices": ["opt1", "opt2"] }
/// ```
///
/// We don't echo the CLI-provided `permission_suggestions` at this layer —
/// that's the harness session's responsibility when building a response,
/// because the suggestions only matter for `AcceptForSession` and the host
/// doesn't need them to render the prompt.
fn translate_control_request(env: &Value) -> Vec<HarnessEvent> {
    // The `user_question` subtype lives at the top level of the envelope,
    // not nested inside a `request` object.
    let top_subtype = env.get("subtype").and_then(Value::as_str);
    if top_subtype == Some("user_question") {
        let Some(request_id) = env.get("request_id").and_then(Value::as_str) else {
            return Vec::new();
        };
        let question = env
            .get("question")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let choices: Vec<String> = env
            .get("choices")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        return vec![HarnessEvent::UserQuestion {
            request_id: request_id.to_string(),
            question,
            choices,
        }];
    }

    let request_subtype = env
        .get("request")
        .and_then(|r| r.get("subtype"))
        .and_then(Value::as_str);

    // P0.9 fix #8: typed elicitation handling. Claude Code wraps MCP
    // elicitations as `control_request.request.subtype = "elicitation"`
    // (the CLI's MCP host forwards the elicitation from the MCP server
    // up to us). Emit a typed McpElicit event so the frontend can
    // render a schema-driven form instead of the raw-text fallback.
    if request_subtype == Some("elicitation") {
        if let Some(request_id) = env.get("request_id").and_then(Value::as_str) {
            let req = env.get("request").expect("checked above");
            let message = req
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("The agent's MCP server is requesting input.")
                .to_string();
            let schema = req.get("schema").cloned().unwrap_or(Value::Null);
            return vec![HarnessEvent::McpElicit {
                request_id: request_id.to_string(),
                message,
                schema,
            }];
        }
        // No request_id → fall through to unknown-envelope catch-all.
    }

    if request_subtype != Some("can_use_tool") {
        // P0.9: a `control_request` we don't recognise (hook_callback,
        // anything new) used to be silently dropped here. Route it
        // through the unknown-envelope path so the user sees a dialog
        // instead of the agent hanging on input that never arrives.
        let subtype = request_subtype
            .or_else(|| env.get("subtype").and_then(Value::as_str))
            .unwrap_or("unknown");
        return translate_unknown_envelope(
            "claude",
            &format!("control_request:{subtype}"),
            env,
        );
    }
    let Some(request_id) = env.get("request_id").and_then(Value::as_str) else {
        return Vec::new();
    };
    let req = env.get("request").expect("checked above");
    let Some(tool_name) = req.get("tool_name").and_then(Value::as_str) else {
        return Vec::new();
    };
    let input = req.get("input").cloned().unwrap_or(Value::Null);
    let tool_use_id = req
        .get("tool_use_id")
        .and_then(Value::as_str)
        .map(str::to_string);

    // P0.9 fix #8: special-case the `exit_plan_mode` tool. The CLI
    // routes it through the normal can_use_tool permission flow, but
    // semantically it's "the agent has a plan to share, approve it?" —
    // not "the agent wants to run a command, approve it?". Emit
    // ApprovalRequest so the frontend renders a proper plan-review
    // card with the payload rendered as markdown.
    if is_approval_gated_tool(tool_name) {
        return vec![HarnessEvent::ApprovalRequest {
            request_id: request_id.to_string(),
            tool_use_id,
            kind: tool_name.to_string(),
            payload: input,
        }];
    }

    vec![HarnessEvent::PermissionRequest {
        request_id: request_id.to_string(),
        tool_use_id,
        tool_name: tool_name.to_string(),
        input,
    }]
}

/// P0.9 fix #8: tools whose `can_use_tool` flow we want to surface as a
/// typed ApprovalRequest instead of the generic PermissionRequest row.
/// Add new names here as the CLI introduces them; the frontend handles
/// unknown `kind` values by falling back to the generic permission card.
fn is_approval_gated_tool(tool_name: &str) -> bool {
    matches!(tool_name, "exit_plan_mode" | "ExitPlanMode")
}

fn translate_system(env: &Value) -> Vec<HarnessEvent> {
    if env.get("subtype").and_then(Value::as_str) != Some("init") {
        return Vec::new();
    }
    let Some(session_id) = env.get("session_id").and_then(Value::as_str) else {
        return Vec::new();
    };
    if session_id.is_empty() {
        return Vec::new();
    }
    // P0.8: capture model + auth mode from session init so cost accounting
    // doesn't depend on the caller threading the model through. Claude Code
    // emits these on every spawn; older versions / Codex may omit one or
    // both and the harness_runtime falls back to the user-picked model
    // (and prints no auth tag).
    let model = env
        .get("model")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let auth_mode = env
        .get("apiKeySource")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    vec![HarnessEvent::SessionReady {
        session_id: session_id.to_string(),
        model,
        auth_mode,
    }]
}

/// `stream_event` envelopes wrap the per-block deltas we want for true
/// character streaming. The shape is `{"type":"stream_event","event":{...}}`
/// where `event.type` is one of Anthropic's SSE event types.
fn translate_stream_event(env: &Value) -> Vec<HarnessEvent> {
    let Some(event) = env.get("event") else {
        return Vec::new();
    };
    let Some(event_type) = event.get("type").and_then(Value::as_str) else {
        return Vec::new();
    };

    match event_type {
        // Per-character text & thinking deltas — the whole reason we asked
        // for partial messages. `delta.type` distinguishes them.
        "content_block_delta" => {
            let Some(delta) = event.get("delta") else {
                return Vec::new();
            };
            match delta.get("type").and_then(Value::as_str) {
                Some("text_delta") => delta
                    .get("text")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(|s| {
                        vec![HarnessEvent::TextDelta {
                            text: s.to_string(),
                        }]
                    })
                    .unwrap_or_default(),
                Some("thinking_delta") => delta
                    .get("thinking")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(|s| {
                        vec![HarnessEvent::ThinkingDelta {
                            text: s.to_string(),
                        }]
                    })
                    .unwrap_or_default(),
                // `input_json_delta` carries tool_use input fragments — we
                // skip them here and read the assembled input from the
                // consolidated assistant envelope instead.
                _ => Vec::new(),
            }
        }
        // The other stream_event subtypes (message_start, content_block_start,
        // content_block_stop, message_delta, message_stop, ping) carry no
        // user-visible payload that the consolidated envelopes don't already
        // surface. Ignore.
        _ => Vec::new(),
    }
}

/// Consolidated `assistant` envelope. Text blocks are skipped because they
/// were already emitted as `TextDelta`s via `stream_event` translation.
/// Tool-use blocks are emitted here because their `input` JSON is fully
/// assembled at this point.
fn translate_assistant_consolidated(env: &Value) -> Vec<HarnessEvent> {
    let Some(content) = env.pointer("/message/content").and_then(Value::as_array) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for block in content {
        match block.get("type").and_then(Value::as_str) {
            Some("tool_use") => {
                let id = block
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let name = block
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if id.is_empty() || name.is_empty() {
                    continue;
                }
                let input = block.get("input").cloned().unwrap_or(Value::Null);
                out.push(HarnessEvent::ToolUse {
                    tool_use_id: id,
                    name,
                    input,
                    // Diff payload assembly lands in chunk 3c.
                    diff_payload: None,
                });
            }
            // Text already streamed; thinking already streamed. Anything
            // unfamiliar (e.g. server-tool blocks Anthropic might add later)
            // is silently skipped.
            _ => {}
        }
    }
    out
}

/// `user` envelopes from the CLI echo tool results back. Shape:
///   {"type":"user","message":{"role":"user","content":[
///       {"type":"tool_result","tool_use_id":"...","content":"...","is_error":false}
///   ]}}
/// `content` may be a string or an array of content blocks (text / image).
/// We flatten to a single string so the existing `agent-tool-result` event
/// can carry it.
fn translate_user(env: &Value) -> Vec<HarnessEvent> {
    let Some(content) = env.pointer("/message/content").and_then(Value::as_array) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for block in content {
        if block.get("type").and_then(Value::as_str) != Some("tool_result") {
            continue;
        }
        let Some(tool_use_id) = block.get("tool_use_id").and_then(Value::as_str) else {
            continue;
        };
        let is_error = block
            .get("is_error")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let content_str = flatten_tool_result_content(block.get("content"));
        out.push(HarnessEvent::ToolResult {
            tool_use_id: tool_use_id.to_string(),
            content: content_str,
            is_error,
        });
    }
    out
}

/// Tool-result content can be `"plain string"`, `[{"type":"text","text":"..."}]`,
/// or include images. We collapse to text so the existing tool-result card
/// can render it; image-bearing results land in chunk 3c.
fn flatten_tool_result_content(value: Option<&Value>) -> String {
    let Some(value) = value else { return String::new() };
    match value {
        Value::String(s) => s.clone(),
        Value::Array(parts) => {
            let mut buf = String::new();
            for part in parts {
                if let Some("text") = part.get("type").and_then(Value::as_str) {
                    if let Some(t) = part.get("text").and_then(Value::as_str) {
                        if !buf.is_empty() {
                            buf.push('\n');
                        }
                        buf.push_str(t);
                    }
                }
            }
            buf
        }
        // Object / number / bool / null → JSON-stringified.
        _ => serde_json::to_string(value).unwrap_or_default(),
    }
}

/// P0.8 fix #3: sum the `costUSD` field across every entry in Claude
/// Code's `result.modelUsage` map. Shape (per the SDK's coreSchemas):
///
/// ```json
/// "modelUsage": {
///   "claude-opus-4-7":   { "inputTokens": 12, "outputTokens": 8806,  "costUSD": 0.42, ... },
///   "claude-haiku-4-5":  { "inputTokens": 547, "outputTokens": 234, "costUSD": 0.001, ... }
/// }
/// ```
///
/// Returns `None` if the field is missing, not an object, or has no
/// positive-finite cost entries (so the caller falls through to local
/// recompute rather than reporting $0).
fn sum_model_usage_cost(model_usage: Option<&Value>) -> Option<f64> {
    let map = model_usage?.as_object()?;
    let mut total = 0.0_f64;
    let mut any = false;
    for (_model, entry) in map {
        if let Some(cost) = entry
            .get("costUSD")
            .and_then(Value::as_f64)
            .filter(|v| v.is_finite() && *v > 0.0)
        {
            total += cost;
            any = true;
        }
    }
    if any { Some(total) } else { None }
}

fn translate_result(env: &Value) -> Vec<HarnessEvent> {
    let usage = env.get("usage");
    let input_tokens = usage
        .and_then(|u| u.get("input_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0) as u32;
    let output_tokens = usage
        .and_then(|u| u.get("output_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0) as u32;
    let cache_read_tokens = usage
        .and_then(|u| u.get("cache_read_input_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0) as u32;
    let cache_write_tokens = usage
        .and_then(|u| u.get("cache_creation_input_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0) as u32;
    // P0.8: trust the CLI's own total when present. It already sums across
    // every model Claude Code used during the turn (e.g. Opus + Haiku in
    // auto-mode), and gets per-model rates right even when our local
    // model_registry would have lagged on a fresh release. We pass through
    // any non-zero value; the harness_runtime falls back to local recompute
    // only when this is `None`.
    //
    // P0.8 (fix #3): fall back to summing `result.modelUsage[*].costUSD`
    // when `total_cost_usd` is missing or filtered out (zero/NaN). The
    // CLI's stream-json `result` envelope ships `modelUsage` keyed by
    // model name with per-model `costUSD` — summing those gives the
    // same number as `total_cost_usd` would have, just spelled
    // differently, so we should still prefer it over local recompute
    // (which can't attribute Haiku auto-mode background tokens to the
    // right rate when only a single primary model is known).
    let cli_reported_cost_usd = env
        .get("total_cost_usd")
        .and_then(Value::as_f64)
        .filter(|v| v.is_finite() && *v > 0.0)
        .or_else(|| sum_model_usage_cost(env.get("modelUsage")));

    vec![
        HarnessEvent::Usage {
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_write_tokens,
            rate_limit: None,
            cli_reported_cost_usd,
        },
        HarnessEvent::TurnComplete,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn system_init_yields_session_ready_with_model_and_auth_mode() {
        let env = json!({
            "type": "system",
            "subtype": "init",
            "session_id": "abc-123",
            "model": "claude-opus-4-7",
            "apiKeySource": "ANTHROPIC_API_KEY",
        });
        let evs = translate_claude_envelope(&env);
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            HarnessEvent::SessionReady {
                session_id,
                model,
                auth_mode,
            } => {
                assert_eq!(session_id, "abc-123");
                assert_eq!(model.as_deref(), Some("claude-opus-4-7"));
                assert_eq!(auth_mode.as_deref(), Some("ANTHROPIC_API_KEY"));
            }
            other => panic!("expected SessionReady, got {other:?}"),
        }
    }

    #[test]
    fn system_init_without_model_or_auth_mode_still_yields_session_ready() {
        // Older Claude Code versions / Codex may omit one or both fields.
        // We still need the session ID for resume.
        let env = json!({
            "type": "system",
            "subtype": "init",
            "session_id": "abc-123",
        });
        let evs = translate_claude_envelope(&env);
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            HarnessEvent::SessionReady {
                session_id,
                model,
                auth_mode,
            } => {
                assert_eq!(session_id, "abc-123");
                assert!(model.is_none());
                assert!(auth_mode.is_none());
            }
            other => panic!("expected SessionReady, got {other:?}"),
        }
    }

    #[test]
    fn system_status_is_ignored_for_now() {
        let env = json!({"type": "system", "subtype": "status", "status": "active"});
        assert!(translate_claude_envelope(&env).is_empty());
    }

    #[test]
    fn stream_event_text_delta_yields_text_delta() {
        let env = json!({
            "type": "stream_event",
            "event": {
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "text_delta", "text": "Hel"},
            },
        });
        let evs = translate_claude_envelope(&env);
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            HarnessEvent::TextDelta { text } => assert_eq!(text, "Hel"),
            other => panic!("expected TextDelta, got {other:?}"),
        }
    }

    #[test]
    fn stream_event_thinking_delta_yields_thinking_delta() {
        let env = json!({
            "type": "stream_event",
            "event": {
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "thinking_delta", "thinking": "Considering..."},
            },
        });
        let evs = translate_claude_envelope(&env);
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            HarnessEvent::ThinkingDelta { text } => assert_eq!(text, "Considering..."),
            other => panic!("expected ThinkingDelta, got {other:?}"),
        }
    }

    #[test]
    fn stream_event_input_json_delta_is_skipped() {
        let env = json!({
            "type": "stream_event",
            "event": {
                "type": "content_block_delta",
                "index": 1,
                "delta": {"type": "input_json_delta", "partial_json": "{\"path"},
            },
        });
        // Skipped — assembled tool inputs come from the consolidated envelope.
        assert!(translate_claude_envelope(&env).is_empty());
    }

    #[test]
    fn assistant_consolidated_skips_text_blocks() {
        // Text was already streamed via stream_event. Skipping here prevents
        // a duplicate render at the end of every turn.
        let env = json!({
            "type": "assistant",
            "message": {
                "content": [
                    {"type": "text", "text": "Hello, world."},
                ],
            },
        });
        assert!(translate_claude_envelope(&env).is_empty());
    }

    #[test]
    fn assistant_consolidated_emits_tool_use() {
        let env = json!({
            "type": "assistant",
            "message": {
                "content": [
                    {"type": "text", "text": "Reading file..."},
                    {
                        "type": "tool_use",
                        "id": "toolu_01",
                        "name": "Read",
                        "input": {"file_path": "/etc/hosts"},
                    },
                ],
            },
        });
        let evs = translate_claude_envelope(&env);
        assert_eq!(evs.len(), 1, "text block should be skipped, tool_use kept");
        match &evs[0] {
            HarnessEvent::ToolUse {
                tool_use_id,
                name,
                input,
                diff_payload,
            } => {
                assert_eq!(tool_use_id, "toolu_01");
                assert_eq!(name, "Read");
                assert_eq!(input["file_path"], "/etc/hosts");
                assert!(diff_payload.is_none());
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
    }

    #[test]
    fn user_envelope_emits_tool_result_with_string_content() {
        let env = json!({
            "type": "user",
            "message": {
                "role": "user",
                "content": [
                    {
                        "type": "tool_result",
                        "tool_use_id": "toolu_01",
                        "content": "127.0.0.1 localhost",
                        "is_error": false,
                    },
                ],
            },
        });
        let evs = translate_claude_envelope(&env);
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            HarnessEvent::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                assert_eq!(tool_use_id, "toolu_01");
                assert_eq!(content, "127.0.0.1 localhost");
                assert!(!is_error);
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }
    }

    #[test]
    fn user_envelope_emits_tool_result_flattening_array_content() {
        let env = json!({
            "type": "user",
            "message": {
                "role": "user",
                "content": [
                    {
                        "type": "tool_result",
                        "tool_use_id": "toolu_02",
                        "content": [
                            {"type": "text", "text": "line one"},
                            {"type": "text", "text": "line two"},
                        ],
                        "is_error": true,
                    },
                ],
            },
        });
        let evs = translate_claude_envelope(&env);
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            HarnessEvent::ToolResult {
                content, is_error, ..
            } => {
                assert_eq!(content, "line one\nline two");
                assert!(*is_error);
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }
    }

    #[test]
    fn result_envelope_emits_usage_and_turn_complete() {
        // total_cost_usd: 0.0 → cli_reported_cost_usd should be None (we
        // only pass through positive finite values, since some envelopes
        // emit 0.0 as a placeholder).
        let env = json!({
            "type": "result",
            "usage": {
                "input_tokens": 100,
                "output_tokens": 50,
                "cache_read_input_tokens": 10,
                "cache_creation_input_tokens": 5,
            },
            "total_cost_usd": 0.0,
        });
        let evs = translate_claude_envelope(&env);
        assert_eq!(evs.len(), 2);
        match &evs[0] {
            HarnessEvent::Usage {
                input_tokens,
                output_tokens,
                cache_read_tokens,
                cache_write_tokens,
                cli_reported_cost_usd,
                ..
            } => {
                assert_eq!(*input_tokens, 100);
                assert_eq!(*output_tokens, 50);
                assert_eq!(*cache_read_tokens, 10);
                assert_eq!(*cache_write_tokens, 5);
                assert!(cli_reported_cost_usd.is_none());
            }
            other => panic!("expected Usage, got {other:?}"),
        }
        assert!(matches!(evs[1], HarnessEvent::TurnComplete));
    }

    #[test]
    fn result_envelope_passes_through_positive_cli_cost() {
        // P0.8: when Claude Code reports a real total_cost_usd, the host
        // must prefer it over local recompute (which only knew about the
        // user-picked model and would miss multi-model auto-mode work).
        let env = json!({
            "type": "result",
            "usage": {
                "input_tokens": 100,
                "output_tokens": 50,
                "cache_read_input_tokens": 0,
                "cache_creation_input_tokens": 0,
            },
            "total_cost_usd": 0.4523,
        });
        let evs = translate_claude_envelope(&env);
        assert_eq!(evs.len(), 2);
        match &evs[0] {
            HarnessEvent::Usage {
                cli_reported_cost_usd,
                ..
            } => assert_eq!(*cli_reported_cost_usd, Some(0.4523)),
            other => panic!("expected Usage, got {other:?}"),
        }
    }

    #[test]
    fn result_envelope_falls_back_to_modelusage_sum_when_total_cost_missing() {
        // P0.8 fix #3: when `total_cost_usd` is absent (or zero/NaN — both
        // filtered out by the same finite-positive predicate), the result
        // envelope should fall back to summing `modelUsage[*].costUSD`.
        // Hits the multi-model auto-mode case where the primary model is
        // Opus but Haiku also ran background work.
        let env = json!({
            "type": "result",
            "usage": {
                "input_tokens": 100,
                "output_tokens": 50,
                "cache_read_input_tokens": 0,
                "cache_creation_input_tokens": 0,
            },
            "modelUsage": {
                "claude-opus-4-7":  { "inputTokens": 12, "outputTokens": 8806, "costUSD": 0.42,  "cacheReadInputTokens": 0, "cacheCreationInputTokens": 0, "webSearchRequests": 0, "contextWindow": 200000, "maxOutputTokens": 8192 },
                "claude-haiku-4-5": { "inputTokens": 547, "outputTokens": 234, "costUSD": 0.001, "cacheReadInputTokens": 0, "cacheCreationInputTokens": 0, "webSearchRequests": 0, "contextWindow": 200000, "maxOutputTokens": 8192 }
            }
        });
        let evs = translate_claude_envelope(&env);
        assert_eq!(evs.len(), 2);
        match &evs[0] {
            HarnessEvent::Usage { cli_reported_cost_usd, .. } => {
                let c = cli_reported_cost_usd.expect("expected summed modelUsage cost");
                assert!((c - 0.421).abs() < 1e-9, "expected ~0.421, got {c}");
            }
            other => panic!("expected Usage, got {other:?}"),
        }
    }

    #[test]
    fn result_envelope_prefers_total_cost_over_modelusage_when_both_present() {
        // When the CLI ships both, the top-level total wins (it's already
        // summed and may include rounding adjustments modelUsage misses).
        let env = json!({
            "type": "result",
            "usage": { "input_tokens": 1, "output_tokens": 1, "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0 },
            "total_cost_usd": 0.50,
            "modelUsage": {
                "claude-opus-4-7": { "inputTokens": 0, "outputTokens": 0, "costUSD": 0.42, "cacheReadInputTokens": 0, "cacheCreationInputTokens": 0, "webSearchRequests": 0, "contextWindow": 200000, "maxOutputTokens": 8192 }
            }
        });
        let evs = translate_claude_envelope(&env);
        match &evs[0] {
            HarnessEvent::Usage { cli_reported_cost_usd, .. } => {
                assert_eq!(*cli_reported_cost_usd, Some(0.50));
            }
            other => panic!("expected Usage, got {other:?}"),
        }
    }

    #[test]
    fn unknown_envelope_type_non_interactive_is_ignored() {
        // Telemetry-style envelopes with no request_id / question / prompt /
        // message should still be silently dropped — the catch-all only
        // fires for envelopes that look like they want a user response.
        let env = json!({"type": "something_new", "data": 42});
        assert!(translate_claude_envelope(&env).is_empty());
    }

    #[test]
    fn unknown_envelope_with_request_id_becomes_unknown_prompt() {
        // P0.9: a fresh envelope shape we've never heard of but that carries
        // a request_id is almost certainly waiting on a user response. We
        // surface it as UnknownPrompt so the user sees a dialog instead of
        // the agent hanging.
        let env = json!({
            "type": "future_interactive_type",
            "request_id": "req_42",
            "prompt": "Pick one",
        });
        let evs = translate_claude_envelope(&env);
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            HarnessEvent::UnknownPrompt { envelope_type, request_id, summary, .. } => {
                assert_eq!(envelope_type, "future_interactive_type");
                assert_eq!(request_id.as_deref(), Some("req_42"));
                assert!(summary.contains("Pick one") || !summary.is_empty());
            }
            other => panic!("expected UnknownPrompt, got {other:?}"),
        }
    }

    #[test]
    fn missing_type_is_ignored() {
        let env = json!({"foo": "bar"});
        assert!(translate_claude_envelope(&env).is_empty());
    }

    #[test]
    fn control_request_can_use_tool_yields_permission_request() {
        let env = json!({
            "type": "control_request",
            "request_id": "req_42",
            "request": {
                "subtype": "can_use_tool",
                "tool_name": "Bash",
                "input": {"command": "rm -rf /tmp/x"},
                "tool_use_id": "toolu_99",
            },
        });
        let evs = translate_claude_envelope(&env);
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            HarnessEvent::PermissionRequest {
                request_id,
                tool_use_id,
                tool_name,
                input,
            } => {
                assert_eq!(request_id, "req_42");
                assert_eq!(tool_use_id.as_deref(), Some("toolu_99"));
                assert_eq!(tool_name, "Bash");
                assert_eq!(input["command"], "rm -rf /tmp/x");
            }
            other => panic!("expected PermissionRequest, got {other:?}"),
        }
    }

    #[test]
    fn control_request_elicitation_yields_typed_mcp_elicit() {
        // P0.9 fix #8: `elicitation` is now a TYPED variant — the
        // translator emits McpElicit so the frontend can render a
        // schema-driven form instead of the generic UnknownPrompt
        // fallback. Other unrecognised subtypes still fall through
        // to UnknownPrompt — see `control_request_unhandled_subtype_*`.
        let env = json!({
            "type": "control_request",
            "request_id": "req_x",
            "request": {"subtype": "elicitation", "mcp_server_name": "foo", "message": "Pick one", "schema": {"type": "object"}},
        });
        let evs = translate_claude_envelope(&env);
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            HarnessEvent::McpElicit { request_id, message, schema } => {
                assert_eq!(request_id, "req_x");
                assert_eq!(message, "Pick one");
                assert!(schema.is_object());
            }
            other => panic!("expected McpElicit, got {other:?}"),
        }
    }

    #[test]
    fn control_request_unhandled_subtype_surfaces_as_unknown_prompt() {
        // P0.9 catch-all: subtypes without a typed handler still go to
        // UnknownPrompt so the agent doesn't hang forever waiting on
        // input that never arrives. Uses `hook_callback` as the
        // concrete unhandled subtype (elicitation is now typed — see
        // `control_request_elicitation_yields_typed_mcp_elicit`).
        let env = json!({
            "type": "control_request",
            "request_id": "req_x",
            "request": {"subtype": "hook_callback", "hook_name": "foo", "message": "Pick one"},
        });
        let evs = translate_claude_envelope(&env);
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            HarnessEvent::UnknownPrompt {
                envelope_type,
                request_id,
                ..
            } => {
                assert!(envelope_type.contains("hook_callback"));
                assert_eq!(request_id.as_deref(), Some("req_x"));
            }
            other => panic!("expected UnknownPrompt, got {other:?}"),
        }
    }

    #[test]
    fn control_request_exit_plan_mode_yields_typed_approval() {
        // P0.9 fix #8: the `exit_plan_mode` tool gets routed to the
        // typed ApprovalRequest variant so the frontend renders a
        // plan-review card rather than the generic permission row.
        let env = json!({
            "type": "control_request",
            "request_id": "req_42",
            "request": {
                "subtype": "can_use_tool",
                "tool_name": "exit_plan_mode",
                "tool_use_id": "toolu_abc",
                "input": {"plan": "Step 1: rename foo to bar"}
            },
        });
        let evs = translate_claude_envelope(&env);
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            HarnessEvent::ApprovalRequest { request_id, kind, payload, tool_use_id } => {
                assert_eq!(request_id, "req_42");
                assert_eq!(kind, "exit_plan_mode");
                assert_eq!(tool_use_id.as_deref(), Some("toolu_abc"));
                assert_eq!(payload["plan"], "Step 1: rename foo to bar");
            }
            other => panic!("expected ApprovalRequest, got {other:?}"),
        }
    }

    #[test]
    fn control_request_missing_tool_name_is_ignored() {
        let env = json!({
            "type": "control_request",
            "request_id": "req_x",
            "request": {"subtype": "can_use_tool", "input": {}},
        });
        assert!(translate_claude_envelope(&env).is_empty());
    }
}

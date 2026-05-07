//! Translate Claude Code's stream-json envelopes into `HarnessEvent`.
//!
//! The CLI's NDJSON output uses one envelope per logical message. Top-level
//! shapes we handle:
//!
//! * `{"type":"system","subtype":"init","session_id":"..."}` — fired once
//!   right after spawn. We extract `session_id` so we can persist it for
//!   `--resume` later.
//!
//! * `{"type":"stream_event","event":{...}}` — only emitted when the CLI is
//!   spawned with `--include-partial-messages`. The wrapped `event` mirrors
//!   the Anthropic API SSE event shape: `content_block_start`,
//!   `content_block_delta`, `content_block_stop`, etc. We use these for
//!   character-by-character text streaming and for thinking deltas.
//!
//! * `{"type":"assistant","message":{"content":[...]}}` — consolidated
//!   assistant turn message. Because we always pass
//!   `--include-partial-messages`, the **text** content blocks were already
//!   streamed via `stream_event`, so we ignore them here to avoid
//!   double-render. We *do* read `tool_use` blocks here, because assembling
//!   them from the partial `input_json_delta` stream requires per-block JSON
//!   buffering that's not worth the complexity for chunk 3a.
//!
//! * `{"type":"user","message":{"content":[{"type":"tool_result", ...}]}}` —
//!   the CLI echoes back tool results in this shape. Translated to
//!   `HarnessEvent::ToolResult` so the chat shows what the tool returned.
//!
//! * `{"type":"result","usage":{...}}` — turn end. Token usage + the
//!   `TurnComplete` marker.
//!
//! Permission requests (`{"type":"permission_request", ...}`) and
//! diff-payload construction land in chunks 3b / 3c respectively.

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
        // Telemetry-only kinds (`tool_progress`, `auth_status`, ...) — ignore.
        other => {
            tracing::debug!(envelope_type = other, "harness/claude: ignoring envelope");
            Vec::new()
        }
    }
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

    if env.get("request").and_then(|r| r.get("subtype")).and_then(Value::as_str) != Some("can_use_tool") {
        return Vec::new();
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

    vec![HarnessEvent::PermissionRequest {
        request_id: request_id.to_string(),
        tool_use_id,
        tool_name: tool_name.to_string(),
        input,
    }]
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
    vec![HarnessEvent::SessionReady {
        session_id: session_id.to_string(),
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

    vec![
        HarnessEvent::Usage {
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_write_tokens,
            rate_limit: None,
        },
        HarnessEvent::TurnComplete,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn system_init_yields_session_ready() {
        let env = json!({
            "type": "system",
            "subtype": "init",
            "session_id": "abc-123",
            "model": "claude-sonnet-4-5",
        });
        let evs = translate_claude_envelope(&env);
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            HarnessEvent::SessionReady { session_id } => assert_eq!(session_id, "abc-123"),
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
                ..
            } => {
                assert_eq!(*input_tokens, 100);
                assert_eq!(*output_tokens, 50);
                assert_eq!(*cache_read_tokens, 10);
                assert_eq!(*cache_write_tokens, 5);
            }
            other => panic!("expected Usage, got {other:?}"),
        }
        assert!(matches!(evs[1], HarnessEvent::TurnComplete));
    }

    #[test]
    fn unknown_envelope_type_is_ignored() {
        let env = json!({"type": "something_new", "data": 42});
        assert!(translate_claude_envelope(&env).is_empty());
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
    fn control_request_other_subtype_is_ignored() {
        // E.g. the CLI's response to our (hypothetical) initialize request.
        let env = json!({
            "type": "control_request",
            "request_id": "req_x",
            "request": {"subtype": "set_permission_mode", "mode": "ifNeeded"},
        });
        assert!(translate_claude_envelope(&env).is_empty());
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

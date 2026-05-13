//! Translate Codex JSON-RPC notifications into `HarnessEvent`s.
//!
//! Schema reference: `docs/codex-schema/v2/*.json`. Codex CLI 0.125.x.
//!
//! # Notification taxonomy
//!
//! The Codex `app-server` emits a flat stream of JSON-RPC notifications
//! per turn. The ones that matter for the chat UI are:
//!
//! | Method | Translates to | Notes |
//! |---|---|---|
//! | `thread/started` | (no event — `SessionReady` is fired by the spawn handshake from the response) | Echoed for any subsequent listeners. |
//! | `item/started` | `ToolUse` (when item is `commandExecution` / `fileChange` / `mcpToolCall` / `dynamicToolCall`) | id+name+input shape mirrors Anthropic content blocks. |
//! | `item/agentMessage/delta` | `TextDelta` | Streaming assistant text. |
//! | `item/reasoning/textDelta` | `ThinkingDelta` | Reasoning stream. |
//! | `item/reasoning/summaryTextDelta` | `ThinkingDelta` | Same channel as above for summary text. |
//! | `item/completed` | `ToolResult` (for tool-shaped items) | Carries final status + output. |
//! | `turn/started` | (no event) | Idle-clock bump only. |
//! | `turn/completed` | `Usage` + `TurnComplete` | Token totals come from `tokenUsage` if present. |
//! | `thread/tokenUsage/updated` | `Usage` | Mid-turn token accounting. |
//! | `error` | `Error` | Server reported a runtime error. |
//!
//! Anything else is silently ignored — these are fine to drop because the
//! host runtime only cares about the high-signal events above. We log at
//! `debug` so unexpected new methods are still discoverable in tracing.

use crate::harness::{DiffPayload, HarnessEvent};
use serde_json::Value;

/// Decode one Codex notification (`method` + `params`) and return zero or
/// more `HarnessEvent`s for the host runtime.
pub fn translate_codex_notification(method: &str, params: &Value) -> Vec<HarnessEvent> {
    match method {
        "item/agentMessage/delta" => agent_message_delta(params),
        "item/reasoning/textDelta" | "item/reasoning/summaryTextDelta" => {
            reasoning_delta(params)
        }
        "item/started" => item_started(params),
        "item/completed" => item_completed(params),
        "turn/completed" => turn_completed(params),
        "thread/tokenUsage/updated" => token_usage_updated(params),
        "error" => error_notification(params),
        // thread/started is informational — SessionReady is already fired
        // from the `thread/start` response in CodexSession::spawn, so we
        // don't fire a duplicate here.
        "thread/started"
        | "thread/closed"
        | "thread/status/changed"
        | "thread/name/updated"
        | "turn/started"
        | "turn/diff/updated"
        | "account/updated"
        | "account/rateLimits/updated"
        | "fs/changed" => Vec::new(),
        // P0.9: any other method may be an interactive request the audit
        // identified (item/tool/requestUserInput, item/*/requestApproval,
        // mcpServer/elicitation/request, mcpServer/oauth/login, ...). The
        // catch-all in event_map promotes anything that *looks* interactive
        // to a visible UnknownPrompt event so the user can respond, instead
        // of letting the CLI wait forever on a silent drop.
        other => crate::harness::event_map::translate_unknown_envelope("codex", other, params),
    }
}

/// `item/agentMessage/delta` → streaming assistant text.
/// Schema: `{ itemId, delta }` per AgentMessageDeltaNotification.
fn agent_message_delta(params: &Value) -> Vec<HarnessEvent> {
    let text = params
        .get("delta")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
    match text {
        Some(t) => vec![HarnessEvent::TextDelta {
            text: t.to_string(),
        }],
        None => Vec::new(),
    }
}

/// `item/reasoning/textDelta` / `summaryTextDelta` → thinking stream.
/// Schema: `{ itemId, delta }` per ReasoningTextDeltaNotification.
fn reasoning_delta(params: &Value) -> Vec<HarnessEvent> {
    let text = params
        .get("delta")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
    match text {
        Some(t) => vec![HarnessEvent::ThinkingDelta {
            text: t.to_string(),
        }],
        None => Vec::new(),
    }
}

/// `item/started` → tool-use card. Codex distinguishes by `item.type`:
/// `commandExecution`, `fileChange`, `mcpToolCall`, `dynamicToolCall`,
/// `webSearch`, etc. Per ThreadItem schema (docs/codex-schema/v2/...).
///
/// We only fire `HarnessEvent::ToolUse` for the kinds Rustic's existing
/// tool-card UI knows how to render. Other kinds (reasoning, agentMessage,
/// userMessage) are lifecycle-only — they're tracked via the dedicated
/// delta streams above and don't need a card.
fn item_started(params: &Value) -> Vec<HarnessEvent> {
    let item = match params.get("item") {
        Some(i) => i,
        None => return Vec::new(),
    };
    let kind = item.get("type").and_then(Value::as_str).unwrap_or("");
    let id = item
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if id.is_empty() {
        return Vec::new();
    }

    match kind {
        "commandExecution" => {
            let command = item
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let cwd = item
                .get("cwd")
                .and_then(Value::as_str)
                .map(String::from);
            let mut input = serde_json::json!({ "command": command });
            if let Some(c) = cwd {
                input["cwd"] = Value::String(c);
            }
            vec![HarnessEvent::ToolUse {
                tool_use_id: id,
                name: "Bash".into(), // Map onto Rustic's existing Bash card.
                input,
                diff_payload: None,
            }]
        }
        "fileChange" => {
            let changes = item
                .get("changes")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            // Surface the full changes array as input so the chat-view can
            // render it. Diff card support for Codex's PatchChangeKind is
            // a follow-up in the same line as Claude Code's Edit/Write
            // diff path (chunk 3c).
            let input = serde_json::json!({ "changes": changes });
            vec![HarnessEvent::ToolUse {
                tool_use_id: id,
                name: "Edit".into(),
                input,
                diff_payload: None,
            }]
        }
        "mcpToolCall" => {
            let server = item
                .get("server")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let tool = item
                .get("tool")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let arguments = item.get("arguments").cloned().unwrap_or(Value::Null);
            vec![HarnessEvent::ToolUse {
                tool_use_id: id,
                name: format!("mcp:{server}/{tool}"),
                input: arguments,
                diff_payload: None,
            }]
        }
        "dynamicToolCall" => {
            let tool = item
                .get("tool")
                .and_then(Value::as_str)
                .unwrap_or("Tool")
                .to_string();
            let arguments = item.get("arguments").cloned().unwrap_or(Value::Null);
            vec![HarnessEvent::ToolUse {
                tool_use_id: id,
                name: tool,
                input: arguments,
                diff_payload: None,
            }]
        }
        "webSearch" => {
            let query = item
                .get("query")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            vec![HarnessEvent::ToolUse {
                tool_use_id: id,
                name: "WebSearch".into(),
                input: serde_json::json!({ "query": query }),
                diff_payload: None,
            }]
        }
        // userMessage / agentMessage / reasoning / etc. → no card needed.
        _ => Vec::new(),
    }
}

/// `item/completed` → tool result (for tool-shaped items only).
/// Schema: `{ item }` where `item.status` indicates success/failure.
fn item_completed(params: &Value) -> Vec<HarnessEvent> {
    let item = match params.get("item") {
        Some(i) => i,
        None => return Vec::new(),
    };
    let kind = item.get("type").and_then(Value::as_str).unwrap_or("");
    let id = item
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if id.is_empty() {
        return Vec::new();
    }

    let status = item.get("status").and_then(Value::as_str).unwrap_or("");
    let is_error = matches!(status, "failed" | "declined");

    match kind {
        "commandExecution" => {
            let output = item
                .get("aggregatedOutput")
                .and_then(Value::as_str)
                .map(String::from)
                .or_else(|| {
                    item.get("exitCode")
                        .and_then(Value::as_i64)
                        .map(|c| format!("(exit code {c})"))
                })
                .unwrap_or_default();
            vec![HarnessEvent::ToolResult {
                tool_use_id: id,
                content: output,
                is_error,
            }]
        }
        "fileChange" => {
            let summary = summarise_file_changes(item);
            vec![HarnessEvent::ToolResult {
                tool_use_id: id,
                content: summary,
                is_error,
            }]
        }
        "mcpToolCall" | "dynamicToolCall" => {
            // Both ride a generic `result` blob; stringify for display.
            let content = item
                .get("result")
                .or_else(|| item.get("error"))
                .map(|v| serde_json::to_string(v).unwrap_or_default())
                .unwrap_or_default();
            vec![HarnessEvent::ToolResult {
                tool_use_id: id,
                content,
                is_error,
            }]
        }
        "webSearch" => {
            // Keep the action object for the UI; flatten to JSON.
            let content = item
                .get("action")
                .map(|v| serde_json::to_string(v).unwrap_or_default())
                .unwrap_or_default();
            vec![HarnessEvent::ToolResult {
                tool_use_id: id,
                content,
                is_error,
            }]
        }
        _ => Vec::new(),
    }
}

/// `turn/completed` → token usage + TurnComplete. Schema:
/// `{ turn: { id, status, startedAt, completedAt, durationMs, ...} }`
/// plus an outer `tokenUsage` field on some versions. The breakdown lives
/// under `tokenUsage.last` (per-turn delta) per `ThreadTokenUsage` —
/// reading the parent object directly returns 0 for every counter.
fn turn_completed(params: &Value) -> Vec<HarnessEvent> {
    let mut out = Vec::new();
    if let Some(usage) = params.get("tokenUsage").or_else(|| params.get("usage")) {
        let breakdown = usage.get("last").unwrap_or(usage);
        out.push(HarnessEvent::Usage {
            input_tokens: usage_field(breakdown, "inputTokens"),
            output_tokens: usage_field(breakdown, "outputTokens"),
            cache_read_tokens: usage_field(breakdown, "cachedInputTokens"),
            cache_write_tokens: 0,
            rate_limit: None,
            // P0.8: Codex's tokenUsage doesn't ship a cost field; the host
            // runtime falls back to local recompute (the user-picked Codex
            // model is the only one Codex uses, so single-model recompute
            // is accurate here).
            cli_reported_cost_usd: None,
        });
    }
    out.push(HarnessEvent::TurnComplete);
    out
}

/// `thread/tokenUsage/updated` → Usage. Mid-turn accounting; emits without
/// TurnComplete so the cost pill advances live. Reads the per-turn `last`
/// breakdown (host runtime accumulates into a cumulative total separately).
fn token_usage_updated(params: &Value) -> Vec<HarnessEvent> {
    let usage = match params.get("tokenUsage").or_else(|| params.get("usage")) {
        Some(u) => u,
        None => return Vec::new(),
    };
    let breakdown = usage.get("last").unwrap_or(usage);
    vec![HarnessEvent::Usage {
        input_tokens: usage_field(breakdown, "inputTokens"),
        output_tokens: usage_field(breakdown, "outputTokens"),
        cache_read_tokens: usage_field(breakdown, "cachedInputTokens"),
        cache_write_tokens: 0,
        rate_limit: None,
        cli_reported_cost_usd: None,
    }]
}

/// `error` notification → fatal. Schema:
/// `ErrorNotification { message, codexErrorInfo? }`.
fn error_notification(params: &Value) -> Vec<HarnessEvent> {
    let message = params
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("Codex reported an error")
        .to_string();
    vec![HarnessEvent::Error { message }]
}

fn usage_field(usage: &Value, key: &str) -> u32 {
    usage.get(key).and_then(Value::as_u64).unwrap_or(0) as u32
}

/// Build a one-line summary from a fileChange item for the tool-result
/// card. Counts adds/updates/deletes from the changes array.
fn summarise_file_changes(item: &Value) -> String {
    let changes = match item.get("changes").and_then(Value::as_array) {
        Some(c) => c,
        None => return String::new(),
    };
    let (mut added, mut updated, mut deleted) = (0u32, 0u32, 0u32);
    for change in changes {
        let kind = change.pointer("/kind/type").and_then(Value::as_str);
        match kind {
            Some("add") => added += 1,
            Some("update") => updated += 1,
            Some("delete") => deleted += 1,
            _ => {}
        }
    }
    format!(
        "{} file(s) changed: +{} added, ~{} updated, -{} deleted",
        added + updated + deleted,
        added,
        updated,
        deleted
    )
}

// Suppress "unused import" warning when DiffPayload becomes useful in
// follow-up (real diff rendering for fileChange items).
#[allow(dead_code)]
fn _hint_diff_payload(_: DiffPayload) {}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn agent_message_delta_yields_text_delta() {
        let evs = translate_codex_notification(
            "item/agentMessage/delta",
            &json!({ "itemId": "i1", "delta": "Hello" }),
        );
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            HarnessEvent::TextDelta { text } => assert_eq!(text, "Hello"),
            other => panic!("expected TextDelta, got {other:?}"),
        }
    }

    #[test]
    fn reasoning_delta_yields_thinking_delta() {
        let evs = translate_codex_notification(
            "item/reasoning/textDelta",
            &json!({ "itemId": "i2", "delta": "thinking..." }),
        );
        assert_eq!(evs.len(), 1);
        assert!(matches!(evs[0], HarnessEvent::ThinkingDelta { .. }));
    }

    #[test]
    fn item_started_command_execution_yields_bash_tool_use() {
        let evs = translate_codex_notification(
            "item/started",
            &json!({
                "item": {
                    "type": "commandExecution",
                    "id": "ce_1",
                    "command": "ls -la",
                    "cwd": "/tmp",
                    "status": "inProgress",
                    "commandActions": [],
                }
            }),
        );
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            HarnessEvent::ToolUse {
                tool_use_id,
                name,
                input,
                ..
            } => {
                assert_eq!(tool_use_id, "ce_1");
                assert_eq!(name, "Bash");
                assert_eq!(input["command"], "ls -la");
                assert_eq!(input["cwd"], "/tmp");
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
    }

    #[test]
    fn item_started_file_change_yields_edit_tool_use() {
        let evs = translate_codex_notification(
            "item/started",
            &json!({
                "item": {
                    "type": "fileChange",
                    "id": "fc_1",
                    "status": "inProgress",
                    "changes": [
                        { "path": "a.rs", "diff": "+ x", "kind": { "type": "update" } }
                    ]
                }
            }),
        );
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            HarnessEvent::ToolUse { name, .. } => assert_eq!(name, "Edit"),
            other => panic!("expected ToolUse, got {other:?}"),
        }
    }

    #[test]
    fn item_completed_command_uses_aggregated_output() {
        let evs = translate_codex_notification(
            "item/completed",
            &json!({
                "item": {
                    "type": "commandExecution",
                    "id": "ce_1",
                    "status": "completed",
                    "aggregatedOutput": "total 0\n",
                    "exitCode": 0,
                }
            }),
        );
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            HarnessEvent::ToolResult { content, is_error, .. } => {
                assert_eq!(content, "total 0\n");
                assert!(!is_error);
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }
    }

    #[test]
    fn turn_completed_emits_usage_and_marker() {
        let evs = translate_codex_notification(
            "turn/completed",
            &json!({
                "tokenUsage": {
                    "inputTokens": 100,
                    "outputTokens": 50,
                    "cachedInputTokens": 10
                }
            }),
        );
        assert_eq!(evs.len(), 2);
        match &evs[0] {
            HarnessEvent::Usage {
                input_tokens,
                output_tokens,
                cache_read_tokens,
                ..
            } => {
                assert_eq!(*input_tokens, 100);
                assert_eq!(*output_tokens, 50);
                assert_eq!(*cache_read_tokens, 10);
            }
            other => panic!("expected Usage, got {other:?}"),
        }
        assert!(matches!(evs[1], HarnessEvent::TurnComplete));
    }

    /// Codex's canonical `ThreadTokenUsage` nests the breakdown under
    /// `last`/`total`. Reading the parent object would return zeros, so the
    /// translator must descend into `last` (per-turn delta).
    #[test]
    fn turn_completed_reads_tokenusage_last_breakdown() {
        let evs = translate_codex_notification(
            "turn/completed",
            &json!({
                "tokenUsage": {
                    "last":  { "inputTokens": 200, "outputTokens": 80, "cachedInputTokens": 20, "reasoningOutputTokens": 0, "totalTokens": 300 },
                    "total": { "inputTokens": 999, "outputTokens": 999, "cachedInputTokens": 0, "reasoningOutputTokens": 0, "totalTokens": 1998 }
                }
            }),
        );
        assert_eq!(evs.len(), 2);
        match &evs[0] {
            HarnessEvent::Usage { input_tokens, output_tokens, cache_read_tokens, .. } => {
                assert_eq!(*input_tokens, 200);
                assert_eq!(*output_tokens, 80);
                assert_eq!(*cache_read_tokens, 20);
            }
            other => panic!("expected Usage, got {other:?}"),
        }
    }

    #[test]
    fn error_notification_yields_error_event() {
        let evs = translate_codex_notification(
            "error",
            &json!({ "message": "rate limited" }),
        );
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            HarnessEvent::Error { message } => assert_eq!(message, "rate limited"),
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn unknown_method_is_ignored() {
        assert!(
            translate_codex_notification("totally/new/method", &json!({}))
                .is_empty()
        );
    }
}

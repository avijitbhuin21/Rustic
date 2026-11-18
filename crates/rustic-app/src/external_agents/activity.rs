//! Activity log for external CLI agent sessions.
//!
//! The hook payloads the CLIs hand to the spool sink (see [`super::hooks`]) say
//! what the agent is doing: the prompt it was given, each tool call, when it is
//! waiting for an approval, and when a turn ends. This module condenses those
//! payloads into small display rows and keeps them in one JSONL file per
//! session, which is what the activity list beside a CLI terminal renders.
//!
//! It is read-only towards the CLI: nothing here is ever sent back to it.
//!
//! Two payload dialects are understood. Claude Code and Codex use snake_case
//! (`hook_event_name`, `tool_name`, `tool_input`); Antigravity uses camelCase
//! (`hookEventName`, `toolCall.{name,args,id}`).
//!
//! Only a short summary of a tool call is kept (the command, the path, the
//! pattern). File contents and tool output are never copied out of the payload.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Longest summary / prompt text kept per row.
const MAX_TEXT: usize = 600;
/// Most file paths kept per row.
const MAX_PATHS: usize = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActivityKind {
    /// The user submitted a prompt.
    Prompt,
    /// A tool call started or finished (see [`ActivityEvent::phase`]).
    Tool,
    /// The CLI is asking the user to approve a tool call.
    Approval,
    /// The turn ended.
    Stop,
}

/// One row of a session's activity list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityEvent {
    /// Unix time in milliseconds at which Rustic folded the event in.
    pub ts: u64,
    pub kind: ActivityKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// The CLI's id for a tool call, shared by its start and end events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
    /// `"start"` or `"end"` for tool events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub error: bool,
}

/// Current unix time in milliseconds.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Turn one raw hook payload into an activity row, or `None` when the event is
/// not something the list shows.
pub fn normalize(raw: &Value, ts: u64) -> Option<ActivityEvent> {
    let event = str_field(raw, &["hook_event_name", "hookEventName"])?;
    let blank = |kind| ActivityEvent {
        ts,
        kind,
        tool: None,
        call_id: None,
        phase: None,
        text: None,
        paths: Vec::new(),
        error: false,
    };

    match event {
        "UserPromptSubmit" => {
            let prompt = str_field(raw, &["prompt"])?;
            Some(ActivityEvent {
                text: Some(clip(prompt)),
                ..blank(ActivityKind::Prompt)
            })
        }
        // Antigravity has no prompt event; the first model call of a turn
        // carries the prompt that started it.
        "PreInvocation" => {
            if raw.get("invocationNum").and_then(Value::as_u64) != Some(0) {
                return None;
            }
            let prompt = str_field(raw, &["lastUserInput"])?;
            Some(ActivityEvent {
                text: Some(clip(prompt)),
                ..blank(ActivityKind::Prompt)
            })
        }
        "PreToolUse" | "PostToolUse" | "PostToolUseFailure" | "PermissionRequest" => {
            let (tool, input, call_id) = tool_call(raw);
            let failed = event == "PostToolUseFailure"
                || str_field(raw, &["error"]).is_some_and(|e| !e.trim().is_empty());
            let (kind, phase) = match event {
                "PermissionRequest" => (ActivityKind::Approval, None),
                "PreToolUse" => (ActivityKind::Tool, Some("start")),
                _ => (ActivityKind::Tool, Some("end")),
            };
            Some(ActivityEvent {
                tool,
                call_id,
                phase: phase.map(str::to_string),
                text: input.and_then(summarize),
                paths: input.map(paths_in).unwrap_or_default(),
                error: failed,
                ..blank(kind)
            })
        }
        "Stop" => Some(ActivityEvent {
            text: str_field(raw, &["last_assistant_message"]).map(clip),
            error: str_field(raw, &["error"]).is_some_and(|e| !e.trim().is_empty()),
            ..blank(ActivityKind::Stop)
        }),
        _ => None,
    }
}

/// Tool name, input object and call id, in whichever dialect the payload uses.
fn tool_call(raw: &Value) -> (Option<String>, Option<&Value>, Option<String>) {
    if let Some(call) = raw.get("toolCall") {
        return (
            str_field(call, &["name"]).map(str::to_string),
            call.get("args"),
            str_field(call, &["id"]).map(str::to_string),
        );
    }
    (
        str_field(raw, &["tool_name"]).map(str::to_string),
        raw.get("tool_input"),
        str_field(raw, &["tool_use_id"]).map(str::to_string),
    )
}

/// The one line that best describes a tool call's input.
fn summarize(input: &Value) -> Option<String> {
    const KEYS: [&str; 14] = [
        "command",
        "CommandLine",
        "file_path",
        "notebook_path",
        "Path",
        "path",
        "AbsolutePath",
        "pattern",
        "Query",
        "query",
        "url",
        "Url",
        "description",
        "prompt",
    ];
    if let Some(text) = KEYS
        .iter()
        .find_map(|k| input.get(k).and_then(Value::as_str))
    {
        return Some(clip(text));
    }
    // Claude Code's ask-user tool: show the first question.
    input
        .get("questions")
        .and_then(|q| q.get(0))
        .and_then(|q| q.get("question"))
        .and_then(Value::as_str)
        .map(clip)
}

/// File paths a tool call names, for the clickable chips on its row.
fn paths_in(input: &Value) -> Vec<String> {
    const KEYS: [&str; 6] = [
        "file_path",
        "notebook_path",
        "Path",
        "path",
        "AbsolutePath",
        "TargetFile",
    ];
    let mut out: Vec<String> = Vec::new();
    let mut push = |p: &str| {
        let p = p.trim();
        if !p.is_empty() && out.len() < MAX_PATHS && !out.iter().any(|x| x == p) {
            out.push(p.to_string());
        }
    };
    for key in KEYS {
        if let Some(p) = input.get(key).and_then(Value::as_str) {
            push(p);
        }
    }
    // Patch-style edits name their files inside the patch text.
    if let Some(map) = input.as_object() {
        for text in map.values().filter_map(Value::as_str) {
            for line in text.lines() {
                for marker in ["*** Add File: ", "*** Update File: ", "*** Delete File: "] {
                    if let Some(p) = line.strip_prefix(marker) {
                        push(p);
                    }
                }
            }
        }
    }
    out
}

fn str_field<'a>(v: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|k| v.get(k).and_then(Value::as_str))
}

/// Trim and cap a string at [`MAX_TEXT`] characters.
fn clip(s: &str) -> String {
    let s = s.trim();
    if s.chars().count() <= MAX_TEXT {
        return s.to_string();
    }
    let cut: String = s.chars().take(MAX_TEXT).collect();
    format!("{}…", cut.trim_end())
}

/// Path of a session's log, or `None` for an id that isn't a plain row id (the
/// id arrives from the frontend, so it must not be able to name another file).
fn log_path(dir: &Path, row_id: &str) -> Option<PathBuf> {
    let ok = !row_id.is_empty()
        && row_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-');
    ok.then(|| dir.join(format!("{row_id}.jsonl")))
}

/// Append one row to a session's log.
pub fn append(dir: &Path, row_id: &str, event: &ActivityEvent) -> std::io::Result<()> {
    let Some(path) = log_path(dir, row_id) else {
        return Ok(());
    };
    std::fs::create_dir_all(dir)?;
    let mut line = serde_json::to_string(event)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    line.push('\n');
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?
        .write_all(line.as_bytes())
}

/// The newest `limit` rows of a session's log, oldest first.
pub fn read(dir: &Path, row_id: &str, limit: usize) -> Vec<ActivityEvent> {
    let Some(path) = log_path(dir, row_id) else {
        return Vec::new();
    };
    let Ok(body) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut rows: Vec<ActivityEvent> = body
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    if rows.len() > limit {
        rows.drain(..rows.len() - limit);
    }
    rows
}

/// Delete a session's log.
pub fn remove(dir: &Path, row_id: &str) {
    if let Some(path) = log_path(dir, row_id) {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn claude_and_codex_tool_calls_become_start_and_end_rows() {
        let start = normalize(
            &json!({
                "hook_event_name": "PreToolUse",
                "tool_name": "Write",
                "tool_use_id": "toolu_1",
                "tool_input": { "file_path": "/w/answer.txt", "content": "SECRET BODY" }
            }),
            1,
        )
        .unwrap();
        assert_eq!(start.kind, ActivityKind::Tool);
        assert_eq!(start.phase.as_deref(), Some("start"));
        assert_eq!(start.tool.as_deref(), Some("Write"));
        assert_eq!(start.call_id.as_deref(), Some("toolu_1"));
        assert_eq!(start.text.as_deref(), Some("/w/answer.txt"));
        assert_eq!(start.paths, vec!["/w/answer.txt".to_string()]);
        let stored = serde_json::to_string(&start).unwrap();
        assert!(
            !stored.contains("SECRET BODY"),
            "file content must not be kept"
        );

        let end = normalize(
            &json!({
                "hook_event_name": "PostToolUse",
                "tool_name": "Bash",
                "tool_use_id": "exec-1",
                "tool_input": { "command": "ls -la" },
                "tool_response": "total 16"
            }),
            2,
        )
        .unwrap();
        assert_eq!(end.phase.as_deref(), Some("end"));
        assert_eq!(end.text.as_deref(), Some("ls -la"));
        assert!(!end.error);
        assert!(!serde_json::to_string(&end).unwrap().contains("total 16"));
    }

    #[test]
    fn antigravity_camel_case_payloads_are_understood() {
        let end = normalize(
            &json!({
                "hookEventName": "PostToolUse",
                "error": "exit status 1",
                "toolCall": {
                    "id": "call_9",
                    "name": "run_command",
                    "args": { "CommandLine": "npm test", "WaitMsBeforeAsync": 5000 }
                }
            }),
            3,
        )
        .unwrap();
        assert_eq!(end.tool.as_deref(), Some("run_command"));
        assert_eq!(end.call_id.as_deref(), Some("call_9"));
        assert_eq!(end.text.as_deref(), Some("npm test"));
        assert!(end.error);

        let first = json!({ "hookEventName": "PreInvocation", "invocationNum": 0, "lastUserInput": "fix it" });
        assert_eq!(normalize(&first, 4).unwrap().kind, ActivityKind::Prompt);
        let later = json!({ "hookEventName": "PreInvocation", "invocationNum": 2, "lastUserInput": "fix it" });
        assert!(
            normalize(&later, 5).is_none(),
            "only the first model call of a turn is a prompt"
        );
    }

    #[test]
    fn approvals_prompts_stops_and_unknown_events() {
        let approval = normalize(
            &json!({
                "hook_event_name": "PermissionRequest",
                "tool_name": "Bash",
                "tool_input": { "command": "curl -sI https://example.com" }
            }),
            1,
        )
        .unwrap();
        assert_eq!(approval.kind, ActivityKind::Approval);
        assert_eq!(
            approval.text.as_deref(),
            Some("curl -sI https://example.com")
        );

        let ask = normalize(
            &json!({
                "hook_event_name": "PreToolUse",
                "tool_name": "AskUserQuestion",
                "tool_input": { "questions": [{ "question": "Red or blue?" }] }
            }),
            1,
        )
        .unwrap();
        assert_eq!(ask.text.as_deref(), Some("Red or blue?"));

        let stop = normalize(
            &json!({ "hook_event_name": "Stop", "last_assistant_message": "Done." }),
            1,
        )
        .unwrap();
        assert_eq!(stop.kind, ActivityKind::Stop);
        assert_eq!(stop.text.as_deref(), Some("Done."));

        assert!(normalize(&json!({ "hook_event_name": "SessionStart" }), 1).is_none());
        assert!(normalize(&json!({ "nothing": true }), 1).is_none());
    }

    #[test]
    fn patch_text_yields_its_file_paths_and_long_text_is_clipped() {
        let ev = normalize(
            &json!({
                "hook_event_name": "PreToolUse",
                "tool_name": "apply_patch",
                "tool_input": { "input": "*** Begin Patch\n*** Update File: src/a.rs\n@@\n*** Add File: src/b.rs\n" }
            }),
            1,
        )
        .unwrap();
        assert_eq!(
            ev.paths,
            vec!["src/a.rs".to_string(), "src/b.rs".to_string()]
        );

        let long = "x".repeat(MAX_TEXT + 50);
        let ev = normalize(
            &json!({ "hook_event_name": "UserPromptSubmit", "prompt": long }),
            1,
        )
        .unwrap();
        assert_eq!(ev.text.unwrap().chars().count(), MAX_TEXT + 1);
    }

    #[test]
    fn log_round_trips_keeps_the_newest_rows_and_rejects_odd_ids() {
        let dir = std::env::temp_dir().join(format!("rustic-activity-{}", uuid::Uuid::new_v4()));
        let row = uuid::Uuid::new_v4().to_string();
        for i in 0..5u64 {
            let ev = normalize(
                &json!({ "hook_event_name": "UserPromptSubmit", "prompt": format!("p{i}") }),
                i,
            )
            .unwrap();
            append(&dir, &row, &ev).unwrap();
        }
        let rows = read(&dir, &row, 3);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].text.as_deref(), Some("p2"));
        assert_eq!(rows[2].text.as_deref(), Some("p4"));

        assert!(read(&dir, "../etc/passwd", 10).is_empty());
        assert!(log_path(&dir, "../x").is_none());

        remove(&dir, &row);
        assert!(read(&dir, &row, 10).is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }
}

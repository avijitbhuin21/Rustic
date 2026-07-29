//! Broker trait for peer top-level agent tasks.
//!
//! Several tasks can run concurrently against the same project. The write path
//! already protects them from clobbering each other (`tools/guarded_write.rs`),
//! but an agent has no way to *see* a peer or talk to it. This module is the
//! bridge: the host app owns the task table, so it implements `PeerAgents` and
//! the agent crate stays free of any Tauri / axum dependency — the same shape
//! as `terminal_broker::AgentTerminals`.
//!
//! Only ACTIVE peers are ever exposed. A task sitting idle is not a
//! coordination hazard, and listing it would invite the model to message a task
//! that will never read its inbox.

use super::{TaskInfo, TaskStatus};
use crate::provider::{ContentBlock, Message, Role};

/// One recent tool call made by a peer, name plus a compact argument summary.
/// Deliberately carries no tool RESULT — the point is to show what a peer is
/// trying to do, not to replay its findings into another agent's context.
#[derive(Debug, Clone)]
pub struct PeerToolCall {
    pub name: String,
    /// Short rendering of the salient arguments (path, command, query…).
    pub summary: String,
}

/// Snapshot of one active peer task.
#[derive(Debug, Clone)]
pub struct PeerAgentInfo {
    pub task_id: String,
    pub title: String,
    /// Lowercase status string: "preparing" | "running" | "waiting_on_subagents".
    pub status: String,
    pub model: String,
    pub provider_type: String,
    /// Active `/goal` completion condition, when the peer is in goal mode.
    pub goal: Option<String>,
    /// How many sub-agents the peer currently has running.
    pub running_subagents: usize,
    /// Most recent tool calls, oldest → newest.
    pub recent_tool_calls: Vec<PeerToolCall>,
    /// Most recent assistant text messages, oldest → newest, truncated.
    pub recent_messages: Vec<String>,
    /// Project-relative paths this peer has written during its task, from
    /// `file_history_task_writes`. The overlap signal that makes the listing
    /// actionable rather than merely informative.
    pub written_paths: Vec<String>,
}

/// How many recent tool calls and assistant messages to carry per peer.
const TOOL_CALL_DEPTH: usize = 10;
const MESSAGE_DEPTH: usize = 2;
/// Cap on one rendered tool-argument summary / assistant message excerpt.
const SUMMARY_CAP: usize = 160;
const MESSAGE_CAP: usize = 400;

/// Statuses that mean "this agent is working and will read its inbox".
pub fn peer_status_is_active(status: &TaskStatus) -> bool {
    matches!(
        status,
        TaskStatus::Preparing | TaskStatus::Running | TaskStatus::WaitingOnSubagents
    )
}

pub fn peer_status_str(status: &TaskStatus) -> &'static str {
    match status {
        TaskStatus::Preparing => "preparing",
        TaskStatus::Running => "running",
        TaskStatus::WaitingOnSubagents => "waiting_on_subagents",
        TaskStatus::Completed => "completed",
        TaskStatus::Failed => "failed",
        TaskStatus::Cancelled => "cancelled",
    }
}

/// Whitespace-collapsed excerpt of `s`, at most `cap` chars plus an ellipsis.
fn truncate_flat(s: &str, cap: usize) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= cap {
        return flat;
    }
    let cut: String = flat.chars().take(cap).collect();
    format!("{}…", cut)
}

/// Compact rendering of a tool call's salient arguments. Prefers the fields
/// that identify WHAT the peer is acting on; falls back to whole-object JSON.
fn summarize_tool_input(input: &serde_json::Value) -> String {
    const KEYS: &[&str] = &[
        "path", "new_path", "command", "query", "pattern", "task_id", "prompt", "name",
    ];
    let mut parts: Vec<String> = Vec::new();
    for key in KEYS {
        if let Some(v) = input.get(*key).and_then(|v| v.as_str()) {
            if !v.trim().is_empty() {
                parts.push(truncate_flat(v, SUMMARY_CAP / 2));
            }
        }
    }
    if parts.is_empty() {
        if input.as_object().map(|o| o.is_empty()).unwrap_or(false) {
            return String::new();
        }
        return truncate_flat(&input.to_string(), SUMMARY_CAP);
    }
    truncate_flat(&parts.join(" · "), SUMMARY_CAP)
}

/// Build a peer snapshot from a task record and its live message history.
/// `running_subagents` and `written_paths` are left empty — only the host can
/// fill them, from the sub-agent registry and the write ledger respectively.
pub fn peer_info_from_history(info: &TaskInfo, messages: &[Message]) -> PeerAgentInfo {
    let mut recent_tool_calls: Vec<PeerToolCall> = Vec::new();
    let mut recent_messages: Vec<String> = Vec::new();
    // Walk newest → oldest so the depth caps bound the work, then reverse into
    // chronological order for the reader.
    for msg in messages.iter().rev() {
        if recent_tool_calls.len() >= TOOL_CALL_DEPTH && recent_messages.len() >= MESSAGE_DEPTH {
            break;
        }
        if !matches!(msg.role, Role::Assistant) {
            continue;
        }
        for block in msg.content.iter().rev() {
            match block {
                ContentBlock::ToolUse { name, input, .. }
                    if recent_tool_calls.len() < TOOL_CALL_DEPTH =>
                {
                    recent_tool_calls.push(PeerToolCall {
                        name: name.clone(),
                        summary: summarize_tool_input(input),
                    });
                }
                ContentBlock::Text { text }
                    if recent_messages.len() < MESSAGE_DEPTH && !text.trim().is_empty() =>
                {
                    recent_messages.push(truncate_flat(text, MESSAGE_CAP));
                }
                _ => {}
            }
        }
    }
    recent_tool_calls.reverse();
    recent_messages.reverse();
    PeerAgentInfo {
        task_id: info.id.clone(),
        title: info.title.clone(),
        status: peer_status_str(&info.status).to_string(),
        model: info.model.clone(),
        provider_type: info.provider_type.clone(),
        goal: info.goal.clone(),
        running_subagents: 0,
        recent_tool_calls,
        recent_messages,
        written_paths: Vec::new(),
    }
}

/// Host-provided view onto peer tasks in the same project.
pub trait PeerAgents: Send + Sync {
    /// Active peers sharing `self_task_id`'s project, excluding the caller
    /// itself and excluding idle / finished tasks.
    fn list_active_peers(&self, self_task_id: &str) -> Vec<PeerAgentInfo>;

    /// Deliver `body` to `to_task_id` as a visible message that starts a turn.
    /// `Err(reason)` when the target is unknown, is not in the caller's
    /// project, or is no longer active.
    fn send_to_peer(&self, from_task_id: &str, to_task_id: &str, body: &str) -> Result<(), String>;

    /// Peer messages `self_task_id` has received and not yet answered, oldest →
    /// newest, each stamped with whether its sender is still reachable.
    fn inbound_peer_messages(&self, self_task_id: &str) -> Vec<InboundPeerMessage>;
}

/// Prefix stamped on a peer-delivered message. The frontend matches this to
/// render the message as a peer capsule with its own background instead of
/// styling it like something the user typed, so keep it in sync with
/// `matchInjectedCapsule` in `src/components/agent/chat-turn.jsx`.
pub const PEER_MESSAGE_PREFIX: &str = "PEER AGENT MESSAGE";

/// Render the body a peer task receives. `from_title` is the sending task's
/// title so the receiver can attribute the request to real work, and
/// `from_task_id` doubles as the reply handle.
pub fn format_peer_message(from_task_id: &str, from_title: &str, body: &str) -> String {
    format!(
        "{} — from concurrent task \"{}\" ({}), running in this same project. \
         This is NOT from the user; another agent sent it to coordinate. To answer, call \
         `message_other_agent` with task_id \"{}\" — and do it now rather than at the end of \
         your work: once that task stops running a reply can no longer reach it. If you have \
         nothing to say back, just act on the message.\n\n{}",
        PEER_MESSAGE_PREFIX,
        from_title,
        from_task_id,
        from_task_id,
        body.trim()
    )
}

/// A peer message this task received. Produced by scanning the receiver's own
/// history, so it survives restarts and needs no side table.
#[derive(Debug, Clone)]
pub struct InboundPeerMessage {
    pub from_task_id: String,
    pub from_title: String,
    /// Truncated body of what the peer said.
    pub excerpt: String,
    /// Whether the sender is still running — i.e. whether a reply can reach it.
    /// Only the host can know this; the history helper leaves it false.
    pub sender_active: bool,
}

/// Split a received peer message into `(from_task_id, from_title, body)`.
/// Mirrors [`format_peer_message`]; returns `None` for anything else.
pub fn parse_peer_message_origin(text: &str) -> Option<(String, String, String)> {
    if !text.starts_with(PEER_MESSAGE_PREFIX) {
        return None;
    }
    const MARKER: &str = "from concurrent task \"";
    let after_marker = &text[text.find(MARKER)? + MARKER.len()..];
    let title_end = after_marker.find('"')?;
    let title = &after_marker[..title_end];
    let rest = after_marker[title_end + 1..].strip_prefix(" (")?;
    let id_end = rest.find(')')?;
    let body = text.split_once("\n\n").map(|(_, b)| b).unwrap_or("").trim();
    Some((
        rest[..id_end].to_string(),
        title.to_string(),
        body.to_string(),
    ))
}

/// Peer messages in `messages` that this task has not answered yet, oldest →
/// newest. A `message_other_agent` call back to the sender counts as the reply;
/// a later message from the same sender re-opens the thread.
pub fn unreplied_inbound_peer_messages(messages: &[Message]) -> Vec<InboundPeerMessage> {
    let mut pending: Vec<InboundPeerMessage> = Vec::new();
    for msg in messages {
        for block in &msg.content {
            match (&msg.role, block) {
                (Role::User, ContentBlock::Text { text }) => {
                    if let Some((id, title, body)) = parse_peer_message_origin(text) {
                        pending.retain(|p| p.from_task_id != id);
                        pending.push(InboundPeerMessage {
                            from_task_id: id,
                            from_title: title,
                            excerpt: truncate_flat(&body, MESSAGE_CAP),
                            sender_active: false,
                        });
                    }
                }
                (Role::Assistant, ContentBlock::ToolUse { name, input, .. })
                    if name == "message_other_agent" =>
                {
                    if let Some(to) = input.get("task_id").and_then(|v| v.as_str()) {
                        pending.retain(|p| p.from_task_id != to);
                    }
                }
                _ => {}
            }
        }
    }
    pending
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_message_carries_prefix_and_origin() {
        let out = format_peer_message("task-9", "Refactor auth", "  hold off on src/auth.rs  ");
        assert!(out.starts_with(PEER_MESSAGE_PREFIX));
        assert!(out.contains("Refactor auth"));
        assert!(out.contains("task-9"));
        assert!(out.trim_end().ends_with("hold off on src/auth.rs"));
    }

    #[test]
    fn peer_message_marks_itself_as_not_user_authored() {
        let out = format_peer_message("t", "T", "body");
        assert!(out.contains("NOT from the user"));
    }

    #[test]
    fn peer_message_states_the_reply_handle() {
        let out = format_peer_message("task-9", "Refactor auth", "body");
        assert!(out.contains("`message_other_agent` with task_id \"task-9\""));
    }

    #[test]
    fn origin_round_trips_through_the_formatter() {
        let text = format_peer_message("task-9", "Refactor auth", "hold off on src/auth.rs");
        let (id, title, body) = parse_peer_message_origin(&text).expect("parses");
        assert_eq!(id, "task-9");
        assert_eq!(title, "Refactor auth");
        assert_eq!(body, "hold off on src/auth.rs");
        assert!(parse_peer_message_origin("just a user message").is_none());
    }

    fn peer_msg(from: &str, body: &str) -> Message {
        Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: format_peer_message(from, "Peer", body),
            }],
        }
    }

    fn reply_to(to: &str) -> Message {
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "x".to_string(),
                name: "message_other_agent".to_string(),
                input: serde_json::json!({ "task_id": to, "message": "ack" }),
                thought_signature: None,
            }],
        }
    }

    #[test]
    fn inbound_messages_are_pending_until_answered() {
        let messages = vec![
            peer_msg("task-a", "claiming auth"),
            peer_msg("task-b", "fyi"),
        ];
        let pending = unreplied_inbound_peer_messages(&messages);
        assert_eq!(pending.len(), 2);
        assert_eq!(pending[0].from_task_id, "task-a");
        assert_eq!(pending[0].excerpt, "claiming auth");
        assert!(!pending[0].sender_active);

        let mut answered = messages.clone();
        answered.push(reply_to("task-a"));
        let pending = unreplied_inbound_peer_messages(&answered);
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].from_task_id, "task-b");
    }

    #[test]
    fn a_new_message_from_an_answered_peer_reopens_the_thread() {
        let messages = vec![
            peer_msg("task-a", "first"),
            reply_to("task-a"),
            peer_msg("task-a", "second"),
        ];
        let pending = unreplied_inbound_peer_messages(&messages);
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].excerpt, "second");
    }

    #[test]
    fn ordinary_user_messages_are_not_inbound_peer_messages() {
        let messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "do the thing".to_string(),
            }],
        }];
        assert!(unreplied_inbound_peer_messages(&messages).is_empty());
    }

    #[test]
    fn only_working_statuses_count_as_active() {
        assert!(peer_status_is_active(&TaskStatus::Running));
        assert!(peer_status_is_active(&TaskStatus::Preparing));
        assert!(peer_status_is_active(&TaskStatus::WaitingOnSubagents));
        assert!(!peer_status_is_active(&TaskStatus::Completed));
        assert!(!peer_status_is_active(&TaskStatus::Failed));
        assert!(!peer_status_is_active(&TaskStatus::Cancelled));
    }

    #[test]
    fn input_summary_prefers_identifying_fields() {
        assert_eq!(
            summarize_tool_input(&serde_json::json!({ "path": "src/auth.rs", "old_string": "x" })),
            "src/auth.rs"
        );
        assert_eq!(
            summarize_tool_input(&serde_json::json!({ "command": "cargo test", "cwd": "." })),
            "cargo test"
        );
        assert_eq!(summarize_tool_input(&serde_json::json!({})), "");
        assert!(summarize_tool_input(&serde_json::json!({ "limit": 5 })).contains("limit"));
    }

    #[test]
    fn truncate_collapses_whitespace_and_marks_elision() {
        assert_eq!(truncate_flat("a\n  b\tc", 40), "a b c");
        let out = truncate_flat(&"x".repeat(50), 10);
        assert_eq!(out.chars().count(), 11);
        assert!(out.ends_with('…'));
    }

    fn info(id: &str) -> TaskInfo {
        TaskInfo {
            id: id.to_string(),
            project_id: "p".to_string(),
            title: "T".to_string(),
            status: TaskStatus::Running,
            provider_type: "claude".to_string(),
            model: "m".to_string(),
            created_at: String::new(),
            updated_at: String::new(),
            thinking_tier: None,
            pinned: false,
            goal: None,
        }
    }

    #[test]
    fn history_extraction_is_chronological_and_depth_capped() {
        let mut messages = Vec::new();
        for i in 0..15 {
            messages.push(Message {
                role: Role::Assistant,
                content: vec![
                    ContentBlock::Text {
                        text: format!("step {}", i),
                    },
                    ContentBlock::ToolUse {
                        id: format!("t{}", i),
                        name: "read_file".to_string(),
                        input: serde_json::json!({ "path": format!("f{}.rs", i) }),
                        thought_signature: None,
                    },
                ],
            });
        }
        let peer = peer_info_from_history(&info("a"), &messages);
        assert_eq!(peer.recent_tool_calls.len(), TOOL_CALL_DEPTH);
        assert_eq!(peer.recent_tool_calls[0].summary, "f5.rs");
        assert_eq!(
            peer.recent_tool_calls[TOOL_CALL_DEPTH - 1].summary,
            "f14.rs"
        );
        assert_eq!(peer.recent_messages, vec!["step 13", "step 14"]);
        assert_eq!(peer.status, "running");
    }

    #[test]
    fn history_extraction_ignores_user_messages() {
        let messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "do the thing".to_string(),
            }],
        }];
        let peer = peer_info_from_history(&info("a"), &messages);
        assert!(peer.recent_messages.is_empty());
        assert!(peer.recent_tool_calls.is_empty());
    }
}

//! `check_other_active_agents` and `message_other_agent` — visibility into and
//! coordination with concurrent top-level tasks in the same project.
//!
//! Both tools are thin: every fact comes from the host's `PeerAgents` broker
//! (see `task/peer_broker.rs`), which owns the task table. This module only
//! formats for the model and validates arguments.

use crate::provider::ToolDef;
use crate::task::peer_broker::{InboundPeerMessage, PeerAgentInfo};
use crate::tools::{ToolContext, ToolOutput};
use anyhow::Result;
use serde_json::{json, Value};

/// Cap on how many written paths are listed per peer before summarising.
const MAX_WRITTEN_PATHS: usize = 25;
/// Cap on a peer message body, to keep one agent from flooding another.
const MAX_MESSAGE_CHARS: usize = 4000;

pub fn definitions() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "check_other_active_agents".into(),
            description: "List the OTHER agents working in this same project right now, so you \
                          can coordinate instead of fighting them for the same files. Returns \
                          one entry per concurrently ACTIVE task (excluding you): its task id, \
                          title, status, model, current goal, how many sub-agents it is running, \
                          the files it has already written this task, its last few tool CALLS \
                          (names + arguments, never their results), and its last couple of \
                          assistant messages.\n\
                          Use it when you suspect someone else is editing what you are editing — \
                          a file changed under you, an `EDIT_NO_MATCH` on a file you just read, \
                          a build breaking on code you did not touch — or BEFORE starting a \
                          large refactor, to check nobody is mid-flight in the same area. The \
                          `written_paths` of each peer is the signal that matters: overlap there \
                          means a real collision risk.\n\
                          Idle and finished tasks are never listed — only agents actually \
                          working. An empty result means you are the only active agent and \
                          anything changing under you is the user or an external process.\n\
                          It also reports any peer message you have received and NOT answered, \
                          with the task id to answer on and whether that sender is still \
                          reachable.\n\
                          This is READ-ONLY. To actually coordinate, follow up with \
                          `message_other_agent`."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "tool_call_limit": {
                        "type": "integer",
                        "description": "How many recent tool calls to show per peer (default 10, max 25)."
                    }
                }
            }),
        },
        ToolDef {
            name: "message_other_agent".into(),
            description: "Send a message to another agent working in this project — the way to \
                          actually coordinate once `check_other_active_agents` shows an overlap. \
                          Use it to claim an area (\"I'm rewriting src/auth.rs, leave it to me\"), \
                          to yield one (\"you finish the migration, I'll wait\"), to warn about a \
                          change you are about to land, or to ask what a peer is doing.\n\
                          IMPORTANT — delivery is immediate and INTERRUPTIVE: the message lands \
                          in the target task as a visible message and starts its next turn, \
                          which supersedes whatever it is generating right now. Its history and \
                          completed tool calls survive, but its in-flight response is cancelled. \
                          So send one clear, self-contained message rather than a stream of \
                          small ones, and say who you are and what you want.\n\
                          The target must be an ACTIVE task id from \
                          `check_other_active_agents` — messaging a task that has finished or \
                          gone idle fails, because nothing will read it. Re-check the listing if \
                          you get that error.\n\
                          REPLYING: a message you receive names the sending task id and this is \
                          how you answer it — call this tool with that id. Reply while the sender \
                          is still running; once it stops, a reply can no longer reach it, so \
                          answer before you disappear into a long stretch of work."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "task_id": {
                        "type": "string",
                        "description": "Task id of the peer agent, exactly as returned by \
                                        `check_other_active_agents`."
                    },
                    "message": {
                        "type": "string",
                        "description": "What to tell the peer. Be explicit about which files or \
                                        areas you are claiming or releasing, and what you want it \
                                        to do — it cannot see your conversation."
                    }
                },
                "required": ["task_id", "message"]
            }),
        },
    ]
}

pub async fn execute(name: &str, params: Value, context: &ToolContext) -> Result<ToolOutput> {
    match name {
        "check_other_active_agents" => Ok(execute_check(params, context)),
        "message_other_agent" => Ok(execute_message(params, context)),
        _ => Ok(ToolOutput {
            content: format!("Unknown peer tool: {}", name),
            is_error: true,
            attachments: Vec::new(),
        }),
    }
}

/// Error returned when the host never wired a peer broker (tests, embedded).
fn no_broker(tool: &str) -> ToolOutput {
    ToolOutput {
        content: format!(
            "PEER_UNAVAILABLE: `{}` is not available in this environment — this host does not \
             expose concurrent task state. Proceed on your own; the write path still protects \
             you from silently clobbering another agent.",
            tool
        ),
        is_error: true,
        attachments: Vec::new(),
    }
}

fn execute_check(params: Value, context: &ToolContext) -> ToolOutput {
    let Some(broker) = context.peer_agents.as_ref() else {
        return no_broker("check_other_active_agents");
    };
    let limit = params["tool_call_limit"]
        .as_u64()
        .unwrap_or(10)
        .clamp(1, 25) as usize;

    let peers = broker.list_active_peers(&context.task_id);
    let inbound = broker.inbound_peer_messages(&context.task_id);
    if peers.is_empty() && inbound.is_empty() {
        return ToolOutput {
            content: "No other agents are active in this project — you are the only one running. \
                      Any file changing under you is the user editing by hand, an external \
                      process, or one of your own sub-agents (use `list_subagents` for those)."
                .into(),
            is_error: false,
            attachments: Vec::new(),
        };
    }

    let mut out = if peers.is_empty() {
        "No other agents are active in this project right now.\n".to_string()
    } else {
        format!(
            "{} other agent(s) active in this project. Their `written_paths` are where collisions \
             happen — compare against what you are about to touch.\n",
            peers.len()
        )
    };
    for p in &peers {
        out.push_str(&render_peer(p, limit));
    }
    if !inbound.is_empty() {
        out.push_str(&render_inbound(&inbound));
    }
    if !peers.is_empty() {
        out.push_str(
            "\nTo coordinate, use `message_other_agent` with one of the task ids above. Note it \
             interrupts the peer's current turn, so make the message count.",
        );
    }
    ToolOutput {
        content: out,
        is_error: false,
        attachments: Vec::new(),
    }
}

/// Render the unanswered-inbox section: who messaged you, and whether a reply
/// can still reach them.
fn render_inbound(inbound: &[InboundPeerMessage]) -> String {
    let mut s = String::from(
        "\n── awaiting your reply ──\nPeer messages you have received and not answered:\n",
    );
    for m in inbound {
        s.push_str(&format!(
            "   · task {} — \"{}\" — {}\n     said: \"{}\"\n",
            m.from_task_id,
            m.from_title,
            if m.sender_active {
                "still running: answer it with `message_other_agent` on this task_id"
            } else {
                "no longer running: a reply cannot reach it, so just act on what it said"
            },
            m.excerpt
        ));
    }
    s
}

/// Render one peer block for the model.
fn render_peer(p: &PeerAgentInfo, tool_call_limit: usize) -> String {
    let mut s = format!(
        "\n── task {} — \"{}\"\n   status: {} · model: {} ({})",
        p.task_id, p.title, p.status, p.model, p.provider_type
    );
    if p.running_subagents > 0 {
        s.push_str(&format!(" · {} sub-agent(s) running", p.running_subagents));
    }
    s.push('\n');
    if let Some(goal) = &p.goal {
        s.push_str(&format!("   goal: {}\n", goal));
    }

    if p.written_paths.is_empty() {
        s.push_str("   written so far: (nothing yet)\n");
    } else {
        let shown = p.written_paths.len().min(MAX_WRITTEN_PATHS);
        s.push_str(&format!(
            "   written so far ({}): {}",
            p.written_paths.len(),
            p.written_paths[..shown].join(", ")
        ));
        if p.written_paths.len() > shown {
            s.push_str(&format!(" … +{} more", p.written_paths.len() - shown));
        }
        s.push('\n');
    }

    if !p.recent_tool_calls.is_empty() {
        let calls =
            &p.recent_tool_calls[p.recent_tool_calls.len().saturating_sub(tool_call_limit)..];
        s.push_str("   recent tool calls (oldest → newest):\n");
        for c in calls {
            if c.summary.is_empty() {
                s.push_str(&format!("     · {}\n", c.name));
            } else {
                s.push_str(&format!("     · {} — {}\n", c.name, c.summary));
            }
        }
    }

    if !p.recent_messages.is_empty() {
        s.push_str("   recent messages:\n");
        for m in &p.recent_messages {
            s.push_str(&format!("     \"{}\"\n", m));
        }
    }
    s
}

fn execute_message(params: Value, context: &ToolContext) -> ToolOutput {
    let Some(broker) = context.peer_agents.as_ref() else {
        return no_broker("message_other_agent");
    };
    let task_id = params["task_id"].as_str().unwrap_or("").trim();
    let message = params["message"].as_str().unwrap_or("").trim();

    if task_id.is_empty() {
        return ToolOutput {
            content: "PEER_MESSAGE_FAILED: `task_id` is required — take it from \
                      `check_other_active_agents`."
                .into(),
            is_error: true,
            attachments: Vec::new(),
        };
    }
    if message.is_empty() {
        return ToolOutput {
            content: "PEER_MESSAGE_FAILED: `message` is required and cannot be empty.".into(),
            is_error: true,
            attachments: Vec::new(),
        };
    }
    if task_id == context.task_id {
        return ToolOutput {
            content: "PEER_MESSAGE_FAILED: that is your own task id — you cannot message \
                      yourself."
                .into(),
            is_error: true,
            attachments: Vec::new(),
        };
    }
    if message.chars().count() > MAX_MESSAGE_CHARS {
        return ToolOutput {
            content: format!(
                "PEER_MESSAGE_FAILED: message is {} characters; keep it under {}. Send the \
                 essential instruction, not your whole reasoning.",
                message.chars().count(),
                MAX_MESSAGE_CHARS
            ),
            is_error: true,
            attachments: Vec::new(),
        };
    }

    match broker.send_to_peer(&context.task_id, task_id, message) {
        Ok(()) => ToolOutput {
            content: format!(
                "Message delivered to task {}. Its current turn was superseded and it will act \
                 on your message next. It may or may not do what you asked — verify rather than \
                 assume, and do not send a follow-up until you have reason to think it was read.",
                task_id
            ),
            is_error: false,
            attachments: Vec::new(),
        },
        Err(reason) => ToolOutput {
            content: format!("PEER_MESSAGE_FAILED: {}", reason),
            is_error: true,
            attachments: Vec::new(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::peer_broker::{PeerAgents, PeerToolCall};
    use std::sync::{Arc, Mutex};

    fn peer(task_id: &str) -> PeerAgentInfo {
        PeerAgentInfo {
            task_id: task_id.into(),
            title: "Refactor auth".into(),
            status: "running".into(),
            model: "claude-sonnet-4-6".into(),
            provider_type: "Claude".into(),
            goal: None,
            running_subagents: 0,
            recent_tool_calls: Vec::new(),
            recent_messages: Vec::new(),
            written_paths: Vec::new(),
        }
    }

    /// Recorded `(from, to, body)` triples for every delivery attempt.
    type SentLog = Arc<Mutex<Vec<(String, String, String)>>>;

    struct FakePeers {
        peers: Vec<PeerAgentInfo>,
        inbound: Vec<InboundPeerMessage>,
        sent: SentLog,
        send_result: Result<(), String>,
    }

    impl PeerAgents for FakePeers {
        fn list_active_peers(&self, _self_task_id: &str) -> Vec<PeerAgentInfo> {
            self.peers.clone()
        }
        fn send_to_peer(&self, from: &str, to: &str, body: &str) -> Result<(), String> {
            self.sent
                .lock()
                .unwrap()
                .push((from.into(), to.into(), body.into()));
            self.send_result.clone()
        }
        fn inbound_peer_messages(&self, _self_task_id: &str) -> Vec<InboundPeerMessage> {
            self.inbound.clone()
        }
    }

    fn ctx_with(
        peers: Vec<PeerAgentInfo>,
        send_result: Result<(), String>,
    ) -> (ToolContext, SentLog) {
        ctx_with_inbox(peers, Vec::new(), send_result)
    }

    fn ctx_with_inbox(
        peers: Vec<PeerAgentInfo>,
        inbound: Vec<InboundPeerMessage>,
        send_result: Result<(), String>,
    ) -> (ToolContext, SentLog) {
        let dir = std::env::temp_dir();
        let (mut ctx, rx) = ToolContext::new_test(dir);
        std::mem::forget(rx);
        let sent = Arc::new(Mutex::new(Vec::new()));
        ctx.task_id = "me".into();
        ctx.peer_agents = Some(Arc::new(FakePeers {
            peers,
            inbound,
            sent: Arc::clone(&sent),
            send_result,
        }));
        (ctx, sent)
    }

    #[test]
    fn no_broker_is_a_clean_error_not_a_panic() {
        let (ctx, rx) = ToolContext::new_test(std::env::temp_dir());
        std::mem::forget(rx);
        let out = execute_check(json!({}), &ctx);
        assert!(out.is_error);
        assert!(out.content.contains("PEER_UNAVAILABLE"));
    }

    #[test]
    fn empty_peer_list_says_you_are_alone() {
        let (ctx, _) = ctx_with(Vec::new(), Ok(()));
        let out = execute_check(json!({}), &ctx);
        assert!(!out.is_error);
        assert!(out.content.contains("only one running"));
    }

    #[test]
    fn peer_listing_includes_written_paths_and_tool_calls() {
        let mut p = peer("task-2");
        p.written_paths = vec!["src/auth.rs".into(), "src/lib.rs".into()];
        p.recent_tool_calls = vec![
            PeerToolCall {
                name: "read_file".into(),
                summary: "src/auth.rs".into(),
            },
            PeerToolCall {
                name: "edit_file".into(),
                summary: "src/auth.rs".into(),
            },
        ];
        p.recent_messages = vec!["Rewriting the token refresh".into()];
        let (ctx, _) = ctx_with(vec![p], Ok(()));
        let out = execute_check(json!({}), &ctx);
        assert!(out.content.contains("task-2"));
        assert!(out.content.contains("src/auth.rs"));
        assert!(out.content.contains("edit_file"));
        assert!(out.content.contains("Rewriting the token refresh"));
    }

    #[test]
    fn tool_call_limit_keeps_the_newest_calls() {
        let mut p = peer("task-2");
        p.recent_tool_calls = (0..10)
            .map(|i| PeerToolCall {
                name: format!("tool{}", i),
                summary: String::new(),
            })
            .collect();
        let (ctx, _) = ctx_with(vec![p], Ok(()));
        let out = execute_check(json!({ "tool_call_limit": 2 }), &ctx);
        assert!(out.content.contains("tool9"));
        assert!(out.content.contains("tool8"));
        assert!(!out.content.contains("tool7"));
    }

    #[test]
    fn written_paths_are_capped_with_a_remainder_note() {
        let mut p = peer("task-2");
        p.written_paths = (0..40).map(|i| format!("f{}.rs", i)).collect();
        let (ctx, _) = ctx_with(vec![p], Ok(()));
        let out = execute_check(json!({}), &ctx);
        assert!(out.content.contains("+15 more"));
    }

    #[test]
    fn messaging_requires_task_id_and_body() {
        let (ctx, _) = ctx_with(vec![peer("task-2")], Ok(()));
        assert!(execute_message(json!({ "message": "hi" }), &ctx).is_error);
        assert!(execute_message(json!({ "task_id": "task-2" }), &ctx).is_error);
    }

    #[test]
    fn messaging_yourself_is_rejected() {
        let (ctx, _) = ctx_with(vec![peer("task-2")], Ok(()));
        let out = execute_message(json!({ "task_id": "me", "message": "hi" }), &ctx);
        assert!(out.is_error);
        assert!(out.content.contains("yourself"));
    }

    #[test]
    fn oversized_message_is_rejected_before_delivery() {
        let (ctx, sent) = ctx_with(vec![peer("task-2")], Ok(()));
        let body = "x".repeat(MAX_MESSAGE_CHARS + 1);
        let out = execute_message(json!({ "task_id": "task-2", "message": body }), &ctx);
        assert!(out.is_error);
        assert!(sent.lock().unwrap().is_empty());
    }

    #[test]
    fn successful_send_reaches_the_broker_with_sender_identity() {
        let (ctx, sent) = ctx_with(vec![peer("task-2")], Ok(()));
        let out = execute_message(
            json!({ "task_id": "task-2", "message": "leave auth to me" }),
            &ctx,
        );
        assert!(!out.is_error);
        let calls = sent.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "me");
        assert_eq!(calls[0].1, "task-2");
        assert_eq!(calls[0].2, "leave auth to me");
    }

    #[test]
    fn broker_rejection_surfaces_as_tool_error() {
        let (ctx, _) = ctx_with(
            vec![peer("task-2")],
            Err("task task-2 is no longer active (status: completed)".into()),
        );
        let out = execute_message(json!({ "task_id": "task-2", "message": "hi" }), &ctx);
        assert!(out.is_error);
        assert!(out.content.contains("no longer active"));
    }

    fn inbound(from: &str, active: bool) -> InboundPeerMessage {
        InboundPeerMessage {
            from_task_id: from.into(),
            from_title: "Migrate db".into(),
            excerpt: "I own the migrations".into(),
            sender_active: active,
        }
    }

    #[test]
    fn unanswered_inbox_is_reported_with_the_reply_handle() {
        let (ctx, _) = ctx_with_inbox(vec![peer("task-2")], vec![inbound("task-2", true)], Ok(()));
        let out = execute_check(json!({}), &ctx);
        assert!(out.content.contains("awaiting your reply"));
        assert!(out.content.contains("I own the migrations"));
        assert!(out.content.contains("answer it with `message_other_agent`"));
    }

    #[test]
    fn unreachable_sender_is_reported_without_inviting_a_reply() {
        let (ctx, _) = ctx_with_inbox(Vec::new(), vec![inbound("task-9", false)], Ok(()));
        let out = execute_check(json!({}), &ctx);
        assert!(!out.is_error);
        assert!(out.content.contains("no longer running"));
        assert!(!out.content.contains("only one running"));
    }
}

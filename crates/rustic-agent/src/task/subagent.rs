use std::collections::{HashMap, VecDeque};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;
use tokio::sync::Notify;

// The sub-agent concurrency cap moved to `BudgetSettings.max_concurrent_subagents`
// so users can raise / disable it from the Settings → Budget panel. The
// historical hard-cap constant lives at
// `crate::budget::DEFAULT_MAX_CONCURRENT_SUBAGENTS` and is read as a
// fallback when the field is missing from a persisted config.

/// Truncate `s` to roughly `ACTIVITY_CONTENT_CAP` bytes on a UTF-8 boundary,
/// appending a `…(+N more)` tail when truncation occurred. Used for tool
/// inputs / results / orchestrator messages stored in the activity ring.
/// Returns a `Cow` so the common short-string case skips an allocation.
/// Project-relative paths a write-tool call touches, extracted from its input
/// (single and batch shapes). Used for the child's `files_written` metadata.
fn extract_write_paths(tool_name: &str, input: &serde_json::Value) -> Vec<String> {
    let mut out = Vec::new();
    let push_str = |out: &mut Vec<String>, v: Option<&serde_json::Value>| {
        if let Some(s) = v.and_then(|x| x.as_str()) {
            if !s.trim().is_empty() {
                out.push(s.to_string());
            }
        }
    };
    match tool_name {
        "create_file" | "edit_file" | "edit_notebook" => {
            push_str(&mut out, input.get("path"));
            let batch_field = if tool_name == "create_file" {
                "creates"
            } else {
                "edits"
            };
            if let Some(entries) = input.get(batch_field).and_then(|v| v.as_array()) {
                for e in entries {
                    push_str(&mut out, e.get("path"));
                }
            }
        }
        "move_file" => {
            push_str(&mut out, input.get("path"));
            push_str(&mut out, input.get("new_path"));
        }
        "apply_patch" => {
            // Paths live inside the diff text — pull them from ---/+++ headers.
            if let Some(patch) = input.get("patch").and_then(|v| v.as_str()) {
                for line in patch.lines() {
                    if let Some(rest) = line
                        .strip_prefix("+++ ")
                        .or_else(|| line.strip_prefix("--- "))
                    {
                        let p = rest.trim();
                        let p = p
                            .strip_prefix("b/")
                            .or_else(|| p.strip_prefix("a/"))
                            .unwrap_or(p);
                        if p != "/dev/null" && !p.is_empty() {
                            out.push(p.to_string());
                        }
                    }
                }
            }
        }
        _ => {}
    }
    out
}

fn truncate_for_activity(s: &str) -> std::borrow::Cow<'_, str> {
    if s.len() <= ACTIVITY_CONTENT_CAP {
        return std::borrow::Cow::Borrowed(s);
    }
    let mut cut = ACTIVITY_CONTENT_CAP;
    while !s.is_char_boundary(cut) {
        cut -= 1;
    }
    let remaining = s.len() - cut;
    std::borrow::Cow::Owned(format!("{}…(+{} more bytes)", &s[..cut], remaining))
}

/// Returns true if two declared-write paths overlap: identical, or one is a
/// directory ancestor of the other. Uses simple string-prefix matching on
/// normalized forward-slash paths — not bulletproof (symlinks, case-insensitive
/// FS quirks) but sufficient for the "don't let two agents edit the same file"
/// contract. Also used at write time by `check_write_scope` in file_ops.rs to
/// reject writes outside a sub-agent's declared scope.
pub fn paths_overlap(a: &str, b: &str) -> bool {
    let norm = |p: &str| p.replace('\\', "/").trim_end_matches('/').to_string();
    let a = norm(a);
    let b = norm(b);
    if a == b {
        return true;
    }
    let a_prefix = format!("{}/", a);
    let b_prefix = format!("{}/", b);
    b.starts_with(&a_prefix) || a.starts_with(&b_prefix)
}

/// A write the sub-agent needed to make but was blocked from by its declared
/// write scope. Returned to the orchestrator so it can decide whether to do
/// the write itself, spawn a follow-up sub-agent, or expand the scope.
#[derive(Debug, Clone)]
pub struct BlockedWrite {
    pub path: String,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub struct SubagentResult {
    pub agent_id: String,
    pub model: String,
    pub summary: String,
    pub notes: Option<String>,
    /// Writes the sub-agent wanted to make but couldn't because they were
    /// outside its declared `writes` scope. Populated via the
    /// `report_blocked_write` tool.
    pub blocked_on: Vec<BlockedWrite>,
}

impl SubagentResult {
    /// Format the "[Sub-agent 'X' completed]" block that gets injected back into
    /// the orchestrator's context. Includes the summary and, when non-empty, a
    /// structured tail listing blocked writes so the orchestrator can decide
    /// what to do with them. Shared across the two injection paths
    /// (executor's `wait_for_any` parking path + executor's `drain_pending`
    /// between tool batches) so they stay in sync.
    pub fn format_completion_block(&self) -> String {
        let mut out = format!(
            "[Sub-agent '{}' completed]\n{}",
            self.agent_id, self.summary
        );
        if let Some(notes) = &self.notes {
            out.push_str(&format!("\n\n[{}]", notes));
        }
        if !self.blocked_on.is_empty() {
            out.push_str(&format!(
                "\n\n[Sub-agent '{}' blocked on {} write(s)]",
                self.agent_id,
                self.blocked_on.len()
            ));
            for bw in &self.blocked_on {
                out.push_str(&format!("\n- {} — {}", bw.path, bw.reason));
            }
            out.push_str(
                "\nYou (the orchestrator) decide: do these writes yourself, spawn a follow-up \
                 sub-agent with narrower scope, or re-dispatch the task with expanded writes.",
            );
        }
        out
    }
}

/// One pending message for a running sub-agent. Sub-agents drain their
/// inbox at the top of every turn, so messages are delivered at the next
/// natural turn boundary rather than as an interrupt. The `kind` lets the
/// executor frame the injected text differently — a `Nudge` is presented
/// as a system steering message; a `User` is presented as the orchestrator
/// speaking.
#[derive(Debug, Clone, PartialEq)]
pub enum InboxKind {
    /// `send_message` from the orchestrator — framed as user speech.
    User,
    /// `nudge_subagent` from the orchestrator — framed as a steering
    /// directive. Higher priority than `User` in the prompt template, but
    /// still consumed at turn boundary (not mid-tool-call).
    Nudge,
}

#[derive(Debug, Clone)]
pub struct InboxMessage {
    pub kind: InboxKind,
    pub content: String,
    pub at: SystemTime,
}

/// One entry in a sub-agent's recent-activity ring buffer. Populated by the
/// event forwarder in `tools::subagent_tools` as the child streams text +
/// tool calls, and read back by the `check_subagent` tool so the orchestrator
/// can inspect what the child is actually doing (not just the last action
/// name surfaced by `list_subagents`).
#[derive(Debug, Clone, PartialEq)]
pub enum ActivityKind {
    /// Assistant text the sub-agent emitted (consecutive deltas are coalesced
    /// into one entry).
    AssistantText,
    /// A tool call the sub-agent made. `content` is `tool_name(json_input)`.
    ToolCall,
    /// The result of a tool call. `content` is prefixed with `[ok]` / `[error]`.
    ToolResult,
    /// A message the orchestrator queued via `send_message`.
    OrchestratorMessage,
    /// A directive the orchestrator queued via `nudge_subagent`.
    OrchestratorNudge,
}

#[derive(Debug, Clone)]
pub struct SubagentActivity {
    pub at: SystemTime,
    pub kind: ActivityKind,
    pub content: String,
}

/// Read-back bundle returned by `SubagentRegistry::read_activity`. Mirrors the
/// fields of `SubagentEntry` that are useful for inspection plus the recent
/// activity list and (when non-empty) the unflushed text buffer appended as a
/// trailing `AssistantText` entry.
#[derive(Debug, Clone)]
pub struct SubagentReadout {
    pub agent_id: String,
    pub model: String,
    pub status: SubagentStatus,
    pub turn_count: u32,
    pub cumulative_cost_usd: f64,
    pub last_action: Option<String>,
    pub activity: Vec<SubagentActivity>,
    /// Total entries in the underlying ring buffer (before tail-trim). Lets the
    /// `check_subagent` tool say "showing last 10 of 47".
    pub total_activity: usize,
}

/// Max activity entries kept per sub-agent. Older entries are evicted FIFO.
/// Keeps memory bounded for long-running children while still giving the
/// orchestrator a useful window of recent behaviour.
const MAX_ACTIVITY_PER_AGENT: usize = 200;
/// Max bytes kept in the pending text buffer (un-flushed deltas) — past this
/// we drop characters from the head, so a 10MB streaming response can't pin
/// the registry.
const TEXT_BUFFER_CAP: usize = 16 * 1024;
/// Per-content cap for tool inputs/results stored in the activity log. The
/// activity buffer is for orchestrator inspection, not full replay — full
/// streams already flow through the UI event channel.
const ACTIVITY_CONTENT_CAP: usize = 2_000;

#[derive(Debug, Clone)]
pub struct SubagentEntry {
    pub agent_id: String,
    pub model: String,
    pub status: SubagentStatus,
    /// File paths this sub-agent declared it will write to. Used by the
    /// spawn-time collision check to serialize agents that target overlapping
    /// files. Empty list = reads-only (safe to run alongside anyone).
    pub writes: Vec<String>,
    /// P1.6: messages queued for the sub-agent. Drained at turn boundary by
    /// the sub-agent's executor. `User`-kind messages frame as orchestrator
    /// speech; `Nudge`-kind frame as a steering directive.
    pub inbox: Vec<InboxMessage>,
    /// P1.6: cancel signal flipped by `stop_subagent`. The sub-agent's
    /// executor reads this between iterations and stops the run loop when
    /// true. `None` means cancellation isn't wired (legacy spawn paths /
    /// tests).
    pub cancel_token: Option<Arc<AtomicBool>>,
    /// P1.6: number of model turns the sub-agent has completed. Reported by
    /// `list_subagents`.
    pub turn_count: u32,
    /// P1.6: estimated USD cost across all turns this sub-agent has run.
    /// Reported by `list_subagents`.
    pub cumulative_cost_usd: f64,
    /// P1.6: short string describing the last action (e.g. "read_file
    /// src/foo.rs"). Optional — `None` until the executor records a turn.
    pub last_action: Option<String>,
    /// Recent activity (text turns, tool calls, tool results, orchestrator
    /// messages) capped at `MAX_ACTIVITY_PER_AGENT`. Newest at the back.
    /// Populated by the event forwarder; read by `check_subagent`.
    pub activity: VecDeque<SubagentActivity>,
    /// Project-relative paths this agent has actually written (create/edit/
    /// move/patch/notebook tools), collected from its tool calls. Surfaced as
    /// structured metadata in the completion block so the orchestrator can
    /// trust-but-verify without re-deriving the change set.
    pub files_written: std::collections::BTreeSet<String>,
    /// Unflushed assistant-text deltas being coalesced into a single entry.
    /// Flushed to `activity` when a non-text event arrives (tool call,
    /// orchestrator message, completion) or appended as a tail entry on read.
    pub text_buffer: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SubagentStatus {
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Clone)]
pub enum SubagentCompletionEvent {
    Completed(SubagentResult),
    Failed {
        agent_id: String,
        error: String,
    },
    /// The child called `escalate_question` and is PAUSED inside that tool
    /// call until the orchestrator replies via `send_message` (or the 24h
    /// timeout fires). Rides the same pending queue as completions so the
    /// parent wakes from its park immediately.
    Escalation {
        agent_id: String,
        question: String,
    },
}

impl SubagentCompletionEvent {
    /// Format this event as the user-message injection block the orchestrator
    /// sees. Shared by the executor's mid-turn drain and the host's
    /// auto-resume path so both produce identical blocks.
    pub fn format_injection(&self, still_running: &[String]) -> String {
        let suffix = if still_running.is_empty() {
            "\n[All sub-agents have finished]".to_string()
        } else {
            format!(
                "\n[{} still running: {}]",
                still_running.len(),
                still_running.join(", ")
            )
        };
        match self {
            Self::Completed(result) => {
                format!("{}{}", result.format_completion_block(), suffix)
            }
            Self::Failed { agent_id, error } => {
                format!("[Sub-agent '{}' FAILED: {}]{}", agent_id, error, suffix)
            }
            Self::Escalation { agent_id, question } => format!(
                "[Sub-agent '{agent}' escalated a question — it is PAUSED inside \
                 escalate_question until you reply with send_message('{agent}', <answer>)]\n\
                 Question: {q}\n\n\
                 Answer from your own context/authority if you can; use ask_user only \
                 if it genuinely needs the user's judgment.",
                agent = agent_id,
                q = question,
            ),
        }
    }
}

pub struct SubagentRegistry {
    /// parent_task_id -> agent_id -> SubagentEntry
    agents: Mutex<HashMap<String, HashMap<String, SubagentEntry>>>,
    /// parent_task_id -> queue of unconsumed completion events
    pending: Mutex<HashMap<String, VecDeque<SubagentCompletionEvent>>>,
    /// parent_task_id -> Notify (wakes wait_for_any on completion)
    notifies: Mutex<HashMap<String, Arc<Notify>>>,
    /// (parent_task_id, agent_id) -> reply channel for a pending
    /// `escalate_question`. The child blocks on the receiver; the parent's
    /// next `send_message` to that agent resolves it.
    escalations: Mutex<HashMap<(String, String), tokio::sync::oneshot::Sender<String>>>,
}

impl SubagentRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            agents: Mutex::new(HashMap::new()),
            pending: Mutex::new(HashMap::new()),
            notifies: Mutex::new(HashMap::new()),
            escalations: Mutex::new(HashMap::new()),
        })
    }

    /// Register a new sub-agent under a parent task.
    pub fn register(
        &self,
        parent_task_id: &str,
        agent_id: &str,
        model: &str,
        writes: Vec<String>,
        cancel_token: Option<Arc<AtomicBool>>,
    ) {
        let mut agents = self.agents.lock().unwrap();
        let task_agents = agents.entry(parent_task_id.to_string()).or_default();
        task_agents.insert(
            agent_id.to_string(),
            SubagentEntry {
                agent_id: agent_id.to_string(),
                model: model.to_string(),
                status: SubagentStatus::Running,
                writes,
                inbox: Vec::new(),
                cancel_token,
                turn_count: 0,
                cumulative_cost_usd: 0.0,
                last_action: None,
                activity: VecDeque::new(),
                files_written: std::collections::BTreeSet::new(),
                text_buffer: String::new(),
            },
        );
        // Ensure a Notify exists for this parent task
        let mut notifies = self.notifies.lock().unwrap();
        notifies
            .entry(parent_task_id.to_string())
            .or_insert_with(|| Arc::new(Notify::new()));
    }

    /// P1.6: push a message into a running sub-agent's inbox. Returns
    /// `Ok(())` on success; `Err` describes why the push failed (no such
    /// sub-agent, sub-agent already completed/failed). The sub-agent's
    /// executor drains the inbox at the top of its next iteration.
    pub fn push_inbox(
        &self,
        parent_task_id: &str,
        agent_id: &str,
        kind: InboxKind,
        content: String,
    ) -> Result<(), String> {
        let mut agents = self.agents.lock().unwrap();
        let task_agents = agents
            .get_mut(parent_task_id)
            .ok_or_else(|| format!("No sub-agents registered for task `{}`", parent_task_id))?;
        let entry = task_agents.get_mut(agent_id).ok_or_else(|| {
            format!(
                "No sub-agent `{}` under task `{}`",
                agent_id, parent_task_id
            )
        })?;
        if entry.status != SubagentStatus::Running {
            return Err(format!(
                "Sub-agent `{}` is not running (status: {:?}) — message not delivered",
                agent_id, entry.status
            ));
        }
        entry.inbox.push(InboxMessage {
            kind,
            content,
            at: SystemTime::now(),
        });
        Ok(())
    }

    /// P1.6: drain (and clear) the inbox for a sub-agent. Called by the
    /// sub-agent's executor at every turn boundary. Returns the messages in
    /// FIFO order; the caller is responsible for formatting them into the
    /// next User message.
    pub fn drain_inbox(&self, parent_task_id: &str, agent_id: &str) -> Vec<InboxMessage> {
        let mut agents = self.agents.lock().unwrap();
        let Some(task_agents) = agents.get_mut(parent_task_id) else {
            return Vec::new();
        };
        let Some(entry) = task_agents.get_mut(agent_id) else {
            return Vec::new();
        };
        std::mem::take(&mut entry.inbox)
    }

    /// Paths a sub-agent has written so far (from its write-tool calls).
    pub fn files_written(&self, parent_task_id: &str, agent_id: &str) -> Vec<String> {
        let agents = self.agents.lock().unwrap();
        agents
            .get(parent_task_id)
            .and_then(|m| m.get(agent_id))
            .map(|e| e.files_written.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Register a pending `escalate_question` from a running child. Pushes an
    /// `Escalation` event onto the parent's pending queue (waking any park)
    /// and returns the receiver the child blocks on.
    pub fn register_escalation(
        &self,
        parent_task_id: &str,
        agent_id: &str,
        question: &str,
    ) -> Result<tokio::sync::oneshot::Receiver<String>, String> {
        {
            let agents = self.agents.lock().unwrap();
            let entry = agents
                .get(parent_task_id)
                .and_then(|m| m.get(agent_id))
                .ok_or_else(|| {
                    format!(
                        "No sub-agent `{}` under task `{}`",
                        agent_id, parent_task_id
                    )
                })?;
            if entry.status != SubagentStatus::Running {
                return Err(format!(
                    "Sub-agent `{}` is not running (status: {:?})",
                    agent_id, entry.status
                ));
            }
        }
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.escalations
            .lock()
            .unwrap()
            .insert((parent_task_id.to_string(), agent_id.to_string()), tx);
        self.pending
            .lock()
            .unwrap()
            .entry(parent_task_id.to_string())
            .or_default()
            .push_back(SubagentCompletionEvent::Escalation {
                agent_id: agent_id.to_string(),
                question: question.to_string(),
            });
        if let Some(n) = self.notifies.lock().unwrap().get(parent_task_id) {
            n.notify_one();
        }
        Ok(rx)
    }

    /// True when the agent has an unanswered escalation — `send_message`
    /// routes its content as the ANSWER in that case instead of queueing it
    /// on the inbox.
    pub fn has_pending_escalation(&self, parent_task_id: &str, agent_id: &str) -> bool {
        self.escalations
            .lock()
            .unwrap()
            .contains_key(&(parent_task_id.to_string(), agent_id.to_string()))
    }

    /// Deliver the orchestrator's answer to a pending escalation. Returns
    /// false when no escalation was pending (or the child already timed out).
    pub fn answer_escalation(&self, parent_task_id: &str, agent_id: &str, answer: String) -> bool {
        let sender = self
            .escalations
            .lock()
            .unwrap()
            .remove(&(parent_task_id.to_string(), agent_id.to_string()));
        match sender {
            Some(tx) => tx.send(answer).is_ok(),
            None => false,
        }
    }

    /// P1.6: signal a sub-agent to stop. Flips the AtomicBool the
    /// sub-agent's executor watches. Returns true if a cancel token was
    /// wired (i.e. the signal will be observed); false otherwise.
    pub fn cancel(&self, parent_task_id: &str, agent_id: &str) -> bool {
        let agents = self.agents.lock().unwrap();
        let Some(task_agents) = agents.get(parent_task_id) else {
            return false;
        };
        let Some(entry) = task_agents.get(agent_id) else {
            return false;
        };
        if let Some(tok) = &entry.cancel_token {
            tok.store(true, std::sync::atomic::Ordering::SeqCst);
            true
        } else {
            false
        }
    }

    /// P1.6: record a turn's outcome — increments the turn counter, adds
    /// to the cumulative cost, and replaces `last_action`. Called by the
    /// sub-agent's executor at the end of each turn.
    pub fn record_turn(
        &self,
        parent_task_id: &str,
        agent_id: &str,
        action: Option<String>,
        cost_delta_usd: f64,
    ) {
        let mut agents = self.agents.lock().unwrap();
        let Some(task_agents) = agents.get_mut(parent_task_id) else {
            return;
        };
        let Some(entry) = task_agents.get_mut(agent_id) else {
            return;
        };
        entry.turn_count = entry.turn_count.saturating_add(1);
        entry.cumulative_cost_usd += cost_delta_usd.max(0.0);
        if let Some(a) = action {
            entry.last_action = Some(a);
        }
    }

    /// Internal: flush any pending text-buffer on the given entry into the
    /// activity ring. Caller must hold the agents lock.
    fn flush_text_buffer_locked(entry: &mut SubagentEntry) {
        if entry.text_buffer.is_empty() {
            return;
        }
        let text = std::mem::take(&mut entry.text_buffer);
        entry.activity.push_back(SubagentActivity {
            at: SystemTime::now(),
            kind: ActivityKind::AssistantText,
            content: text,
        });
        while entry.activity.len() > MAX_ACTIVITY_PER_AGENT {
            entry.activity.pop_front();
        }
    }

    /// Append a streaming text delta from the sub-agent. Deltas accumulate in
    /// the entry's `text_buffer` and are flushed into the activity ring when
    /// a non-text event arrives, or when `check_subagent` reads the buffer.
    pub fn record_text_delta(&self, parent_task_id: &str, agent_id: &str, delta: &str) {
        if delta.is_empty() {
            return;
        }
        let mut agents = self.agents.lock().unwrap();
        let Some(task_agents) = agents.get_mut(parent_task_id) else {
            return;
        };
        let Some(entry) = task_agents.get_mut(agent_id) else {
            return;
        };
        entry.text_buffer.push_str(delta);
        if entry.text_buffer.len() > TEXT_BUFFER_CAP {
            let excess = entry.text_buffer.len() - TEXT_BUFFER_CAP;
            // Drain by chars to avoid splitting a UTF-8 boundary.
            let mut drained_bytes = 0usize;
            let mut idx = 0usize;
            for (i, _) in entry.text_buffer.char_indices() {
                if drained_bytes >= excess {
                    idx = i;
                    break;
                }
                drained_bytes = i;
            }
            entry.text_buffer.drain(..idx);
        }
    }

    /// Record a tool call the sub-agent made. Flushes any pending text first
    /// so the activity ring stays in temporal order.
    pub fn record_tool_call(
        &self,
        parent_task_id: &str,
        agent_id: &str,
        tool_name: &str,
        input: &serde_json::Value,
    ) {
        let mut agents = self.agents.lock().unwrap();
        let Some(task_agents) = agents.get_mut(parent_task_id) else {
            return;
        };
        let Some(entry) = task_agents.get_mut(agent_id) else {
            return;
        };
        Self::flush_text_buffer_locked(entry);
        for p in extract_write_paths(tool_name, input) {
            entry.files_written.insert(p);
        }
        let input_str = serde_json::to_string(input).unwrap_or_else(|_| "<unprintable>".into());
        let content = format!("{}({})", tool_name, truncate_for_activity(&input_str));
        entry.activity.push_back(SubagentActivity {
            at: SystemTime::now(),
            kind: ActivityKind::ToolCall,
            content,
        });
        while entry.activity.len() > MAX_ACTIVITY_PER_AGENT {
            entry.activity.pop_front();
        }
    }

    /// Record a tool result the sub-agent received. Does NOT flush text first
    /// — results follow their corresponding call, not interleaving model text.
    pub fn record_tool_result(
        &self,
        parent_task_id: &str,
        agent_id: &str,
        content: &str,
        is_error: bool,
    ) {
        let mut agents = self.agents.lock().unwrap();
        let Some(task_agents) = agents.get_mut(parent_task_id) else {
            return;
        };
        let Some(entry) = task_agents.get_mut(agent_id) else {
            return;
        };
        let prefix = if is_error { "[error]" } else { "[ok]" };
        let body = truncate_for_activity(content);
        entry.activity.push_back(SubagentActivity {
            at: SystemTime::now(),
            kind: ActivityKind::ToolResult,
            content: format!("{} {}", prefix, body),
        });
        while entry.activity.len() > MAX_ACTIVITY_PER_AGENT {
            entry.activity.pop_front();
        }
    }

    /// Record an orchestrator-originated message (either `send_message` user
    /// content or a `nudge_subagent` directive). Mirrors the inbox push so the
    /// activity log reflects what the orchestrator told the child, in order.
    pub fn record_orchestrator_message(
        &self,
        parent_task_id: &str,
        agent_id: &str,
        kind: InboxKind,
        content: &str,
    ) {
        let mut agents = self.agents.lock().unwrap();
        let Some(task_agents) = agents.get_mut(parent_task_id) else {
            return;
        };
        let Some(entry) = task_agents.get_mut(agent_id) else {
            return;
        };
        Self::flush_text_buffer_locked(entry);
        let activity_kind = match kind {
            InboxKind::User => ActivityKind::OrchestratorMessage,
            InboxKind::Nudge => ActivityKind::OrchestratorNudge,
        };
        entry.activity.push_back(SubagentActivity {
            at: SystemTime::now(),
            kind: activity_kind,
            content: truncate_for_activity(content).into_owned(),
        });
        while entry.activity.len() > MAX_ACTIVITY_PER_AGENT {
            entry.activity.pop_front();
        }
    }

    /// Read back recent activity for one sub-agent. Returns `None` if no such
    /// agent. The returned `activity` vec is ordered oldest → newest and
    /// includes a synthetic trailing `AssistantText` entry if there's an
    /// unflushed text buffer (so in-progress streaming text shows up too).
    /// `total_activity` is the un-trimmed count for "showing N of M" framing.
    pub fn read_activity(&self, parent_task_id: &str, agent_id: &str) -> Option<SubagentReadout> {
        let agents = self.agents.lock().unwrap();
        let task_agents = agents.get(parent_task_id)?;
        let entry = task_agents.get(agent_id)?;
        let mut activity: Vec<SubagentActivity> = entry.activity.iter().cloned().collect();
        if !entry.text_buffer.is_empty() {
            activity.push(SubagentActivity {
                at: SystemTime::now(),
                kind: ActivityKind::AssistantText,
                content: entry.text_buffer.clone(),
            });
        }
        let total_activity = activity.len();
        Some(SubagentReadout {
            agent_id: entry.agent_id.clone(),
            model: entry.model.clone(),
            status: entry.status.clone(),
            turn_count: entry.turn_count,
            cumulative_cost_usd: entry.cumulative_cost_usd,
            last_action: entry.last_action.clone(),
            activity,
            total_activity,
        })
    }

    /// Returns the id of a currently-running sub-agent whose declared writes
    /// overlap with `candidate_writes`. Returns None if no collision. Used by
    /// spawn_subagent to reject spawns that would race on the same file.
    pub fn find_write_collision(
        &self,
        parent_task_id: &str,
        candidate_writes: &[String],
    ) -> Option<String> {
        if candidate_writes.is_empty() {
            return None;
        }
        let agents = self.agents.lock().unwrap();
        let task_agents = match agents.get(parent_task_id) {
            Some(m) => m,
            None => return None,
        };
        for entry in task_agents.values() {
            if entry.status != SubagentStatus::Running {
                continue;
            }
            for w in &entry.writes {
                if candidate_writes.iter().any(|c| paths_overlap(c, w)) {
                    return Some(entry.agent_id.clone());
                }
            }
        }
        None
    }

    /// Count currently-running sub-agents for a parent task.
    pub fn running_count(&self, parent_task_id: &str) -> usize {
        let agents = self.agents.lock().unwrap();
        agents
            .get(parent_task_id)
            .map(|m| {
                m.values()
                    .filter(|e| e.status == SubagentStatus::Running)
                    .count()
            })
            .unwrap_or(0)
    }

    /// Mark a sub-agent as completed and wake any waiting executor.
    pub fn complete(&self, parent_task_id: &str, result: SubagentResult) {
        {
            let mut agents = self.agents.lock().unwrap();
            if let Some(task_agents) = agents.get_mut(parent_task_id) {
                if let Some(entry) = task_agents.get_mut(&result.agent_id) {
                    Self::flush_text_buffer_locked(entry);
                    entry.status = SubagentStatus::Completed;
                }
            }
        }
        {
            let mut pending = self.pending.lock().unwrap();
            pending
                .entry(parent_task_id.to_string())
                .or_default()
                .push_back(SubagentCompletionEvent::Completed(result));
        }
        let notifies = self.notifies.lock().unwrap();
        if let Some(notify) = notifies.get(parent_task_id) {
            notify.notify_one();
        }
    }

    /// Mark a sub-agent as failed and wake any waiting executor.
    pub fn fail(&self, parent_task_id: &str, agent_id: &str, error: String) {
        {
            let mut agents = self.agents.lock().unwrap();
            if let Some(task_agents) = agents.get_mut(parent_task_id) {
                if let Some(entry) = task_agents.get_mut(agent_id) {
                    Self::flush_text_buffer_locked(entry);
                    entry.status = SubagentStatus::Failed;
                }
            }
        }
        {
            let mut pending = self.pending.lock().unwrap();
            pending
                .entry(parent_task_id.to_string())
                .or_default()
                .push_back(SubagentCompletionEvent::Failed {
                    agent_id: agent_id.to_string(),
                    error,
                });
        }
        let notifies = self.notifies.lock().unwrap();
        if let Some(notify) = notifies.get(parent_task_id) {
            notify.notify_one();
        }
    }

    /// Returns all Running sub-agents for the given parent task.
    pub fn active_for_task(&self, parent_task_id: &str) -> Vec<SubagentEntry> {
        let agents = self.agents.lock().unwrap();
        agents
            .get(parent_task_id)
            .map(|m| {
                m.values()
                    .filter(|e| e.status == SubagentStatus::Running)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Returns all sub-agents (any status) for the given parent task.
    pub fn all_for_task(&self, parent_task_id: &str) -> Vec<SubagentEntry> {
        let agents = self.agents.lock().unwrap();
        agents
            .get(parent_task_id)
            .map(|m| m.values().cloned().collect())
            .unwrap_or_default()
    }

    /// Drain all pending (already-completed) events without blocking.
    /// Returns events for sub-agents that finished while the model was generating or tools were executing.
    pub fn drain_pending(&self, parent_task_id: &str) -> Vec<SubagentCompletionEvent> {
        let mut pending = self.pending.lock().unwrap();
        if let Some(queue) = pending.get_mut(parent_task_id) {
            queue.drain(..).collect()
        } else {
            Vec::new()
        }
    }

    /// Non-consuming check for queued completion events.
    pub fn has_pending(&self, parent_task_id: &str) -> bool {
        let pending = self.pending.lock().unwrap();
        pending
            .get(parent_task_id)
            .map(|q| !q.is_empty())
            .unwrap_or(false)
    }

    /// Return an already-consumed event to the front of the pending queue.
    /// Used by the host's auto-resume watcher when it loses the claim race
    /// (a new run started first) so the running executor drains it instead.
    pub fn push_front_pending(&self, parent_task_id: &str, event: SubagentCompletionEvent) {
        {
            let mut pending = self.pending.lock().unwrap();
            pending
                .entry(parent_task_id.to_string())
                .or_default()
                .push_front(event);
        }
        let notifies = self.notifies.lock().unwrap();
        if let Some(notify) = notifies.get(parent_task_id) {
            notify.notify_one();
        }
    }

    /// Flip the cancel token of every running sub-agent under a parent task.
    /// Returns how many were signalled. Used when the user aborts the task.
    pub fn cancel_all_for_task(&self, parent_task_id: &str) -> usize {
        let agents = self.agents.lock().unwrap();
        let mut signalled = 0;
        if let Some(task_agents) = agents.get(parent_task_id) {
            for entry in task_agents.values() {
                if entry.status == SubagentStatus::Running {
                    if let Some(tok) = &entry.cancel_token {
                        tok.store(true, std::sync::atomic::Ordering::SeqCst);
                        signalled += 1;
                    }
                }
            }
        }
        signalled
    }

    /// Wait asynchronously for any sub-agent to complete or fail.
    /// Returns None if there are no active agents.
    pub async fn wait_for_any(&self, parent_task_id: &str) -> Option<SubagentCompletionEvent> {
        loop {
            // Check the pending queue first
            {
                let mut pending = self.pending.lock().unwrap();
                if let Some(queue) = pending.get_mut(parent_task_id) {
                    if let Some(event) = queue.pop_front() {
                        return Some(event);
                    }
                }
            }

            // If there are no running agents (and no pending events), we're done
            if self.active_for_task(parent_task_id).is_empty() {
                return None;
            }

            // Get the Notify for this parent task
            let notify = {
                let notifies = self.notifies.lock().unwrap();
                notifies.get(parent_task_id)?.clone()
            };

            // Wait for a notification (notify_one() stores a permit if called before we await)
            notify.notified().await;
            // Loop back to check the queue
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_overlap_identical() {
        assert!(paths_overlap("src/foo.rs", "src/foo.rs"));
    }

    #[test]
    fn paths_overlap_directory_ancestor() {
        // One path is a directory ancestor of the other.
        assert!(paths_overlap("src/foo", "src/foo/bar.rs"));
        assert!(paths_overlap("src/foo/bar.rs", "src/foo"));
    }

    #[test]
    fn paths_overlap_sibling_no_overlap() {
        assert!(!paths_overlap("src/foo.rs", "src/bar.rs"));
        assert!(!paths_overlap("src/foo", "src/foobar"));
    }

    #[test]
    fn paths_overlap_normalizes_backslashes() {
        // Windows-style paths should match their forward-slash counterparts.
        assert!(paths_overlap("src\\foo.rs", "src/foo.rs"));
        assert!(paths_overlap("src\\foo", "src/foo/bar.rs"));
    }

    #[test]
    fn paths_overlap_trailing_slash_tolerance() {
        // Trailing slashes on directory scope entries shouldn't matter.
        assert!(paths_overlap("src/foo/", "src/foo/bar.rs"));
    }

    // ── P1.6 registry surface ───────────────────────────────────────────

    #[test]
    fn inbox_push_and_drain() {
        let reg = SubagentRegistry::new();
        reg.register("t1", "a1", "claude-sonnet-4-6", vec![], None);
        reg.push_inbox("t1", "a1", InboxKind::User, "hi there".into())
            .unwrap();
        reg.push_inbox(
            "t1",
            "a1",
            InboxKind::Nudge,
            "stop reading, summarize".into(),
        )
        .unwrap();
        let msgs = reg.drain_inbox("t1", "a1");
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].kind, InboxKind::User);
        assert_eq!(msgs[1].kind, InboxKind::Nudge);
        // Subsequent drain returns empty.
        assert!(reg.drain_inbox("t1", "a1").is_empty());
    }

    #[test]
    fn push_inbox_to_unknown_agent_errors() {
        let reg = SubagentRegistry::new();
        let err = reg
            .push_inbox("t1", "missing", InboxKind::User, "hello".into())
            .unwrap_err();
        assert!(err.contains("No sub-agents registered"));
    }

    #[test]
    fn push_inbox_to_completed_agent_errors() {
        let reg = SubagentRegistry::new();
        reg.register("t1", "a1", "m", vec![], None);
        reg.complete(
            "t1",
            SubagentResult {
                agent_id: "a1".into(),
                model: "m".into(),
                summary: "done".into(),
                notes: None,
                blocked_on: vec![],
            },
        );
        let err = reg
            .push_inbox("t1", "a1", InboxKind::User, "too late".into())
            .unwrap_err();
        assert!(err.contains("not running"));
    }

    #[test]
    fn cancel_flips_token() {
        let reg = SubagentRegistry::new();
        let tok = Arc::new(AtomicBool::new(false));
        reg.register("t1", "a1", "m", vec![], Some(tok.clone()));
        assert!(reg.cancel("t1", "a1"));
        assert!(tok.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn cancel_returns_false_when_no_token() {
        let reg = SubagentRegistry::new();
        reg.register("t1", "a1", "m", vec![], None);
        assert!(!reg.cancel("t1", "a1"));
    }

    #[test]
    fn record_turn_accumulates() {
        let reg = SubagentRegistry::new();
        reg.register("t1", "a1", "m", vec![], None);
        reg.record_turn("t1", "a1", Some("read_file foo.rs".into()), 0.0123);
        reg.record_turn("t1", "a1", Some("edit_file foo.rs".into()), 0.0044);
        let entries = reg.all_for_task("t1");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].turn_count, 2);
        assert!((entries[0].cumulative_cost_usd - 0.0167).abs() < 1e-9);
        assert_eq!(entries[0].last_action.as_deref(), Some("edit_file foo.rs"));
    }

    // ── P1.9 parking-loop building blocks + C8.3 model field round-trip ──
    //
    // These cover the registry surface the executor's park-on-end_turn loop
    // relies on. The executor itself wraps `wait_for_any` in a 30-min
    // `tokio::time::timeout` and re-arms on elapsed; the timeout pattern is
    // simulated in `park_timeout_re_arms_and_eventually_wakes` below using a
    // short timeout so we can verify the "keep waiting after timeout" flow
    // without sleeping for real minutes.

    fn make_result(agent_id: &str, model: &str, summary: &str) -> SubagentResult {
        SubagentResult {
            agent_id: agent_id.into(),
            model: model.into(),
            summary: summary.into(),
            notes: None,
            blocked_on: vec![],
        }
    }

    #[test]
    fn drain_pending_returns_events_in_fifo_order() {
        let reg = SubagentRegistry::new();
        reg.register("t1", "a", "m", vec![], None);
        reg.register("t1", "b", "m", vec![], None);
        reg.complete("t1", make_result("a", "m", "did a"));
        reg.complete("t1", make_result("b", "m", "did b"));
        let events = reg.drain_pending("t1");
        assert_eq!(events.len(), 2);
        match &events[0] {
            SubagentCompletionEvent::Completed(r) => assert_eq!(r.agent_id, "a"),
            other => panic!("expected Completed(a), got {:?}", other),
        }
        match &events[1] {
            SubagentCompletionEvent::Completed(r) => assert_eq!(r.agent_id, "b"),
            other => panic!("expected Completed(b), got {:?}", other),
        }
        // Second drain is empty.
        assert!(reg.drain_pending("t1").is_empty());
    }

    #[test]
    fn drain_pending_returns_failures_mixed_with_completions() {
        let reg = SubagentRegistry::new();
        reg.register("t1", "a", "m", vec![], None);
        reg.register("t1", "b", "m", vec![], None);
        reg.complete("t1", make_result("a", "m", "done"));
        reg.fail("t1", "b", "boom".into());
        let events = reg.drain_pending("t1");
        assert_eq!(events.len(), 2);
        matches!(&events[0], SubagentCompletionEvent::Completed(_));
        match &events[1] {
            SubagentCompletionEvent::Failed { agent_id, error } => {
                assert_eq!(agent_id, "b");
                assert_eq!(error, "boom");
            }
            other => panic!("expected Failed for b, got {:?}", other),
        }
    }

    // C8.3 — model field round-trips through the completion path.
    #[tokio::test]
    async fn wait_for_any_preserves_model_field() {
        let reg = SubagentRegistry::new();
        reg.register("t1", "a", "claude-opus-4-7", vec![], None);
        reg.complete("t1", make_result("a", "claude-opus-4-7", "all done"));
        let event = reg.wait_for_any("t1").await;
        match event {
            Some(SubagentCompletionEvent::Completed(result)) => {
                assert_eq!(result.agent_id, "a");
                assert_eq!(
                    result.model, "claude-opus-4-7",
                    "model name must survive the registry round-trip"
                );
                assert_eq!(result.summary, "all done");
            }
            other => panic!("expected Completed event, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn wait_for_any_returns_none_when_no_active_agents() {
        let reg = SubagentRegistry::new();
        // No registrations, no pending events.
        assert!(reg.wait_for_any("t1").await.is_none());
    }

    #[tokio::test]
    async fn wait_for_any_drains_pre_existing_pending_without_blocking() {
        let reg = SubagentRegistry::new();
        reg.register("t1", "a", "m", vec![], None);
        // Complete BEFORE waiting — the event sits in pending until drained.
        reg.complete("t1", make_result("a", "m", "early bird"));
        // Should return immediately without blocking.
        let event = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            reg.wait_for_any("t1"),
        )
        .await
        .expect("wait_for_any must not block when pending queue is non-empty");
        assert!(event.is_some());
    }

    #[tokio::test]
    async fn wait_for_any_blocks_then_wakes_on_complete() {
        let reg = SubagentRegistry::new();
        reg.register("t1", "a", "m", vec![], None);
        let reg_clone = Arc::clone(&reg);
        // Spawn the wait first — it should block because no event is pending.
        let wait_handle = tokio::spawn(async move { reg_clone.wait_for_any("t1").await });
        // Give the wait a moment to actually park on the notifier.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        // Fire the completion — Notify::notify_one should wake the wait.
        reg.complete("t1", make_result("a", "m", "woke up"));
        let event = tokio::time::timeout(std::time::Duration::from_millis(500), wait_handle)
            .await
            .expect("wait must wake within 500ms of complete()")
            .expect("join handle must not panic");
        match event {
            Some(SubagentCompletionEvent::Completed(r)) => {
                assert_eq!(r.summary, "woke up");
            }
            other => panic!("expected Completed, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn wait_for_any_blocks_then_wakes_on_fail() {
        let reg = SubagentRegistry::new();
        reg.register("t1", "a", "m", vec![], None);
        let reg_clone = Arc::clone(&reg);
        let wait_handle = tokio::spawn(async move { reg_clone.wait_for_any("t1").await });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        reg.fail("t1", "a", "kaboom".into());
        let event = tokio::time::timeout(std::time::Duration::from_millis(500), wait_handle)
            .await
            .expect("wait must wake on fail()")
            .expect("join handle ok");
        match event {
            Some(SubagentCompletionEvent::Failed { agent_id, error }) => {
                assert_eq!(agent_id, "a");
                assert_eq!(error, "kaboom");
            }
            other => panic!("expected Failed, got {:?}", other),
        }
    }

    // C7.7 — the executor's parking pattern: tokio::time::timeout(short, wait)
    // returns Err on elapsed, the loop checks active_for_task, and re-arms.
    // We simulate this with a 50ms timeout so the timeout-then-keep-waiting
    // path is exercised before the real completion arrives.
    #[tokio::test]
    async fn park_timeout_re_arms_and_eventually_wakes() {
        let reg = SubagentRegistry::new();
        reg.register("t1", "a", "m", vec![], None);

        // Sleep then complete — the wait must time out at least once before
        // the completion fires.
        let reg_completer = Arc::clone(&reg);
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            reg_completer.complete("t1", make_result("a", "m", "late but here"));
        });

        // Mirror the executor's loop: timeout, on Err check active and continue.
        let mut timeouts_observed: u32 = 0;
        let result = loop {
            match tokio::time::timeout(std::time::Duration::from_millis(50), reg.wait_for_any("t1"))
                .await
            {
                Ok(event) => break event,
                Err(_elapsed) => {
                    timeouts_observed = timeouts_observed.saturating_add(1);
                    let still_active = reg.active_for_task("t1");
                    if still_active.is_empty() {
                        break None;
                    }
                    // Keep waiting (the executor would also emit a UI notice here).
                    continue;
                }
            }
        };

        assert!(
            timeouts_observed >= 1,
            "expected at least one timeout cycle before the 200ms completion; got {}",
            timeouts_observed
        );
        match result {
            Some(SubagentCompletionEvent::Completed(r)) => {
                assert_eq!(r.summary, "late but here");
            }
            other => panic!("expected Completed after re-arm, got {:?}", other),
        }
    }

    // The completion-injection format is what the executor pastes into the
    // orchestrator's next User message. Make sure the agent_id and summary
    // are present in the right shape — drift here breaks the orchestrator's
    // parsing.
    #[test]
    fn completion_injection_format_contains_agent_id_and_summary() {
        let r = make_result("worker-42", "claude-haiku-4-5", "found three TODOs");
        let block = r.format_completion_block();
        assert!(block.starts_with("[Sub-agent 'worker-42' completed]"));
        assert!(block.contains("found three TODOs"));
        // No blocked_on tail when blocked_on is empty.
        assert!(!block.contains("blocked on"));
    }

    #[test]
    fn completion_injection_format_appends_blocked_writes_tail() {
        let r = SubagentResult {
            agent_id: "w".into(),
            model: "m".into(),
            summary: "did some stuff".into(),
            notes: None,
            blocked_on: vec![BlockedWrite {
                path: "src/secret.rs".into(),
                reason: "outside writes scope".into(),
            }],
        };
        let block = r.format_completion_block();
        assert!(block.contains("[Sub-agent 'w' blocked on 1 write(s)]"));
        assert!(block.contains("src/secret.rs"));
        assert!(block.contains("outside writes scope"));
        assert!(block.contains("do these writes yourself"));
    }
}

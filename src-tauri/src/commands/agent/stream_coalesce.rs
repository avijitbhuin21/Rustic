//! Per-task stream-delta coalescer.
//!
//! Buffers `TextDelta` / `ThinkingDelta` / `SubagentTextDelta` into per-key
//! strings, flushing on a 24ms timer or on explicit flush before non-delta
//! events, to reduce per-token Tauri IPC overhead.

use std::collections::HashMap;
use crate::sync_ext::MutexExt;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::{AppHandle, Emitter};

use super::{
    AgentStreamEvent, AgentSubagentTextDeltaEvent, AgentThinkingDeltaEvent,
};

const FLUSH_MS: u64 = 24;

struct State {
    text: HashMap<String, String>,
    thinking: HashMap<String, String>,
    subagent: HashMap<(String, String), String>,
    /// True while a flush task is parked on `tokio::time::sleep`. We only
    /// ever want one flush task in flight per coalescer — it drains
    /// everything when it wakes.
    flush_pending: bool,
}

#[derive(Clone)]
pub(super) struct StreamCoalescer {
    inner: Arc<Mutex<State>>,
    app: AppHandle,
}

impl StreamCoalescer {
    pub fn new(app: AppHandle) -> Self {
        Self {
            inner: Arc::new(Mutex::new(State {
                text: HashMap::new(),
                thinking: HashMap::new(),
                subagent: HashMap::new(),
                flush_pending: false,
            })),
            app,
        }
    }

    pub fn push_text(&self, task_id: String, text: String) {
        let mut s = self.inner.lock_safe();
        s.text.entry(task_id).or_default().push_str(&text);
        self.ensure_flush(&mut s);
    }

    pub fn push_thinking(&self, task_id: String, text: String) {
        let mut s = self.inner.lock_safe();
        s.thinking.entry(task_id).or_default().push_str(&text);
        self.ensure_flush(&mut s);
    }

    pub fn push_subagent(&self, task_id: String, agent_id: String, text: String) {
        let mut s = self.inner.lock_safe();
        s.subagent
            .entry((task_id, agent_id))
            .or_default()
            .push_str(&text);
        self.ensure_flush(&mut s);
    }

    /// Flush every pending delta tied to `task_id` (text + thinking +
    /// every sub-agent under this task). Call this BEFORE emitting a
    /// non-delta event for the same task so the model's output order is
    /// preserved on the wire.
    pub fn flush_task(&self, task_id: &str) {
        let (text, thinking, subagent_entries) = {
            let mut s = self.inner.lock_safe();
            let text = s.text.remove(task_id);
            let thinking = s.thinking.remove(task_id);
            let mut subagent_entries: Vec<((String, String), String)> = Vec::new();
            let keys: Vec<(String, String)> = s
                .subagent
                .keys()
                .filter(|(t, _)| t == task_id)
                .cloned()
                .collect();
            for k in keys {
                if let Some(v) = s.subagent.remove(&k) {
                    subagent_entries.push((k, v));
                }
            }
            (text, thinking, subagent_entries)
        };
        if let Some(text) = text {
            let _ = self.app.emit_to("main",
                "agent-stream",
                AgentStreamEvent {
                    task_id: task_id.to_string(),
                    text,
                },
            );
        }
        if let Some(text) = thinking {
            let _ = self.app.emit_to("main",
                "agent-thinking-delta",
                AgentThinkingDeltaEvent {
                    task_id: task_id.to_string(),
                    text,
                },
            );
        }
        for ((task_id, agent_id), text) in subagent_entries {
            let _ = self.app.emit_to("main",
                "agent-subagent-text-delta",
                AgentSubagentTextDeltaEvent {
                    task_id,
                    agent_id,
                    text,
                },
            );
        }
    }

    /// Drain everything currently buffered. Call once after the
    /// event-forward loop exits so we don't lose the trailing delta.
    pub fn flush_all(&self) {
        let (texts, thinkings, subagents) = {
            let mut s = self.inner.lock_safe();
            let texts: Vec<(String, String)> = s.text.drain().collect();
            let thinkings: Vec<(String, String)> = s.thinking.drain().collect();
            let subagents: Vec<((String, String), String)> = s.subagent.drain().collect();
            (texts, thinkings, subagents)
        };
        for (task_id, text) in texts {
            let _ = self
                .app
                .emit("agent-stream", AgentStreamEvent { task_id, text });
        }
        for (task_id, text) in thinkings {
            let _ = self.app.emit_to("main",
                "agent-thinking-delta",
                AgentThinkingDeltaEvent { task_id, text },
            );
        }
        for ((task_id, agent_id), text) in subagents {
            let _ = self.app.emit_to("main",
                "agent-subagent-text-delta",
                AgentSubagentTextDeltaEvent {
                    task_id,
                    agent_id,
                    text,
                },
            );
        }
    }

    fn ensure_flush(&self, s: &mut State) {
        if s.flush_pending {
            return;
        }
        s.flush_pending = true;
        let me = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(FLUSH_MS)).await;
            // Atomically clear the flag + take the pending entries so the
            // next `push_*` schedules a fresh flush.
            let (texts, thinkings, subagents) = {
                let mut s = me.inner.lock_safe();
                s.flush_pending = false;
                let texts: Vec<(String, String)> = s.text.drain().collect();
                let thinkings: Vec<(String, String)> = s.thinking.drain().collect();
                let subagents: Vec<((String, String), String)> =
                    s.subagent.drain().collect();
                (texts, thinkings, subagents)
            };
            for (task_id, text) in texts {
                let _ = me
                    .app
                    .emit("agent-stream", AgentStreamEvent { task_id, text });
            }
            for (task_id, text) in thinkings {
                let _ = me.app.emit_to("main",
                    "agent-thinking-delta",
                    AgentThinkingDeltaEvent { task_id, text },
                );
            }
            for ((task_id, agent_id), text) in subagents {
                let _ = me.app.emit_to("main",
                    "agent-subagent-text-delta",
                    AgentSubagentTextDeltaEvent {
                        task_id,
                        agent_id,
                        text,
                    },
                );
            }
        });
    }
}

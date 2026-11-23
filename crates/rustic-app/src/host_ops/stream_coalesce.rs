//! Host-side stream-delta batching.
//!
//! The executor emits one `TaskEvent` per provider token. Forwarding each one
//! as its own IPC event costs a JSON encode + bridge hop + listener dispatch
//! per token. `batch_deltas` sits between the executor channel and the host's
//! forward loop and merges runs of consecutive same-key deltas (text,
//! thinking, tool-input JSON, sub-agent text/thinking) into one event, flushed
//! after `FLUSH_MS` or as soon as any other event arrives. Because merging only
//! ever joins *adjacent* events, wire order is preserved exactly; payload
//! shapes are unchanged (strings are simply longer).
//!
//! Used by both hosts (src-tauri agent forward loop and rustic-server agent_chat).
//!
//! `SubagentOutputWriter` batches further on the storage side: the merged
//! sub-agent text is written to the database a few times a second.

use std::time::Duration;

use rustic_agent::TaskEvent;
use tokio::sync::mpsc::{self, Receiver};
use tokio::time::Instant;

const FLUSH_MS: u64 = 24;

/// Appends `next` onto `pending` when both are the same delta kind for the
/// same key. Returns `next` back when it can't be merged.
#[allow(clippy::result_large_err)] // Err hands the unmerged event back by value
fn try_merge(pending: &mut TaskEvent, next: TaskEvent) -> Result<(), TaskEvent> {
    use TaskEvent::*;
    match (pending, next) {
        (
            TextDelta { task_id: a, text },
            TextDelta {
                task_id: b,
                text: t,
            },
        ) if *a == b => {
            text.push_str(&t);
            Ok(())
        }
        (
            ThinkingDelta { task_id: a, text },
            ThinkingDelta {
                task_id: b,
                text: t,
            },
        ) if *a == b => {
            text.push_str(&t);
            Ok(())
        }
        (
            ToolUseInputDelta {
                task_id: a,
                tool_use_id: ai,
                partial_json,
            },
            ToolUseInputDelta {
                task_id: b,
                tool_use_id: bi,
                partial_json: p,
            },
        ) if *a == b && *ai == bi => {
            partial_json.push_str(&p);
            Ok(())
        }
        (
            SubagentTextDelta {
                task_id: a,
                agent_id: ai,
                text,
            },
            SubagentTextDelta {
                task_id: b,
                agent_id: bi,
                text: t,
            },
        ) if *a == b && *ai == bi => {
            text.push_str(&t);
            Ok(())
        }
        (
            SubagentThinkingDelta {
                task_id: a,
                agent_id: ai,
                text,
            },
            SubagentThinkingDelta {
                task_id: b,
                agent_id: bi,
                text: t,
            },
        ) if *a == b && *ai == bi => {
            text.push_str(&t);
            Ok(())
        }
        (_, next) => Err(next),
    }
}

fn is_delta(ev: &TaskEvent) -> bool {
    matches!(
        ev,
        TaskEvent::TextDelta { .. }
            | TaskEvent::ThinkingDelta { .. }
            | TaskEvent::ToolUseInputDelta { .. }
            | TaskEvent::SubagentTextDelta { .. }
            | TaskEvent::SubagentThinkingDelta { .. }
    )
}

/// Wraps the executor's event receiver; the returned receiver yields the same
/// events in the same order with adjacent same-key deltas coalesced.
pub fn batch_deltas(mut rx: Receiver<TaskEvent>) -> Receiver<TaskEvent> {
    let (tx, out) = mpsc::channel::<TaskEvent>(rustic_agent::EVENT_CHANNEL_CAP);
    tokio::spawn(async move {
        let mut pending: Option<(TaskEvent, Instant)> = None;
        loop {
            let next = match &pending {
                Some((_, deadline)) => {
                    tokio::select! {
                        biased;
                        ev = rx.recv() => ev.map(Some),
                        _ = tokio::time::sleep_until(*deadline) => Some(None),
                    }
                }
                None => rx.recv().await.map(Some),
            };
            match next {
                // Channel closed: flush and stop.
                None => {
                    if let Some((ev, _)) = pending.take() {
                        let _ = tx.send(ev).await;
                    }
                    break;
                }
                // Flush timer fired.
                Some(None) => {
                    if let Some((ev, _)) = pending.take() {
                        if tx.send(ev).await.is_err() {
                            break;
                        }
                    }
                }
                Some(Some(ev)) => {
                    let ev = match pending.as_mut() {
                        Some((p, _)) => match try_merge(p, ev) {
                            Ok(()) => continue,
                            Err(ev) => {
                                if let Some((flushed, _)) = pending.take() {
                                    if tx.send(flushed).await.is_err() {
                                        break;
                                    }
                                }
                                ev
                            }
                        },
                        None => ev,
                    };
                    if is_delta(&ev) {
                        pending = Some((ev, Instant::now() + Duration::from_millis(FLUSH_MS)));
                    } else if tx.send(ev).await.is_err() {
                        break;
                    }
                }
            }
        }
    });
    out
}

/// How long streamed sub-agent text may wait before it is written.
const OUTPUT_FLUSH_AFTER: Duration = Duration::from_millis(300);
/// Buffered sub-agent text is written once it reaches this size, whatever its age.
const OUTPUT_FLUSH_BYTES: usize = 4096;

struct PendingOutput {
    task_id: String,
    agent_id: String,
    text: String,
    since: Instant,
}

/// Persists streamed sub-agent text in batches.
///
/// Storing each delta appends to one growing column, and SQLite rewrites the
/// whole value every time: a long sub-agent reply cost one full rewrite per
/// delta, many times a second, each under the database lock. Text is held
/// here instead and written through `sink(task_id, agent_id, text)` when it is
/// 300 ms old or 4 KB long, before any other persisted event of the same
/// sub-agent (so stored text and tool calls stay in order), and when the
/// writer is dropped at the end of the run. A crash loses at most that tail.
/// Writes one batch: `(task_id, agent_id, text)`.
type OutputSink = Box<dyn FnMut(&str, &str, &str) + Send>;

pub struct SubagentOutputWriter {
    sink: OutputSink,
    pending: Vec<PendingOutput>,
}

impl SubagentOutputWriter {
    pub fn new(sink: impl FnMut(&str, &str, &str) + Send + 'static) -> Self {
        Self {
            sink: Box::new(sink),
            pending: Vec::new(),
        }
    }

    /// Buffer streamed text of one sub-agent.
    pub fn push(&mut self, task_id: &str, agent_id: &str, text: &str) {
        if text.is_empty() {
            return;
        }
        match self
            .pending
            .iter_mut()
            .find(|p| p.task_id == task_id && p.agent_id == agent_id)
        {
            Some(p) => p.text.push_str(text),
            None => self.pending.push(PendingOutput {
                task_id: task_id.to_string(),
                agent_id: agent_id.to_string(),
                text: text.to_string(),
                since: Instant::now(),
            }),
        }
    }

    /// Write whatever is due. Call before handling each event of the run.
    pub fn before(&mut self, next: &TaskEvent) {
        self.flush_due(next, Instant::now());
    }

    fn flush_due(&mut self, next: &TaskEvent, now: Instant) {
        use TaskEvent::*;
        // Events whose own write must land after the sub-agent's buffered text.
        let settles = match next {
            SubagentCompleted {
                task_id, agent_id, ..
            }
            | SubagentFailed {
                task_id, agent_id, ..
            }
            | SubagentToolUse {
                task_id, agent_id, ..
            }
            | SubagentToolResult {
                task_id, agent_id, ..
            } => Some((task_id.as_str(), agent_id.as_str())),
            _ => None,
        };
        let mut i = 0;
        while i < self.pending.len() {
            let p = &self.pending[i];
            let due = p.text.len() >= OUTPUT_FLUSH_BYTES
                || now.duration_since(p.since) >= OUTPUT_FLUSH_AFTER
                || settles == Some((p.task_id.as_str(), p.agent_id.as_str()));
            if due {
                let p = self.pending.remove(i);
                (self.sink)(&p.task_id, &p.agent_id, &p.text);
            } else {
                i += 1;
            }
        }
    }
}

impl Drop for SubagentOutputWriter {
    fn drop(&mut self) {
        for p in std::mem::take(&mut self.pending) {
            (self.sink)(&p.task_id, &p.agent_id, &p.text);
        }
    }
}

#[cfg(test)]
mod output_writer_tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    type Written = Arc<Mutex<Vec<(String, String, String)>>>;

    fn writer() -> (SubagentOutputWriter, Written) {
        let written: Written = Arc::default();
        let sink = Arc::clone(&written);
        let w = SubagentOutputWriter::new(move |t, a, text| {
            sink.lock().unwrap().push((t.into(), a.into(), text.into()));
        });
        (w, written)
    }

    fn other() -> TaskEvent {
        TaskEvent::TextDelta {
            task_id: "t".into(),
            text: "x".into(),
        }
    }

    fn tool_use(agent: &str) -> TaskEvent {
        TaskEvent::SubagentToolUse {
            task_id: "t".into(),
            agent_id: agent.into(),
            tool_name: "read_file".into(),
            tool_use_id: "u".into(),
            input: serde_json::json!({}),
        }
    }

    #[test]
    fn text_is_batched_until_it_is_old_or_large() {
        let (mut w, written) = writer();
        let start = Instant::now();
        for piece in ["a", "b", "c"] {
            w.push("t", "one", piece);
        }
        w.flush_due(&other(), start + Duration::from_millis(100));
        assert!(
            written.lock().unwrap().is_empty(),
            "young and small: still buffered"
        );

        w.flush_due(&other(), start + Duration::from_millis(400));
        assert_eq!(
            *written.lock().unwrap(),
            vec![("t".to_string(), "one".to_string(), "abc".to_string())],
            "one write for the three deltas"
        );

        w.push("t", "one", &"z".repeat(OUTPUT_FLUSH_BYTES));
        w.flush_due(&other(), Instant::now());
        assert_eq!(
            written.lock().unwrap().len(),
            2,
            "large text is written at once"
        );
    }

    #[test]
    fn text_lands_before_the_same_agents_next_persisted_event() {
        let (mut w, written) = writer();
        w.push("t", "one", "before the tool call");
        w.push("t", "two", "other agent");
        w.flush_due(&tool_use("one"), Instant::now());
        assert_eq!(
            *written.lock().unwrap(),
            vec![(
                "t".to_string(),
                "one".to_string(),
                "before the tool call".to_string()
            )],
            "only the agent whose event arrived is flushed"
        );
        w.flush_due(
            &TaskEvent::SubagentFailed {
                task_id: "t".into(),
                agent_id: "two".into(),
                error: "boom".into(),
            },
            Instant::now(),
        );
        assert_eq!(written.lock().unwrap().len(), 2);
    }

    #[test]
    fn nothing_is_lost_when_the_run_ends() {
        let (mut w, written) = writer();
        w.push("t", "one", "tail ");
        w.push("t", "one", "of the reply");
        w.push("t", "one", "");
        drop(w);
        assert_eq!(
            *written.lock().unwrap(),
            vec![(
                "t".to_string(),
                "one".to_string(),
                "tail of the reply".to_string()
            )]
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(t: &str) -> TaskEvent {
        TaskEvent::TextDelta {
            task_id: "t".into(),
            text: t.into(),
        }
    }

    #[tokio::test]
    async fn merges_adjacent_and_preserves_order() {
        let (tx, rx) = mpsc::channel(64);
        let mut out = batch_deltas(rx);
        tx.send(text("a")).await.unwrap();
        tx.send(text("b")).await.unwrap();
        tx.send(TaskEvent::ToolUseInputDelta {
            task_id: "t".into(),
            tool_use_id: "x".into(),
            partial_json: "{\"p".into(),
        })
        .await
        .unwrap();
        tx.send(TaskEvent::ToolUseInputDelta {
            task_id: "t".into(),
            tool_use_id: "x".into(),
            partial_json: "\":1}".into(),
        })
        .await
        .unwrap();
        tx.send(TaskEvent::ToolUseStop {
            task_id: "t".into(),
            tool_use_id: "x".into(),
        })
        .await
        .unwrap();
        tx.send(text("c")).await.unwrap();
        drop(tx);
        let mut got = Vec::new();
        while let Some(ev) = out.recv().await {
            got.push(ev);
        }
        assert_eq!(got.len(), 4);
        assert!(matches!(&got[0], TaskEvent::TextDelta { text, .. } if text == "ab"));
        assert!(
            matches!(&got[1], TaskEvent::ToolUseInputDelta { partial_json, .. } if partial_json == "{\"p\":1}")
        );
        assert!(matches!(&got[2], TaskEvent::ToolUseStop { .. }));
        assert!(matches!(&got[3], TaskEvent::TextDelta { text, .. } if text == "c"));
    }

    #[tokio::test]
    async fn does_not_merge_different_keys_and_flushes_on_timer() {
        let (tx, rx) = mpsc::channel(64);
        let mut out = batch_deltas(rx);
        tx.send(TaskEvent::SubagentTextDelta {
            task_id: "t".into(),
            agent_id: "a".into(),
            text: "1".into(),
        })
        .await
        .unwrap();
        tx.send(TaskEvent::SubagentTextDelta {
            task_id: "t".into(),
            agent_id: "b".into(),
            text: "2".into(),
        })
        .await
        .unwrap();
        // Keep the sender alive: the timer alone must flush.
        let first = out.recv().await.unwrap();
        let second = tokio::time::timeout(Duration::from_secs(2), out.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(first, TaskEvent::SubagentTextDelta { agent_id, .. } if agent_id == "a"));
        assert!(matches!(second, TaskEvent::SubagentTextDelta { agent_id, .. } if agent_id == "b"));
        drop(tx);
    }
}

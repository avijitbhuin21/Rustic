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

use std::time::Duration;

use rustic_agent::TaskEvent;
use tokio::sync::mpsc::{self, Receiver};
use tokio::time::Instant;

const FLUSH_MS: u64 = 24;

/// Appends `next` onto `pending` when both are the same delta kind for the
/// same key. Returns `next` back when it can't be merged.
fn try_merge(pending: &mut TaskEvent, next: TaskEvent) -> Result<(), TaskEvent> {
    use TaskEvent::*;
    match (pending, next) {
        (TextDelta { task_id: a, text }, TextDelta { task_id: b, text: t }) if *a == b => {
            text.push_str(&t);
            Ok(())
        }
        (ThinkingDelta { task_id: a, text }, ThinkingDelta { task_id: b, text: t }) if *a == b => {
            text.push_str(&t);
            Ok(())
        }
        (
            ToolUseInputDelta { task_id: a, tool_use_id: ai, partial_json },
            ToolUseInputDelta { task_id: b, tool_use_id: bi, partial_json: p },
        ) if *a == b && *ai == bi => {
            partial_json.push_str(&p);
            Ok(())
        }
        (
            SubagentTextDelta { task_id: a, agent_id: ai, text },
            SubagentTextDelta { task_id: b, agent_id: bi, text: t },
        ) if *a == b && *ai == bi => {
            text.push_str(&t);
            Ok(())
        }
        (
            SubagentThinkingDelta { task_id: a, agent_id: ai, text },
            SubagentThinkingDelta { task_id: b, agent_id: bi, text: t },
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
                                let (flushed, _) = pending.take().expect("pending checked");
                                if tx.send(flushed).await.is_err() {
                                    break;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn text(t: &str) -> TaskEvent {
        TaskEvent::TextDelta { task_id: "t".into(), text: t.into() }
    }

    #[tokio::test]
    async fn merges_adjacent_and_preserves_order() {
        let (tx, rx) = mpsc::channel(64);
        let mut out = batch_deltas(rx);
        tx.send(text("a")).await.unwrap();
        tx.send(text("b")).await.unwrap();
        tx.send(TaskEvent::ToolUseInputDelta { task_id: "t".into(), tool_use_id: "x".into(), partial_json: "{\"p".into() }).await.unwrap();
        tx.send(TaskEvent::ToolUseInputDelta { task_id: "t".into(), tool_use_id: "x".into(), partial_json: "\":1}".into() }).await.unwrap();
        tx.send(TaskEvent::ToolUseStop { task_id: "t".into(), tool_use_id: "x".into() }).await.unwrap();
        tx.send(text("c")).await.unwrap();
        drop(tx);
        let mut got = Vec::new();
        while let Some(ev) = out.recv().await {
            got.push(ev);
        }
        assert_eq!(got.len(), 4);
        assert!(matches!(&got[0], TaskEvent::TextDelta { text, .. } if text == "ab"));
        assert!(matches!(&got[1], TaskEvent::ToolUseInputDelta { partial_json, .. } if partial_json == "{\"p\":1}"));
        assert!(matches!(&got[2], TaskEvent::ToolUseStop { .. }));
        assert!(matches!(&got[3], TaskEvent::TextDelta { text, .. } if text == "c"));
    }

    #[tokio::test]
    async fn does_not_merge_different_keys_and_flushes_on_timer() {
        let (tx, rx) = mpsc::channel(64);
        let mut out = batch_deltas(rx);
        tx.send(TaskEvent::SubagentTextDelta { task_id: "t".into(), agent_id: "a".into(), text: "1".into() }).await.unwrap();
        tx.send(TaskEvent::SubagentTextDelta { task_id: "t".into(), agent_id: "b".into(), text: "2".into() }).await.unwrap();
        // Keep the sender alive: the timer alone must flush.
        let first = out.recv().await.unwrap();
        let second = tokio::time::timeout(Duration::from_secs(2), out.recv()).await.unwrap().unwrap();
        assert!(matches!(first, TaskEvent::SubagentTextDelta { agent_id, .. } if agent_id == "a"));
        assert!(matches!(second, TaskEvent::SubagentTextDelta { agent_id, .. } if agent_id == "b"));
        drop(tx);
    }
}

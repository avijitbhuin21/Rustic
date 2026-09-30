//! P0.2 — broker for the `ask_user` tool.
//!
//! Mirrors [`PermissionBroker`] but for a JSON-shaped response (the user's
//! answers map keyed by question id, plus an optional `cancelled` flag).
//! When the `ask_user` tool fires it:
//!
//! 1. Picks a fresh request_id.
//! 2. Inserts a oneshot sender into `pending`.
//! 3. Emits [`crate::task::TaskEvent::AskUserRequest`] so the host
//!    runtime can forward it to the frontend.
//! 4. Awaits the oneshot. The Tauri `respond_to_ask_user` command
//!    drains the matching entry and unblocks the tool with the JSON.
//!
//! Timeout matches `PermissionBroker` (24h) — long enough that the user
//! can step away and come back without the agent silently failing.

use crate::task::{EventTx, TaskEvent};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::oneshot;
use uuid::Uuid;

#[derive(Default)]
pub struct AskUserBroker {
    pending: Mutex<HashMap<String, oneshot::Sender<AskUserResponse>>>,
}

/// What the frontend sends back. `answers` is keyed by the question `id`
/// the agent provided in its tool call. `cancelled` lets the UI surface
/// the difference between "user picked Skip / Cancel" and "user actually
/// answered" — the tool result includes the flag so the agent can react.
/// `images` are any pictures the user attached to their answer; they ride
/// back to the model as image attachments on the tool result.
#[derive(Debug, Clone)]
pub struct AskUserResponse {
    pub answers: Value,
    pub cancelled: bool,
    pub images: Vec<crate::tools::ToolAttachment>,
}

impl AskUserBroker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Emit an `AskUserRequest` event and wait for the response. Returns
    /// `None` on timeout (24h) so the tool can surface a clean error
    /// rather than hanging the task forever.
    pub async fn request(
        &self,
        event_tx: &EventTx,
        task_id: &str,
        questions: Value,
    ) -> Option<AskUserResponse> {
        let request_id = Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().unwrap();
            pending.insert(request_id.clone(), tx);
        }
        let _ = event_tx.try_send(TaskEvent::AskUserRequest {
            task_id: task_id.to_string(),
            request_id: request_id.clone(),
            questions,
        });
        // If this future is dropped (the run was cancelled — e.g. a peer
        // message interrupted the turn), the guard removes the stale entry
        // and tells the UI to close the card, so it can't linger as a second
        // "running" popup whose answer would be silently lost.
        let mut guard = PendingGuard {
            broker: self,
            event_tx,
            task_id,
            request_id: &request_id,
            armed: true,
        };
        let out = match tokio::time::timeout(Duration::from_secs(86_400), rx).await {
            Ok(Ok(response)) => Some(response),
            _ => None,
        };
        if out.is_some() {
            guard.armed = false;
        }
        out
    }

    /// Resolve the pending request with the user's answers. Returns `false`
    /// when the request is no longer waiting (already answered, cancelled,
    /// timed out, or unknown) so the host can tell the user.
    pub fn respond(&self, request_id: &str, response: AskUserResponse) -> bool {
        let sender = {
            let mut pending = self.pending.lock().unwrap();
            pending.remove(request_id)
        };
        match sender {
            Some(tx) => tx.send(response).is_ok(),
            None => false,
        }
    }

    /// Number of requests still waiting for an answer.
    pub fn pending_count(&self) -> usize {
        self.pending.lock().unwrap().len()
    }
}

/// Cleans up a pending ask_user entry when its waiting future ends without an answer.
struct PendingGuard<'a> {
    broker: &'a AskUserBroker,
    event_tx: &'a EventTx,
    task_id: &'a str,
    request_id: &'a str,
    armed: bool,
}

impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let removed = self
            .broker
            .pending
            .lock()
            .map(|mut p| p.remove(self.request_id).is_some())
            .unwrap_or(false);
        if removed {
            let _ = self.event_tx.try_send(TaskEvent::AskUserCancelled {
                task_id: self.task_id.to_string(),
                request_id: self.request_id.to_string(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn respond_unblocks_pending_request() {
        let broker = std::sync::Arc::new(AskUserBroker::new());
        let (tx, mut rx) = tokio::sync::mpsc::channel::<TaskEvent>(16);
        let broker_for_task = broker.clone();
        let handle = tokio::spawn(async move {
            broker_for_task
                .request(
                    &tx,
                    "task-1",
                    json!([{"id": "q1", "text": "?", "kind": "free_text"}]),
                )
                .await
        });
        // Grab the request_id from the emitted event.
        let ev = rx.recv().await.expect("event");
        let request_id = match ev {
            TaskEvent::AskUserRequest { request_id, .. } => request_id,
            _ => panic!("expected AskUserRequest"),
        };
        broker.respond(
            &request_id,
            AskUserResponse {
                answers: json!({ "q1": "hello" }),
                cancelled: false,
                images: Vec::new(),
            },
        );
        let resp = handle.await.expect("join").expect("response");
        assert_eq!(resp.answers["q1"], "hello");
        assert!(!resp.cancelled);
    }

    #[tokio::test]
    async fn dropped_request_cleans_up_and_late_answer_is_not_delivered() {
        let broker = std::sync::Arc::new(AskUserBroker::new());
        let (tx, mut rx) = tokio::sync::mpsc::channel::<TaskEvent>(16);
        let broker_for_task = broker.clone();
        let handle = tokio::spawn(async move {
            broker_for_task
                .request(
                    &tx,
                    "task-1",
                    json!([{"id": "q1", "text": "?", "kind": "free_text"}]),
                )
                .await
        });
        let request_id = match rx.recv().await.expect("event") {
            TaskEvent::AskUserRequest { request_id, .. } => request_id,
            _ => panic!("expected AskUserRequest"),
        };
        assert_eq!(broker.pending_count(), 1);
        handle.abort();
        let _ = handle.await;
        match rx.recv().await.expect("cancel event") {
            TaskEvent::AskUserCancelled {
                request_id: id,
                task_id,
            } => {
                assert_eq!(id, request_id);
                assert_eq!(task_id, "task-1");
            }
            _ => panic!("expected AskUserCancelled"),
        }
        assert_eq!(broker.pending_count(), 0, "stale entry must be removed");
        let delivered = broker.respond(
            &request_id,
            AskUserResponse {
                answers: json!({"q1": "late"}),
                cancelled: false,
                images: Vec::new(),
            },
        );
        assert!(
            !delivered,
            "answer to an interrupted question must report non-delivery"
        );
    }
}

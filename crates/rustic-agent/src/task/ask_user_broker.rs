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
#[derive(Debug, Clone)]
pub struct AskUserResponse {
    pub answers: Value,
    pub cancelled: bool,
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
        match tokio::time::timeout(Duration::from_secs(86_400), rx).await {
            Ok(Ok(response)) => Some(response),
            _ => {
                let mut pending = self.pending.lock().unwrap();
                pending.remove(&request_id);
                None
            }
        }
    }

    /// Resolve the pending request with the user's answers. Silently
    /// no-ops if the request_id isn't pending (already responded, timed
    /// out, or never existed).
    pub fn respond(&self, request_id: &str, response: AskUserResponse) {
        let sender = {
            let mut pending = self.pending.lock().unwrap();
            pending.remove(request_id)
        };
        if let Some(tx) = sender {
            let _ = tx.send(response);
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
                .request(&tx, "task-1", json!([{"id": "q1", "text": "?", "kind": "free_text"}]))
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
            },
        );
        let resp = handle.await.expect("join").expect("response");
        assert_eq!(resp.answers["q1"], "hello");
        assert!(!resp.cancelled);
    }
}

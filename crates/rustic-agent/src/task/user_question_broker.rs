use crate::task::{EventTx, TaskEvent};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::oneshot;
use uuid::Uuid;

/// Mediates ask_user tool calls — the agent pauses and waits for user input.
///
/// When the agent calls `ask_user`, this broker:
///   1. Emits a `TaskEvent::UserQuestionRequest` to the UI
///   2. Suspends until the user types a response (waits up to 24 hours)
///
/// The Tauri `respond_to_question` command calls `respond()` to unblock the tool.
///
/// Each pending request also records the `task_id` it belongs to so a
/// follow-up `respond_all_for_task` can release every blocked question for
/// that task at once. This is the unblock path used when the user sends a
/// fresh message in a chat that's still waiting on an earlier question —
/// without it the orphaned tool would sit in `broker.ask` for up to 24h
/// while the new run_turn races alongside it on the same `task_id`.
pub struct UserQuestionBroker {
    pending: Mutex<HashMap<String, (String, oneshot::Sender<String>)>>,
}

impl UserQuestionBroker {
    pub fn new() -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
        }
    }

    /// Emit a question to the user and wait for their text response.
    /// Returns the user's answer, or an error string on timeout.
    pub async fn ask(
        &self,
        event_tx: &EventTx,
        task_id: &str,
        question: &str,
        choices: Vec<String>,
    ) -> Result<String, String> {
        let request_id = Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();

        {
            let mut pending = self.pending.lock().unwrap();
            pending.insert(request_id.clone(), (task_id.to_string(), tx));
        }

        let _ = event_tx.send(TaskEvent::UserQuestionRequest {
            task_id: task_id.to_string(),
            request_id: request_id.clone(),
            question: question.to_string(),
            choices,
        });

        // Wait indefinitely (24h timeout as safety net) — user may be away
        match tokio::time::timeout(Duration::from_secs(86400), rx).await {
            Ok(Ok(answer)) => Ok(answer),
            _ => {
                let mut pending = self.pending.lock().unwrap();
                pending.remove(&request_id);
                Err("QUESTION_TIMEOUT: User did not respond in time.".to_string())
            }
        }
    }

    /// Called by `respond_to_question` Tauri command to resolve a pending question.
    pub fn respond(&self, request_id: &str, answer: String) {
        let mut pending = self.pending.lock().unwrap();
        if let Some((_, tx)) = pending.remove(request_id) {
            let _ = tx.send(answer);
        }
    }

    /// Respond to every pending question owned by `task_id` with the same
    /// `answer`. Used when a fresh `send_message` arrives for a task that
    /// still has an older `chat_message` question in flight — the old
    /// run_turn is unblocked so it can unwind cleanly instead of running
    /// in parallel with the new one. Returns the number of senders released.
    pub fn respond_all_for_task(&self, task_id: &str, answer: String) -> usize {
        let mut pending = self.pending.lock().unwrap();
        let to_remove: Vec<String> = pending
            .iter()
            .filter(|(_, (tid, _))| tid == task_id)
            .map(|(rid, _)| rid.clone())
            .collect();
        let count = to_remove.len();
        for rid in to_remove {
            if let Some((_, tx)) = pending.remove(&rid) {
                let _ = tx.send(answer.clone());
            }
        }
        count
    }
}

impl Default for UserQuestionBroker {
    fn default() -> Self {
        Self::new()
    }
}

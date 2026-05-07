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
pub struct UserQuestionBroker {
    pending: Mutex<HashMap<String, oneshot::Sender<String>>>,
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
            pending.insert(request_id.clone(), tx);
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
        if let Some(tx) = pending.remove(request_id) {
            let _ = tx.send(answer);
        }
    }
}

impl Default for UserQuestionBroker {
    fn default() -> Self {
        Self::new()
    }
}

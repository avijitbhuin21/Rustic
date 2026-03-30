use crate::task::{EventTx, PermissionOp, TaskEvent};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::oneshot;
use uuid::Uuid;

/// Mediates per-operation approval in ManualEdit / AutoEdit modes.
///
/// When a tool needs user approval, it calls `request()`, which:
///   1. Emits a `TaskEvent::PermissionRequest` to the UI
///   2. Suspends until the user responds (or 60 s auto-deny fires)
///
/// The Tauri `respond_to_permission` command calls `respond()` to unblock the tool.
pub struct PermissionBroker {
    pending: Mutex<HashMap<String, oneshot::Sender<bool>>>,
}

impl PermissionBroker {
    pub fn new() -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
        }
    }

    /// Emit a permission request and wait for the user to approve or deny.
    /// Returns `true` if approved, `false` if denied or timed out (60 s).
    pub async fn request(&self, event_tx: &EventTx, task_id: &str, op: PermissionOp) -> bool {
        let request_id = Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();

        {
            let mut pending = self.pending.lock().unwrap();
            pending.insert(request_id.clone(), tx);
        }

        let (operation, description, preview) = op.describe();
        let _ = event_tx.send(TaskEvent::PermissionRequest {
            task_id: task_id.to_string(),
            request_id: request_id.clone(),
            operation,
            description,
            preview,
        });

        match tokio::time::timeout(Duration::from_secs(60), rx).await {
            Ok(Ok(approved)) => approved,
            _ => {
                // Timeout or channel closed — auto-deny and clean up
                let mut pending = self.pending.lock().unwrap();
                pending.remove(&request_id);
                false
            }
        }
    }

    /// Called by `respond_to_permission` Tauri command to resolve a pending request.
    pub fn respond(&self, request_id: &str, approved: bool) {
        let mut pending = self.pending.lock().unwrap();
        if let Some(tx) = pending.remove(request_id) {
            let _ = tx.send(approved);
        }
    }
}

impl Default for PermissionBroker {
    fn default() -> Self {
        Self::new()
    }
}

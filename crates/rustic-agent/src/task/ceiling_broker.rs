//! P0.4 fix #4 — broker for daily-cost-ceiling-breached pauses.
//!
//! Mirrors [`crate::task::ask_user_broker::AskUserBroker`] but resolves to
//! one of two enum variants: bump the ceiling to a new cents value, or
//! stop the task. When the executor detects the daily ceiling has been
//! reached at the top of a new turn, it:
//!
//! 1. Picks a fresh request_id.
//! 2. Inserts a oneshot sender into `pending`.
//! 3. Emits [`crate::task::TaskEvent::CeilingBreached`] so the host
//!    runtime can render the "Raise ceiling or stop" modal.
//! 4. Awaits the oneshot. The Tauri `respond_to_ceiling_breach` command
//!    drains the matching entry and unblocks the awaiting turn.
//!
//! Timeout matches the other interactive brokers (24h) — long enough that
//! the user can step away and decide later.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::oneshot;

/// How the user chose to resolve the breach. `RaiseTo(cents)` means the
/// task should retry the same turn after the host has bumped its budget
/// settings to `cents`; `Stop` means the task should fail as it does
/// today (no resume).
#[derive(Debug, Clone, Copy)]
pub enum CeilingResolution {
    /// New ceiling in cents. Caller is responsible for plumbing this
    /// into the persisted `BudgetSettings` AND into the in-memory
    /// `Budget` instance the executor sees on the next turn.
    RaiseTo(u64),
    /// User chose to stop the task. Executor fails the turn as today.
    Stop,
}

#[derive(Default)]
pub struct CeilingBroker {
    pending: Mutex<HashMap<String, oneshot::Sender<CeilingResolution>>>,
}

impl CeilingBroker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a fresh request_id and return its oneshot receiver. The
    /// executor emits the `CeilingBreached` event with the same id and
    /// awaits the receiver. Returns `None` on 24h timeout so the
    /// executor can fail the task with a clean error rather than hang.
    pub async fn wait_for_resolution(&self, request_id: &str) -> Option<CeilingResolution> {
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().unwrap();
            pending.insert(request_id.to_string(), tx);
        }
        match tokio::time::timeout(Duration::from_secs(86_400), rx).await {
            Ok(Ok(resolution)) => Some(resolution),
            _ => {
                let mut pending = self.pending.lock().unwrap();
                pending.remove(request_id);
                None
            }
        }
    }

    /// Resolve the pending request. Silently no-ops if the id isn't
    /// pending (already responded, timed out, or never existed).
    pub fn respond(&self, request_id: &str, resolution: CeilingResolution) {
        let sender = {
            let mut pending = self.pending.lock().unwrap();
            pending.remove(request_id)
        };
        if let Some(tx) = sender {
            let _ = tx.send(resolution);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn respond_unblocks_pending_request() {
        let broker = std::sync::Arc::new(CeilingBroker::new());
        let broker_for_task = broker.clone();
        let handle = tokio::spawn(async move {
            broker_for_task.wait_for_resolution("req-1").await
        });
        // Yield so the task installs the oneshot sender before we respond.
        tokio::task::yield_now().await;
        broker.respond("req-1", CeilingResolution::RaiseTo(5000));
        let resp = handle.await.expect("join").expect("resolution");
        match resp {
            CeilingResolution::RaiseTo(c) => assert_eq!(c, 5000),
            other => panic!("expected RaiseTo, got {other:?}"),
        }
    }
}

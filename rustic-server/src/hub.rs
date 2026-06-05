//! The WebSocket event hub.
//!
//! `ServerContext::emit` publishes onto a tokio `broadcast` channel; every
//! `/ws` connection subscribes to it and forwards messages to its browser tab.
//! Using `broadcast` (not `mpsc`) means a single emit fans out to *all*
//! connected tabs — the multi-tab requirement from the plan — for free.

use serde::Serialize;
use tokio::sync::broadcast;

/// One server→client event: the event name the frontend's `listen()` filters
/// on, plus its JSON payload.
#[derive(Clone, Debug, Serialize)]
pub struct EventMsg {
    pub event: String,
    pub payload: serde_json::Value,
}

/// Clonable handle to the broadcast channel. Cheap to clone (`Arc` inside).
#[derive(Clone)]
pub struct EventHub {
    tx: broadcast::Sender<EventMsg>,
}

impl EventHub {
    /// Create a hub with the given channel capacity. A slow/absent subscriber
    /// that lags past `capacity` messages gets a `Lagged` error on recv (handled
    /// in the ws loop by skipping ahead) rather than blocking publishers.
    pub fn new(capacity: usize) -> Self {
        let (tx, _rx) = broadcast::channel(capacity);
        Self { tx }
    }

    /// Publish an event. Returns silently if there are no subscribers (the
    /// desktop's `app.emit` is likewise best-effort when no window listens).
    pub fn publish(&self, event: &str, payload: serde_json::Value) {
        let _ = self.tx.send(EventMsg {
            event: event.to_string(),
            payload,
        });
    }

    /// Subscribe a new `/ws` connection.
    pub fn subscribe(&self) -> broadcast::Receiver<EventMsg> {
        self.tx.subscribe()
    }
}

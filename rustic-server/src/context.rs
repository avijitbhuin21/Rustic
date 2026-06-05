//! `ServerContext` — the server's implementation of `rustic_app::AppContext`.

use std::path::PathBuf;
use std::sync::Arc;

use rustic_app::context::{AppContext, EventEmitter};
use rustic_app::secrets::SecretStore;
use rustic_app::state::AppState;

use crate::browser::BrowserManager;
use crate::hub::EventHub;

/// Holds everything the shared command bodies need on the server side. Cloning
/// is cheap — every field is an `Arc` or a small handle.
#[derive(Clone)]
pub struct ServerContext {
    pub state: Arc<AppState>,
    pub hub: EventHub,
    pub data_dir: PathBuf,
    pub home_dir: PathBuf,
    pub secrets: Arc<dyn SecretStore>,
    /// The embedded VM browser's Chromium lifecycle manager. Server-only — the
    /// desktop build has a real local browser and never constructs this.
    pub browser: Arc<BrowserManager>,
}

impl EventEmitter for ServerContext {
    fn emit_json(&self, event: &str, payload: serde_json::Value) {
        self.hub.publish(event, payload);
    }
}

impl AppContext for ServerContext {
    fn data_dir(&self) -> PathBuf {
        self.data_dir.clone()
    }
    fn home_dir(&self) -> PathBuf {
        self.home_dir.clone()
    }
    fn state(&self) -> &Arc<AppState> {
        &self.state
    }
    fn secrets(&self) -> &dyn SecretStore {
        &*self.secrets
    }
}

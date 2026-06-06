//! `ServerContext` — the server's implementation of `rustic_app::AppContext`.

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use rustic_app::context::{AppContext, EventEmitter};
use rustic_app::secrets::SecretStore;
use rustic_app::state::AppState;
use serde::{Deserialize, Serialize};

use crate::browser::BrowserManager;
use crate::hub::EventHub;

/// Live port-forwarding tunnel configuration. Settable at runtime from the
/// Settings UI (persisted under the `tunnel_config` settings key) so the host
/// router + login cookie pick up changes without a restart.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelConfig {
    /// `"path"` (default), `"subdomain"`, or `"cloudflare"`.
    pub mode: String,
    /// Wildcard preview domain for subdomain mode (e.g. `preview.example.com`).
    pub preview_domain: Option<String>,
    /// Parent domain to scope the session cookie to (e.g. `.example.com`).
    pub cookie_domain: Option<String>,
}

impl Default for TunnelConfig {
    fn default() -> Self {
        Self {
            mode: "path".to_string(),
            preview_domain: None,
            cookie_domain: None,
        }
    }
}

impl TunnelConfig {
    /// The preview domain to use for host routing / advertise to the frontend —
    /// only meaningful in subdomain mode with a non-empty domain.
    pub fn active_preview_domain(&self) -> Option<String> {
        if self.mode == "subdomain" {
            self.preview_domain.clone().filter(|s| !s.is_empty())
        } else {
            None
        }
    }

    /// The cookie domain to scope the session cookie to — only in subdomain mode
    /// (so the cookie reaches `<port>.preview.example.com`).
    pub fn active_cookie_domain(&self) -> Option<String> {
        if self.mode == "subdomain" {
            self.cookie_domain.clone().filter(|s| !s.is_empty())
        } else {
            None
        }
    }
}

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
    /// Live tunnel config (mode + preview/cookie domains), mutated at runtime by
    /// the `set_tunnel_config` command and read by the host router + login.
    pub tunnel: Arc<RwLock<TunnelConfig>>,
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

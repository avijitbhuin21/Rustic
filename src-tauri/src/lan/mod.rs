//! Desktop wiring for peer sync. The implementation lives in
//! `rustic_app::peer` (shared with rustic-server); this module re-exports it
//! and provides [`DesktopPeerHost`].

pub use rustic_app::peer::*;

use std::path::PathBuf;
use std::sync::Arc;

use tauri::{AppHandle, Manager};

use crate::transport::{KeychainSecretStore, TauriEmitter};

/// [`PeerHost`] for the Tauri app: app-data dir, OS keychain, Tauri events.
pub struct DesktopPeerHost {
    app: AppHandle,
    secrets: KeychainSecretStore,
}

impl DesktopPeerHost {
    /// Shared host handle for `app`.
    pub fn arc(app: &AppHandle) -> HostRef {
        Arc::new(DesktopPeerHost {
            app: app.clone(),
            secrets: KeychainSecretStore,
        })
    }
}

impl PeerHost for DesktopPeerHost {
    fn state(&self) -> &crate::state::AppState {
        self.app.state::<crate::state::AppState>().inner()
    }

    fn data_dir(&self) -> Result<PathBuf, String> {
        crate::app_paths::app_data_dir(&self.app).map_err(|e| e.to_string())
    }

    fn home_dir(&self) -> PathBuf {
        self.app
            .path()
            .home_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
    }

    fn emitter(&self) -> Arc<dyn rustic_app::EventEmitter> {
        Arc::new(TauriEmitter::new(self.app.clone()))
    }

    fn secrets(&self) -> &dyn rustic_app::secrets::SecretStore {
        &self.secrets
    }
}

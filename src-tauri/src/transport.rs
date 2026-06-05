//! Desktop-side implementations of the `rustic-app` transport traits.
//!
//! These are the Tauri half of the `AppContext` seam: events go out through
//! `AppHandle::emit`, secrets live in the OS keychain. The server half lives in
//! the `rustic-server` crate. Command bodies that have been migrated to the
//! shared layer run against either, unchanged.

use rustic_app::context::EventEmitter;
use rustic_app::secrets::SecretStore;
use tauri::{AppHandle, Emitter};

/// Emits events to the desktop webview via `AppHandle::emit`. Wrapped in an
/// `Arc<dyn EventEmitter>` and handed to the filesystem watcher (and any other
/// shared component that only needs to push events).
#[derive(Clone)]
pub struct TauriEmitter {
    app: AppHandle,
}

impl TauriEmitter {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

impl EventEmitter for TauriEmitter {
    fn emit_json(&self, event: &str, payload: serde_json::Value) {
        // Mirrors the historical `app.emit(event, payload)` — `serde_json::Value`
        // is `Serialize`, so this serializes identically to the old typed calls.
        let _ = self.app.emit(event, payload);
    }
}

/// OS-keychain-backed secret store (Windows Credential Manager, macOS Keychain,
/// Linux libsecret). Delegates to the existing `crate::secrets` free functions
/// so behavior is byte-for-byte what the desktop app shipped. Constructed once
/// the desktop setup is migrated onto the shared `bootstrap()`; kept here now so
/// the `SecretStore` seam is symmetric across both transports.
#[allow(dead_code)]
pub struct KeychainSecretStore;

impl SecretStore for KeychainSecretStore {
    fn set(&self, account: &str, secret: &str) -> Result<(), String> {
        crate::secrets::set(account, secret)
    }
    fn get(&self, account: &str) -> Result<Option<String>, String> {
        crate::secrets::get(account)
    }
    fn delete(&self, account: &str) -> Result<(), String> {
        crate::secrets::delete(account)
    }
}

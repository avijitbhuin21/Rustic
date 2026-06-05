//! The `AppContext` abstraction — the seam between transport-agnostic command
//! bodies and the two transports (Tauri desktop, headless server).
//!
//! Historically every command took a `tauri::AppHandle` and used it for four
//! things: emitting events, resolving the app-data dir, resolving the home dir,
//! and reaching the global state. Those four capabilities are exactly what this
//! trait exposes — so a command written against `&dyn AppContext` runs
//! unchanged on the desktop (where the impl wraps an `AppHandle`) and on the
//! server (where the impl wraps a WebSocket broadcast hub + env-derived paths).

use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;

use crate::state::AppState;

/// The narrow "push an event to the client" capability. Kept separate from
/// [`AppContext`] and object-safe (no generics) so the filesystem watcher —
/// which only needs to emit — can hold an `Arc<dyn EventEmitter>` without
/// dragging in path/state access.
pub trait EventEmitter: Send + Sync + 'static {
    /// Emit a named event carrying an already-serialized JSON payload. On the
    /// desktop this forwards to `AppHandle::emit`; on the server it publishes
    /// onto the broadcast hub that every `/ws` connection is subscribed to.
    fn emit_json(&self, event: &str, payload: serde_json::Value);
}

/// Ergonomic, generic `emit` layered over the object-safe [`EventEmitter`].
/// Implemented for every `EventEmitter` via a blanket impl so call sites can
/// write `ctx.emit("event", payload)` with any `Serialize` payload, exactly as
/// `AppHandle::emit` allowed.
pub trait EventEmitterExt {
    fn emit<T: Serialize>(&self, event: &str, payload: T);
}

impl<E: EventEmitter + ?Sized> EventEmitterExt for E {
    fn emit<T: Serialize>(&self, event: &str, payload: T) {
        match serde_json::to_value(payload) {
            Ok(value) => self.emit_json(event, value),
            Err(e) => tracing::error!(event, error = %e, "failed to serialize event payload"),
        }
    }
}

/// Free-function form for spots that hold a `&dyn EventEmitter` and prefer not
/// to import the extension trait.
pub fn emit_event<T: Serialize>(emitter: &dyn EventEmitter, event: &str, payload: T) {
    emitter.emit(event, payload);
}

/// Everything a command body needs from its host transport. Object-safe so it
/// can be passed as `&dyn AppContext`.
pub trait AppContext: EventEmitter {
    /// The application data directory (DB, logs, file-history, secrets file).
    /// Desktop: the Tauri app-data dir (profile-scoped in debug). Server:
    /// `RUSTIC_DATA_DIR` or the platform default.
    fn data_dir(&self) -> PathBuf;

    /// The user's home directory. Used to resolve `~/projects` and friends.
    fn home_dir(&self) -> PathBuf;

    /// Shared, thread-safe global state. Both transports hold the same
    /// `Arc<AppState>` for the life of the process.
    fn state(&self) -> &Arc<AppState>;

    /// Best-effort secret store (API keys, git token). Desktop: OS keychain.
    /// Server: encrypted file under `data_dir`.
    fn secrets(&self) -> &dyn crate::secrets::SecretStore;
}

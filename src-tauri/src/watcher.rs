//! Filesystem watcher. Moved to `rustic-app` (emit abstracted behind
//! `EventEmitter`) so the server shares it; re-exported here so
//! `crate::watcher::*` keeps resolving. Desktop call sites wrap the
//! `AppHandle` in [`crate::transport::TauriEmitter`].

#[allow(unused_imports)]
pub use rustic_app::watcher::*;

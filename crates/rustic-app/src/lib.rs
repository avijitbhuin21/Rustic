//! Transport-agnostic application layer for Rustic.
//!
//! Both the Tauri desktop shell and the headless `rustic-server` build on top
//! of this crate. It owns the global [`AppState`], the filesystem
//! [`watcher`], the [`path_scope`] guards, the poison-resilient mutex helper
//! ([`sync_ext`]), and — crucially — the [`AppContext`] / [`SecretStore`]
//! abstractions that let command bodies stay identical across both transports.
//!
//! The desktop shell injects a `TauriContext` (emits via `AppHandle`, paths via
//! the Tauri path API, secrets via the OS keychain). The server injects a
//! `ServerContext` (emits onto a WebSocket broadcast hub, paths from env,
//! secrets from an encrypted file). Neither lives here — only the traits do.

pub mod bootstrap;
pub mod cloud_sync;
pub mod config;
pub mod context;
pub mod github_download;
pub mod notebook_kernel;
pub mod path_scope;
pub mod preview_ops;
pub mod search_ops;
pub mod secrets;
pub mod state;
pub mod sync_ext;
pub mod watcher;

pub use bootstrap::{bootstrap, Bootstrapped};
pub use config::ServerConfig;
pub use context::{emit_event, AppContext, EventEmitter, EventEmitterExt};
pub use secrets::{FileSecretStore, SecretStore};
pub use state::{AgentState, AgentTask, AppState, FileHistoryHandle, TaskCostMap};
pub use sync_ext::MutexExt;
pub use watcher::{FileWatcherManager, FsChangeEvent};

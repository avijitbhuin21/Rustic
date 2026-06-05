//! Per-module server command handlers.
//!
//! Each module exposes
//! `pub async fn dispatch(ctx: &ServerContext, command: &str, args: &Value)
//!     -> Option<Result<Value, ApiError>>`
//! returning `None` for commands it doesn't own (so [`crate::api::dispatch`]
//! tries the next module) and `Some(result)` once it claims one. Handlers reuse
//! the same core-crate functions the desktop `#[tauri::command]` bodies call, so
//! behavior is identical across transports.

pub mod agent_chat;
pub mod agent_config;
pub mod browser;
pub mod editor;
pub mod file_history;
pub mod file_tree;
pub mod formatters;
pub mod git;
pub mod meta;
pub mod preview;
pub mod rules;
pub mod search;
pub mod settings;
pub mod skills;
pub mod terminal;
pub mod workflows;
pub mod workspace;

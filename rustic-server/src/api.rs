//! HTTP command dispatch: `POST /api/<command>` with a JSON body of the same
//! args the desktop passes to `invoke(command, args)`.
//!
//! Frontend args are camelCase (Tauri converts them to snake_case Rust params),
//! so every arg struct here is `#[serde(rename_all = "camelCase")]` to read the
//! wire format unchanged.
//!
//! The actual command handlers live in per-module files under
//! [`crate::commands`]. Each module exposes
//! `async fn dispatch(ctx, command, args) -> Option<Result<Value, ApiError>>`,
//! returning `None` for commands it doesn't own so the next module gets a turn.
//! Every server handler reuses the exact same core-crate function the matching
//! `#[tauri::command]` calls, so behavior is identical to desktop. Commands not
//! owned by any module return `501 Not Implemented` with the command name —
//! never a silent empty success (see the "no silent backend fallbacks" rule).

use serde::Deserialize;
use serde_json::Value;

use rustic_app::context::AppContext;
use rustic_app::sync_ext::MutexExt;

use crate::commands;
use crate::context::ServerContext;

/// A dispatch error carrying the HTTP status the route layer should return.
pub struct ApiError {
    pub status: u16,
    pub message: String,
}

impl ApiError {
    pub fn bad(msg: impl Into<String>) -> Self {
        Self {
            status: 400,
            message: msg.into(),
        }
    }
    pub fn not_impl(cmd: &str) -> Self {
        Self {
            status: 501,
            message: format!(
                "command '{cmd}' is not yet wired into the server build. \
                 It works on desktop; server support lands with the command-body migration."
            ),
        }
    }
}

/// `String` command errors (the `Result<_, String>` every command returns) map
/// to HTTP 400 with the message body — same shape the frontend already handles.
impl From<String> for ApiError {
    fn from(message: String) -> Self {
        Self {
            status: 400,
            message,
        }
    }
}

/// `&str` errors (`.ok_or("…")?` in ported command bodies) map the same way.
impl From<&str> for ApiError {
    fn from(message: &str) -> Self {
        Self {
            status: 400,
            message: message.to_string(),
        }
    }
}

/// Deserialize the JSON args into a command's typed arg struct.
pub fn parse<T: for<'de> Deserialize<'de>>(args: &Value) -> Result<T, ApiError> {
    serde_json::from_value(args.clone()).map_err(|e| ApiError::bad(format!("invalid args: {e}")))
}

/// Serialize a command's return value into the JSON the frontend resolves to.
pub fn ok<T: serde::Serialize>(v: T) -> Result<Value, ApiError> {
    serde_json::to_value(v).map_err(|e| ApiError::bad(format!("serialize failed: {e}")))
}

/// Resolve a project_id to its on-disk root via the in-memory workspace.
/// Mirrors `commands::git::get_project_path`.
pub fn project_root(ctx: &ServerContext, project_id: &str) -> Result<String, ApiError> {
    let ws = ctx.state().workspace.lock_safe();
    ws.list_projects()
        .into_iter()
        .find(|p| p.id.to_string() == project_id)
        .map(|p| p.root_path.to_string_lossy().to_string())
        .ok_or_else(|| ApiError::bad(format!("Project not found: {project_id}")))
}

// ---- shared arg structs used by more than one module ----

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PathArg {
    pub path: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectArg {
    pub project_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectPathArg {
    pub project_id: String,
    pub path: String,
}

/// The top-level dispatch table. Walks each command module in dependency order;
/// the first one that owns `command` handles it. Anything unclaimed is 501.
pub async fn dispatch(ctx: &ServerContext, command: &str, args: Value) -> Result<Value, ApiError> {
    macro_rules! try_modules {
        ($($module:path),* $(,)?) => {
            $(
                if let Some(result) = $module(ctx, command, &args).await {
                    return result;
                }
            )*
        };
    }

    try_modules!(
        commands::meta::dispatch,
        commands::workspace::dispatch,
        commands::file_tree::dispatch,
        commands::editor::dispatch,
        commands::search::dispatch,
        commands::git::dispatch,
        commands::terminal::dispatch,
        commands::browser::dispatch,
        commands::file_history::dispatch,
        commands::formatters::dispatch,
        commands::preview::dispatch,
        commands::settings::dispatch,
        commands::power::dispatch,
        commands::process::dispatch,
        commands::tunnel::dispatch,
        commands::skills::dispatch,
        commands::workflows::dispatch,
        commands::rules::dispatch,
        commands::agent_config::dispatch,
        commands::agent_chat::dispatch,
        commands::github_auto::dispatch,
    );

    Err(ApiError::not_impl(command))
}

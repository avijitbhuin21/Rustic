//! MCP server config + connection commands.
//!
//! Servers live in JSON files (not SQLite) so they're easy to edit by
//! hand and easy to commit to source for project scope.
//!   - User scope:    <app_data_dir>/mcp.json
//!   - Project scope: <project_root>/.mcp.json

use crate::state::AppState;
use rustic_agent::{
    LoadProjectScopeResult, McpConnectResult, McpScope, McpServerWithStatus, ToolDef,
};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{AppHandle, State};

// === MCP commands ===
//
// Config model (matches Claude Code): servers live in JSON files, not SQLite.
//   - User scope:    <app_data_dir>/mcp.json       — shared across projects
//   - Project scope: <project_root>/.mcp.json      — committed to source
//
// The frontend edits the raw JSON via a modal. On save, the backend validates,
// writes atomically, reloads the scope into McpManager, and tries to connect
// each server so the UI can show per-server success/failure inline.

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpSaveResult {
    pub name: String,
    pub connected: bool,
    pub tool_count: usize,
    pub error: Option<String>,
}

impl From<McpConnectResult> for McpSaveResult {
    fn from(r: McpConnectResult) -> Self {
        Self {
            name: r.name,
            connected: r.connected,
            tool_count: r.tool_count,
            error: r.error,
        }
    }
}

fn parse_scope(s: &str) -> Result<McpScope, String> {
    match s {
        "user" => Ok(McpScope::User),
        "project" => Ok(McpScope::Project),
        other => Err(format!("Unknown MCP scope: {}. Valid values: user, project", other)),
    }
}

/// Resolve the on-disk path for a given scope.
/// User scope uses the Tauri app data dir; project scope needs `project_id`.
fn resolve_scope_path(
    app: &AppHandle,
    state: &State<'_, AppState>,
    scope: McpScope,
    project_id: Option<&str>,
) -> Result<PathBuf, String> {
    match scope {
        McpScope::User => {
            let dir = tauri::Manager::path(app)
                .app_data_dir()
                .map_err(|e| format!("Failed to resolve app data dir: {}", e))?;
            Ok(dir.join("mcp.json"))
        }
        McpScope::Project => {
            let pid = project_id
                .ok_or_else(|| "project_id is required for project scope".to_string())?;
            let workspace = state.workspace.lock().unwrap();
            let project = workspace
                .list_projects()
                .into_iter()
                .find(|p| p.id.to_string() == pid)
                .ok_or_else(|| format!("Project not found: {}", pid))?;
            Ok(project.root_path.join(".mcp.json"))
        }
    }
}

/// Return the current JSON text for a scope. If the file does not exist yet,
/// returns a blank template so the user has something to edit.
#[tauri::command]
pub fn read_mcp_json(
    app: AppHandle,
    state: State<'_, AppState>,
    scope: String,
    project_id: Option<String>,
) -> Result<String, String> {
    let scope = parse_scope(&scope)?;
    let path = resolve_scope_path(&app, &state, scope, project_id.as_deref())?;
    if path.exists() {
        std::fs::read_to_string(&path).map_err(|e| e.to_string())
    } else {
        Ok("{\n  \"mcpServers\": {}\n}\n".to_string())
    }
}

/// Validate + write raw JSON content for a scope, reload it into the manager,
/// and try to connect each server. Returns per-server `{name, connected, error}`.
///
/// `async` + `spawn_blocking` so the slow path (spawning MCP child processes,
/// performing the `initialize`/`tools/list` round-trip) doesn't block the Tauri
/// main-thread command dispatcher — other UI commands (file tree, chat, etc.)
/// stay responsive while servers are being tested.
#[tauri::command]
pub async fn save_mcp_json(
    app: AppHandle,
    state: State<'_, AppState>,
    scope: String,
    project_id: Option<String>,
    content: String,
) -> Result<Vec<McpSaveResult>, String> {
    let scope = parse_scope(&scope)?;
    let path = resolve_scope_path(&app, &state, scope, project_id.as_deref())?;
    let mcp_arc = Arc::clone(&state.agent.lock().unwrap().mcp_manager);

    tokio::task::spawn_blocking(move || {
        let mut mcp = mcp_arc.lock().unwrap();
        match scope {
            McpScope::User => mcp.set_user_path(path.clone()),
            McpScope::Project => mcp.set_project_path(path.clone()),
        }
        mcp.save_scope_raw(scope, &content)
            .map_err(|e| e.to_string())?;
        Ok(mcp
            .test_scope(scope)
            .into_iter()
            .map(McpSaveResult::from)
            .collect())
    })
    .await
    .map_err(|e| format!("save_mcp_json task panicked: {}", e))?
}

#[tauri::command]
pub async fn list_mcp_servers(
    app: AppHandle,
    state: State<'_, AppState>,
    project_id: Option<String>,
) -> Result<Vec<McpServerWithStatus>, String> {
    let user_path = resolve_scope_path(&app, &state, McpScope::User, None).ok();
    let project_path = project_id
        .as_deref()
        .and_then(|pid| resolve_scope_path(&app, &state, McpScope::Project, Some(pid)).ok());
    let mcp_arc = Arc::clone(&state.agent.lock().unwrap().mcp_manager);

    tokio::task::spawn_blocking(move || {
        let mut mcp = mcp_arc.lock().unwrap();
        if let Some(p) = user_path {
            let _ = mcp.load_scope(McpScope::User, &p);
        }
        if let Some(p) = project_path {
            // F-10 gate: don't auto-spawn project-scope MCP servers without
            // prior user consent on the current content hash.
            let _ = mcp.load_project_scope_gated(&p);
        }
        let _ = mcp.connect_all();
        Ok(mcp.list_servers_with_status())
    })
    .await
    .map_err(|e| format!("list_mcp_servers task panicked: {}", e))?
}

/// Inspect a project's `.mcp.json` and report whether it's pending consent.
/// Returns the raw content + sha256 so the UI modal can render it. If the
/// file is absent OR already approved, returns null.
#[tauri::command]
pub async fn get_pending_mcp_consent(
    app: AppHandle,
    state: State<'_, AppState>,
    project_id: String,
) -> Result<Option<serde_json::Value>, String> {
    let path = resolve_scope_path(&app, &state, McpScope::Project, Some(&project_id))?;
    let mcp_arc = Arc::clone(&state.agent.lock().unwrap().mcp_manager);
    tokio::task::spawn_blocking(move || {
        if !path.exists() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        let hash = rustic_agent::mcp_sha256_hex(text.as_bytes());
        let mcp = mcp_arc.lock().unwrap();
        if mcp.is_project_consented(&path, &hash) {
            return Ok(None);
        }
        Ok(Some(serde_json::json!({
            "projectPath": path.display().to_string(),
            "contentHash": hash,
            "content": text,
        })))
    })
    .await
    .map_err(|e| format!("get_pending_mcp_consent task panicked: {}", e))?
}

/// User clicked "Approve" on the consent modal. Records the (project_path,
/// content_hash) pair, then re-runs the gated load and the connect cycle so
/// servers come online without waiting for the next chat message.
#[tauri::command]
pub async fn approve_mcp_project_consent(
    app: AppHandle,
    state: State<'_, AppState>,
    project_id: String,
    content_hash: String,
) -> Result<Vec<McpSaveResult>, String> {
    let path = resolve_scope_path(&app, &state, McpScope::Project, Some(&project_id))?;
    let mcp_arc = Arc::clone(&state.agent.lock().unwrap().mcp_manager);
    tokio::task::spawn_blocking(move || -> Result<Vec<McpSaveResult>, String> {
        // Re-read the file and re-hash it to make sure the user is approving
        // the same bytes the modal showed them (defence against TOCTOU where
        // someone swapped the file between display and click).
        let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        let actual = rustic_agent::mcp_sha256_hex(text.as_bytes());
        if actual != content_hash {
            return Err(format!(
                ".mcp.json changed since the consent modal opened (expected {}, got {}); refusing to approve.",
                content_hash, actual
            ));
        }
        let mut mcp = mcp_arc.lock().unwrap();
        mcp.approve_project_consent(path.clone(), actual)
            .map_err(|e| e.to_string())?;
        // Now actually load + connect.
        match mcp.load_project_scope_gated(&path) {
            Ok(LoadProjectScopeResult::Loaded(_)) => {}
            Ok(other) => {
                return Err(format!("Unexpected gated load result after approval: {:?}", other));
            }
            Err(e) => return Err(e.to_string()),
        }
        Ok(mcp
            .test_scope(McpScope::Project)
            .into_iter()
            .map(McpSaveResult::from)
            .collect())
    })
    .await
    .map_err(|e| format!("approve_mcp_project_consent task panicked: {}", e))?
}

/// Forget a previously-granted approval (e.g. user wants to re-review).
/// Any currently-loaded project-scope servers stay running until the next
/// load cycle, which will then refuse to re-spawn.
#[tauri::command]
pub async fn revoke_mcp_project_consent(
    app: AppHandle,
    state: State<'_, AppState>,
    project_id: String,
) -> Result<(), String> {
    let path = resolve_scope_path(&app, &state, McpScope::Project, Some(&project_id))?;
    let mcp_arc = Arc::clone(&state.agent.lock().unwrap().mcp_manager);
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let mut mcp = mcp_arc.lock().unwrap();
        mcp.revoke_project_consent(&path).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("revoke_mcp_project_consent task panicked: {}", e))?
}

#[tauri::command]
pub async fn test_mcp_server(
    state: State<'_, AppState>,
    id: String,
) -> Result<Vec<ToolDef>, String> {
    let mcp_arc = Arc::clone(&state.agent.lock().unwrap().mcp_manager);
    tokio::task::spawn_blocking(move || {
        mcp_arc
            .lock()
            .unwrap()
            .test_server(&id)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("test_mcp_server task panicked: {}", e))?
}

/// Tools advertised by a single connected MCP server. The settings UI calls
/// this when the user expands a server row to show what the agent can call.
/// Errors if the server isn't currently connected — the frontend should
/// surface this as "Server not connected" rather than a generic failure.
#[tauri::command]
pub async fn list_mcp_server_tools(
    state: State<'_, AppState>,
    id: String,
) -> Result<Vec<ToolDef>, String> {
    let mcp_arc = Arc::clone(&state.agent.lock().unwrap().mcp_manager);
    tokio::task::spawn_blocking(move || {
        mcp_arc
            .lock()
            .unwrap()
            .server_tools(&id)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("list_mcp_server_tools task panicked: {}", e))?
}

#[tauri::command]
pub async fn remove_mcp_server(
    state: State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    let mcp_arc = Arc::clone(&state.agent.lock().unwrap().mcp_manager);
    tokio::task::spawn_blocking(move || {
        mcp_arc
            .lock()
            .unwrap()
            .remove_server(&id)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("remove_mcp_server task panicked: {}", e))?
}

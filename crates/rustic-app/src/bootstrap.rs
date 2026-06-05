//! Transport-agnostic startup sequence shared by both binaries.
//!
//! This replicates the parts of `src-tauri`'s `setup()` that do not touch the
//! webview: open the DB, hydrate AI/tool config + secrets, restore the git
//! token, seed default workflows, and restore persisted projects (loading them
//! into the workspace and starting filesystem watchers). The Tauri shell still
//! runs its own webview-specific setup; the server calls this and nothing else.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::context::EventEmitter;
use crate::secrets::{provider_account, SecretStore};
use crate::state::AppState;
use crate::sync_ext::MutexExt;

/// Account name for the stored GitHub token. Must match
/// `src-tauri/src/commands/git.rs::GIT_TOKEN_ACCOUNT`.
pub const GIT_TOKEN_ACCOUNT: &str = "github_token";

/// Result of [`bootstrap`]: the shared state plus the data dir it was opened
/// against (so the caller can point logging / static-asset routes at it).
pub struct Bootstrapped {
    pub state: Arc<AppState>,
    pub data_dir: PathBuf,
}

/// Open the database at `<data_dir>/rustic.db`, build [`AppState`], hydrate
/// config + secrets, seed workflows, and restore persisted projects.
///
/// * `emitter` — used to start filesystem watchers for restored projects, so
///   external edits stream to the client just like on the desktop.
/// * `secrets` — the secret backend (keychain on desktop, file/env on server).
pub fn bootstrap(
    data_dir: &Path,
    secrets: &dyn SecretStore,
    emitter: Arc<dyn EventEmitter>,
) -> anyhow::Result<Bootstrapped> {
    std::fs::create_dir_all(data_dir)?;

    let db_path = data_dir.join("rustic.db");
    let db = rustic_db::Database::new(&db_path)
        .map_err(|e| anyhow::anyhow!("Could not open the Rustic database at {}: {}", db_path.display(), e))?;

    let state = Arc::new(AppState::new(db));

    // Gate project-scope .mcp.json auto-load on content-hash consent.
    {
        let mcp_arc = Arc::clone(&state.agent.lock_safe().mcp_manager);
        let consent_path = data_dir.join("mcp_consent.json");
        mcp_arc.lock_safe().set_consent_path(consent_path);
    }

    hydrate_config_and_secrets(&state, secrets);

    if let Ok(Some(tok)) = secrets.get(GIT_TOKEN_ACCOUNT) {
        *state.git_token.lock_safe() = Some(tok);
    }

    rustic_agent::seed_default_workflows();

    restore_projects(&state, emitter);

    Ok(Bootstrapped {
        state,
        data_dir: data_dir.to_path_buf(),
    })
}

/// Load `ai_config` + `tool_config` from SQLite and hydrate per-provider API
/// keys from the secret store. Mirrors the desktop hydration, minus the
/// legacy-plaintext→keychain migration write-back (the server's file store has
/// no legacy plaintext to migrate from).
fn hydrate_config_and_secrets(state: &Arc<AppState>, secrets: &dyn SecretStore) {
    let db = state.db.lock_safe();

    if let Ok(Some(json)) = db.get_setting("ai_config") {
        if let Ok(mut config) = serde_json::from_str::<rustic_agent::AiConfig>(&json) {
            for entry in config.providers.iter_mut() {
                if !entry.api_key.is_empty() {
                    continue;
                }
                let acct = provider_account(entry.provider_type.as_str(), entry.name.as_deref());
                match secrets.get(&acct) {
                    Ok(Some(secret)) => {
                        entry.api_key = secret;
                        tracing::info!(account = %acct, "[secrets] hydrated key");
                    }
                    Ok(None) => {
                        tracing::info!(account = %acct, "[secrets] no entry — provider not configured");
                    }
                    Err(e) => {
                        tracing::error!(account = %acct, error = %e, "[secrets] GET failed");
                    }
                }
            }
            state.agent.lock_safe().ai_config = config;
        }
    }

    if let Ok(Some(json)) = db.get_setting("tool_config") {
        if let Ok(config) = serde_json::from_str(&json) {
            state.agent.lock_safe().tool_config = config;
        }
    }
}

/// Load persisted (non-archived) projects from the DB into the in-memory
/// workspace and start a filesystem watcher for each. Mirrors the project
/// restore in the desktop `setup()`.
fn restore_projects(state: &Arc<AppState>, emitter: Arc<dyn EventEmitter>) {
    let projects = {
        let db = state.db.lock_safe();
        db.list_projects().unwrap_or_default()
    };

    {
        let mut workspace = state.workspace.lock_safe();
        for row in &projects {
            let path = PathBuf::from(&row.root_path);
            if !workspace.projects.iter().any(|p| p.id == row.id) {
                let mut project = rustic_core::workspace::project::Project::new(path);
                project.id = row.id.clone();
                project.name = row.name.clone();
                workspace.projects.push(project);
            }
        }
    }

    let mut watcher = state.file_watcher.lock_safe();
    for project in &projects {
        watcher.watch_project(
            &project.root_path,
            Arc::clone(&emitter),
            Some(state.workspace_services.clone()),
        );
    }
}

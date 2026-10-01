//! Cloud sync commands: push the local environment (or one project / some
//! metadata) to a deployed rustic-server, or pull from it, authenticated with
//! the server password. The client itself is shared with peer sync
//! (`rustic_app::peer::client`); this module adds password login and the
//! per-backend keychain accounts.

use crate::lan::DesktopPeerHost;
use crate::transport::KeychainSecretStore;
use rustic_app::peer::client as pc;

pub use pc::RemoteProject;

/// Normalize + validate the server base URL.
fn normalize_base(url: &str) -> Result<String, String> {
    pc::normalize_base(url)
}

/// Client with a connect timeout only — archives can be large and links slow.
fn http() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())
}

/// Log in to `url` with the saved password (or `password` when non-empty).
async fn session(url: &str, password: Option<String>) -> Result<(reqwest::Client, String, String), String> {
    let base = normalize_base(url)?;
    let password = match password.filter(|p| !p.is_empty()) {
        Some(p) => p,
        None => remembered_password(url)?,
    };
    let client = http()?;
    let token = pc::login(&client, &base, &password).await?;
    Ok((client, base, token))
}

/// Push the entire local environment to the server. Everything on the server
/// is replaced by the local copy.
#[tauri::command]
pub async fn cloud_sync_push(
    app: tauri::AppHandle,
    url: String,
    password: String,
) -> Result<String, String> {
    let host = DesktopPeerHost::arc(&app);
    let reporter = pc::reporter(&host, "push");
    reporter.stage("connecting", &normalize_base(&url)?, 0, 0);
    let (client, base, token) = session(&url, Some(password)).await?;
    pc::push_env(&host, &client, &base, &token, &reporter, pc::REMOTE_STREAMS).await
}

/// Pull the server's entire environment down. Everything local is replaced by
/// the cloud copy — the app state reloads in place.
#[tauri::command]
pub async fn cloud_sync_pull(
    app: tauri::AppHandle,
    url: String,
    password: String,
) -> Result<String, String> {
    let host = DesktopPeerHost::arc(&app);
    let reporter = pc::reporter(&host, "pull");
    reporter.stage("connecting", &normalize_base(&url)?, 0, 0);
    let (client, base, token) = session(&url, Some(password)).await?;
    pc::pull_env(&host, &client, &base, &token, &reporter, pc::REMOTE_STREAMS).await
}

/// Keychain account holding the cloud-sync password, so the explorer's
/// per-project push/pull doesn't have to ask for it on every use. Written by
/// [`cloud_sync_remember`] after the Remote Backend settings verify a
/// connection.
const CLOUD_PASSWORD_ACCOUNT: &str = "cloud-sync-password";

/// Keychain account for one saved backend, so several can stay connected.
fn password_account(url: &str) -> Result<String, String> {
    Ok(format!("{CLOUD_PASSWORD_ACCOUNT}:{}", normalize_base(url)?))
}

/// Remember the verified cloud-sync password in the OS keychain — per backend
/// when `url` is given (empty password forgets it).
#[tauri::command]
pub async fn cloud_sync_remember(password: String, url: Option<String>) -> Result<(), String> {
    use rustic_app::secrets::SecretStore;
    let account = match url.as_deref().filter(|u| !u.trim().is_empty()) {
        Some(u) => password_account(u)?,
        None => CLOUD_PASSWORD_ACCOUNT.to_string(),
    };
    if password.is_empty() {
        return KeychainSecretStore.delete(&account);
    }
    KeychainSecretStore.set(&account, &password)
}

/// Whether a remembered cloud-sync password exists (for `url`, or any legacy one).
#[tauri::command]
pub async fn cloud_sync_has_credentials(url: Option<String>) -> Result<bool, String> {
    Ok(match url.as_deref().filter(|u| !u.trim().is_empty()) {
        Some(u) => remembered_password(u).is_ok(),
        None => legacy_password().is_some(),
    })
}

fn legacy_password() -> Option<String> {
    use rustic_app::secrets::SecretStore;
    KeychainSecretStore
        .get(CLOUD_PASSWORD_ACCOUNT)
        .ok()
        .flatten()
        .filter(|p| !p.is_empty())
}

/// The saved password for `url`, falling back to the pre-multi-backend
/// single account so existing setups keep working.
pub(crate) fn remembered_password(url: &str) -> Result<String, String> {
    use rustic_app::secrets::SecretStore;
    let per_backend = password_account(url)
        .ok()
        .and_then(|a| KeychainSecretStore.get(&a).ok().flatten())
        .filter(|p| !p.is_empty());
    per_backend.or_else(legacy_password).ok_or_else(|| {
        "No password saved for this backend — connect to it in Settings › Cloud first"
            .to_string()
    })
}

/// Is a saved backend reachable and does its saved password still work?
/// Drives the online/offline badge in Cloud → Machines.
#[tauri::command]
pub async fn cloud_backend_ping(url: String) -> Result<bool, String> {
    let base = normalize_base(&url)?;
    let password = remembered_password(&url)?;
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_secs(8))
        .build()
        .map_err(|e| e.to_string())?;
    pc::login(&client, &base, &password).await.map(|_| true)
}

/// Push ONE project's files to the server. Nothing else on the server changes:
/// its database, keys, tasks and other projects are untouched.
#[tauri::command]
pub async fn cloud_sync_push_project(
    app: tauri::AppHandle,
    url: String,
    project_id: String,
) -> Result<String, String> {
    let host = DesktopPeerHost::arc(&app);
    let reporter = pc::reporter(&host, "push");
    reporter.stage("connecting", &normalize_base(&url)?, 0, 1);
    let (client, base, token) = session(&url, None).await?;
    pc::push_project_env(&host, &client, &base, &token, project_id, &reporter, pc::REMOTE_STREAMS).await
}

/// Pull ONE project's files from the server, replacing that project's local
/// tree. Everything else on this machine is left alone.
#[tauri::command]
pub async fn cloud_sync_pull_project(
    app: tauri::AppHandle,
    url: String,
    project_id: String,
    target_parent: Option<String>,
) -> Result<String, String> {
    let host = DesktopPeerHost::arc(&app);
    let reporter = pc::reporter(&host, "pull");
    reporter.stage("connecting", &normalize_base(&url)?, 0, 1);
    let (client, base, token) = session(&url, None).await?;
    pc::pull_project_env(
        &host,
        &client,
        &base,
        &token,
        project_id,
        target_parent,
        &reporter,
        pc::REMOTE_STREAMS,
    )
    .await
}

/// List the projects on the remote backend (uses the remembered password), so
/// the Pull picker can offer projects that don't exist on this machine yet.
#[tauri::command]
pub async fn cloud_list_remote_projects(url: String) -> Result<Vec<RemoteProject>, String> {
    let (client, base, token) = session(&url, None).await?;
    pc::list_projects_env(&client, &base, &token).await
}

/// Preview a metadata-only sync: per-item status (new / conflict / same /
/// local_only) from the perspective of the side that RECEIVES the items.
#[tauri::command]
pub async fn cloud_meta_preview(
    app: tauri::AppHandle,
    url: String,
    direction: String,
) -> Result<Vec<rustic_app::meta_sync::MetaDiffEntry>, String> {
    let (client, base, token) = session(&url, None).await?;
    pc::meta_preview_env(&DesktopPeerHost::arc(&app), &client, &base, &token, &direction).await
}

/// Apply a metadata-only sync. `overwrite` lists the conflict keys the user
/// chose to replace; `only` limits it to those item keys (which overwrite).
#[tauri::command]
pub async fn cloud_meta_apply(
    app: tauri::AppHandle,
    url: String,
    direction: String,
    overwrite: Vec<String>,
    only: Option<Vec<String>>,
) -> Result<rustic_app::meta_sync::MetaApplySummary, String> {
    let (client, base, token) = session(&url, None).await?;
    pc::meta_apply_env(&DesktopPeerHost::arc(&app), &client, &base, &token, &direction, overwrite, only)
        .await
}

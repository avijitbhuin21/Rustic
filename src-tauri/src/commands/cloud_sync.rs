//! Cloud sync commands: push the local environment to a deployed
//! rustic-server, or pull the server's environment down — full replace in
//! both directions, applied in-process (see `rustic_app::cloud_sync`).

use std::path::PathBuf;
use std::sync::Arc;

use tauri::{AppHandle, Manager};

use crate::state::AppState;
use crate::transport::{KeychainSecretStore, TauriEmitter};

/// Normalize + validate the server base URL.
fn normalize_base(url: &str) -> Result<String, String> {
    let base = url.trim().trim_end_matches('/').to_string();
    if !base.starts_with("http://") && !base.starts_with("https://") {
        return Err("URL must start with http:// or https://".into());
    }
    Ok(base)
}

/// Log in to the remote server and return a bearer token.
async fn login(client: &reqwest::Client, base: &str, password: &str) -> Result<String, String> {
    let resp = client
        .post(format!("{base}/login"))
        .json(&serde_json::json!({ "password": password }))
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| format!("Could not reach {base}: {e}"))?;
    let status = resp.status();
    if status.as_u16() == 401 || status.as_u16() == 403 {
        return Err("Server reachable, but the password was rejected".into());
    }
    if !status.is_success() {
        return Err(format!(
            "Server responded with HTTP {status} — is this a rustic-server deployment?"
        ));
    }
    let body: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    body.get("token")
        .and_then(|t| t.as_str())
        .map(|t| t.to_string())
        .ok_or_else(|| "Login response carried no token".into())
}

/// Push the entire local environment to the server. Everything on the server
/// is replaced by the local copy.
#[tauri::command]
pub async fn cloud_sync_push(
    app: AppHandle,
    url: String,
    password: String,
) -> Result<String, String> {
    let base = normalize_base(&url)?;
    // No overall timeout: archives can be large and links slow. Connect
    // failures still surface quickly via the connect timeout.
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;
    let token = login(&client, &base, &password).await?;

    // Ask the server what it already holds so unchanged project trees can be
    // skipped (incremental sync). Any failure just means a full upload.
    let peer_state: Vec<rustic_app::cloud_sync::PeerProjectState> = match client
        .get(format!("{base}/api/sync/state"))
        .bearer_auth(&token)
        .timeout(std::time::Duration::from_secs(120))
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => resp
            .json::<serde_json::Value>()
            .await
            .ok()
            .and_then(|v| serde_json::from_value(v.get("projects")?.clone()).ok())
            .unwrap_or_default(),
        _ => Vec::new(),
    };

    let data_dir = crate::app_paths::app_data_dir(&app).map_err(|e| e.to_string())?;
    let archive = data_dir.join("sync-push.tar.zst");
    let archive_build = archive.clone();
    let app_build = app.clone();
    let manifest = tauri::async_runtime::spawn_blocking(move || {
        let state = app_build.state::<AppState>();
        let data_dir = crate::app_paths::app_data_dir(&app_build).map_err(|e| e.to_string())?;
        let skips = rustic_app::cloud_sync::decide_skips(state.inner(), &data_dir, &peer_state);
        rustic_app::cloud_sync::build_sync_archive(
            state.inner(),
            &data_dir,
            &KeychainSecretStore,
            &archive_build,
            &skips,
        )
    })
    .await
    .map_err(|e| e.to_string())??;

    let size = std::fs::metadata(&archive).map(|m| m.len()).unwrap_or(0);
    let file = tokio::fs::File::open(&archive)
        .await
        .map_err(|e| e.to_string())?;
    let resp = client
        .post(format!("{base}/api/sync/push"))
        .bearer_auth(&token)
        .header(reqwest::header::CONTENT_TYPE, "application/zstd")
        .header(reqwest::header::CONTENT_LENGTH, size)
        .body(reqwest::Body::from(file))
        .send()
        .await
        .map_err(|e| format!("Upload failed: {e}"));
    let _ = tokio::fs::remove_file(&archive).await;
    let resp = resp?;

    let status = resp.status();
    let body: serde_json::Value = resp.json().await.unwrap_or_default();
    if !status.is_success() {
        let msg = body
            .get("error")
            .and_then(|e| e.as_str())
            .unwrap_or("unknown error");
        return Err(format!("Server rejected the sync (HTTP {status}): {msg}"));
    }
    let skipped = manifest.projects.iter().filter(|p| p.files_skipped).count();
    Ok(format!(
        "Pushed {} project(s) ({:.1} MB, {} unchanged & skipped) to the cloud",
        manifest.projects.len(),
        size as f64 / (1024.0 * 1024.0),
        skipped
    ))
}

/// Pull the server's entire environment down. Everything local is replaced by
/// the cloud copy — the app state reloads in place.
#[tauri::command]
pub async fn cloud_sync_pull(
    app: AppHandle,
    url: String,
    password: String,
) -> Result<String, String> {
    let base = normalize_base(&url)?;
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;
    let token = login(&client, &base, &password).await?;

    let data_dir = crate::app_paths::app_data_dir(&app).map_err(|e| e.to_string())?;
    let archive = data_dir.join("sync-pull.tar.zst");

    // Tell the server what this machine already holds so unchanged project
    // trees are skipped in the archive it builds.
    let app_state = app.clone();
    let local_state = tauri::async_runtime::spawn_blocking(move || {
        let state = app_state.state::<AppState>();
        let data_dir = crate::app_paths::app_data_dir(&app_state).map_err(|e| e.to_string())?;
        Ok::<_, String>(rustic_app::cloud_sync::compute_peer_state(
            state.inner(),
            &data_dir,
        ))
    })
    .await
    .map_err(|e| e.to_string())??;

    // Stream the archive to disk.
    {
        let mut resp = client
            .post(format!("{base}/api/sync/pull"))
            .bearer_auth(&token)
            .json(&serde_json::json!({ "projects": local_state }))
            .send()
            .await
            .map_err(|e| format!("Download failed: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            let body: serde_json::Value = resp.json().await.unwrap_or_default();
            let msg = body
                .get("error")
                .and_then(|e| e.as_str())
                .unwrap_or("unknown error");
            return Err(format!("Server refused the sync (HTTP {status}): {msg}"));
        }
        use tokio::io::AsyncWriteExt;
        let mut file = tokio::fs::File::create(&archive)
            .await
            .map_err(|e| e.to_string())?;
        while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
            file.write_all(&chunk).await.map_err(|e| e.to_string())?;
        }
        file.flush().await.map_err(|e| e.to_string())?;
    }

    let app_apply = app.clone();
    let archive_apply = archive.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        use rustic_app::cloud_sync::{apply_sync_archive, safe_dir_name, SyncProjectEntry};

        let state = app_apply.state::<AppState>();
        let data_dir = crate::app_paths::app_data_dir(&app_apply).map_err(|e| e.to_string())?;
        let emitter: Arc<dyn rustic_app::EventEmitter> =
            Arc::new(TauriEmitter::new(app_apply.clone()));
        let home = app_apply
            .path()
            .home_dir()
            .unwrap_or_else(|_| PathBuf::from("."));
        let default_root = home.join("projects");

        // Where do imported projects land locally? 1) wherever this machine
        // already kept the same project (by id), 2) the origin path when it
        // came from a machine with the same path flavor (a desktop
        // round-trip), 3) ~/projects/<name>.
        let used: std::sync::Mutex<std::collections::HashSet<String>> = Default::default();
        let resolve = |entry: &SyncProjectEntry, old: Option<&str>| -> PathBuf {
            if let Some(old) = old {
                return PathBuf::from(old);
            }
            if path_is_native(&entry.origin_root_path) {
                return PathBuf::from(&entry.origin_root_path);
            }
            let base = safe_dir_name(&entry.name);
            let mut used = used.lock().unwrap_or_else(|p| p.into_inner());
            let mut candidate = base.clone();
            let mut n = 1;
            while !used.insert(candidate.clone()) {
                n += 1;
                candidate = format!("{base}-{n}");
            }
            default_root.join(candidate)
        };
        apply_sync_archive(
            state.inner(),
            &data_dir,
            &KeychainSecretStore,
            &archive_apply,
            emitter,
            &resolve,
        )
    })
    .await
    .map_err(|e| e.to_string());
    let _ = tokio::fs::remove_file(&archive).await;
    let manifest = result??;

    let skipped = manifest.projects.iter().filter(|p| p.files_skipped).count();
    Ok(format!(
        "Pulled {} project(s) from the cloud ({} unchanged & skipped)",
        manifest.projects.len(),
        skipped
    ))
}

/// True when `p` looks like an absolute path of THIS machine's OS flavor.
fn path_is_native(p: &str) -> bool {
    #[cfg(windows)]
    {
        let bytes = p.as_bytes();
        bytes.len() > 2
            && bytes[1] == b':'
            && (bytes[2] == b'\\' || bytes[2] == b'/')
            && bytes[0].is_ascii_alphabetic()
    }
    #[cfg(not(windows))]
    {
        p.starts_with('/')
    }
}

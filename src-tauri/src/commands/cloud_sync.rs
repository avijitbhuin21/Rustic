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
    let emitter: Arc<dyn rustic_app::EventEmitter> = Arc::new(TauriEmitter::new(app.clone()));
    let reporter = rustic_app::cloud_sync::SyncReporter::new("push", emitter);
    reporter.stage("connecting", &base, 0, 0);
    // No overall timeout: archives can be large and links slow. Connect
    // failures still surface quickly via the connect timeout.
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;
    let token = login(&client, &base, &password).await?;
    push_env(&app, &client, &base, &token, &reporter, REMOTE_STREAMS).await
}

/// Full-environment push over an authenticated connection (remote backend or
/// a paired LAN peer — both serve the same `/api/sync/*` routes).
pub(crate) async fn push_env(
    app: &AppHandle,
    client: &reqwest::Client,
    base: &str,
    token: &str,
    reporter: &rustic_app::cloud_sync::SyncReporter,
    parallel: usize,
) -> Result<String, String> {
    let app = app.clone();
    let reporter = reporter.clone();

    // Ask the server what it already holds so unchanged project trees can be
    // skipped (incremental sync). Any failure just means a full upload.
    let peer_state: Vec<rustic_app::cloud_sync::PeerProjectState> = match client
        .get(format!("{base}/api/sync/state"))
        .bearer_auth(token)
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
    let app_build = app.clone();
    let reporter_build = reporter.clone();
    let (manifest, size) = send_archive(
        client,
        base,
        token,
        archive,
        "full",
        parallel,
        &reporter,
        move |sink| {
            let state = app_build.state::<AppState>();
            let data_dir = crate::app_paths::app_data_dir(&app_build).map_err(|e| e.to_string())?;
            let skips = rustic_app::cloud_sync::decide_skips(state.inner(), &data_dir, &peer_state);
            rustic_app::cloud_sync::build_sync_archive_into(
                state.inner(),
                &data_dir,
                &KeychainSecretStore,
                sink,
                &skips,
                &reporter_build,
            )
        },
    )
    .await?;

    let skipped = manifest.projects.iter().filter(|p| p.files_skipped).count();
    reporter.stage("done", "sync complete", size, size);
    Ok(format!(
        "Pushed {} project(s) ({:.1} MB, {} unchanged & skipped) to the cloud",
        manifest.projects.len(),
        size as f64 / (1024.0 * 1024.0),
        skipped
    ))
}

/// Human-readable "12.4 MB / 88.1 MB" label for a transfer phase.
fn format_transfer(done: u64, total: u64) -> String {
    let mb = |v: u64| v as f64 / (1024.0 * 1024.0);
    if total == 0 {
        format!("{:.1} MB", mb(done))
    } else {
        format!("{:.1} MB / {:.1} MB", mb(done), mb(total))
    }
}

/// Chunks in flight per transfer: the remote backend hides WAN latency with
/// more streams; a LAN peer saturates the link with fewer.
pub(crate) const REMOTE_STREAMS: usize = 4;
pub(crate) const LAN_STREAMS: usize = 2;

type Manifest = rustic_app::cloud_sync::SyncManifest;

/// Pack an archive with `build` while uploading it in parallel, checksummed,
/// resumable chunks (`kind` = "full" | "project"). Falls back to the legacy
/// single-stream push when the peer predates chunked sync (the finished
/// archive is reused, so nothing is packed twice). Returns manifest + size.
#[allow(clippy::too_many_arguments)]
async fn send_archive<F>(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    archive: PathBuf,
    kind: &str,
    parallel: usize,
    reporter: &rustic_app::cloud_sync::SyncReporter,
    build: F,
) -> Result<(Manifest, u64), String>
where
    F: FnOnce(Box<dyn std::io::Write + Send>) -> Result<Manifest, String> + Send + 'static,
{
    use rustic_app::transfer::{client as tc, Growing};
    let grow = Growing::new(&archive);
    let writer = grow.writer().map_err(|e| e.to_string())?;
    let g = Arc::clone(&grow);
    let packer = tauri::async_runtime::spawn_blocking(move || {
        let r = build(Box::new(writer));
        match &r {
            Ok(_) => g.finish(),
            Err(e) => g.fail(e.clone()),
        }
        r
    });
    let ep = tc::Endpoint {
        client: client.clone(),
        base: base.to_string(),
        token: token.to_string(),
        parallel,
    };
    let rep = reporter.clone();
    let progress: tc::Progress = Arc::new(move |phase: &str, done: u64, total: u64| {
        if phase == "applying" {
            rep.stage(
                "applying",
                "the other side is applying the sync",
                done,
                total,
            );
        } else {
            rep.tick("uploading", &format_transfer(done, total), done, total);
        }
    });
    let sent = tc::upload(&ep, kind, Arc::clone(&grow), progress).await;
    let packed = packer.await.map_err(|e| e.to_string()).and_then(|r| r);
    let result = match (packed, sent) {
        (Err(e), _) => Err(e),
        (Ok(m), Ok(_)) => Ok(m),
        (Ok(m), Err(tc::TransferError::Unsupported)) => {
            upload_archive(client, base, token, &archive, reporter)
                .await
                .map(|_| m)
        }
        (Ok(_), Err(tc::TransferError::Failed(e))) => Err(format!("Upload failed: {e}")),
    };
    let size = grow.snapshot().0;
    let _ = tokio::fs::remove_file(&archive).await;
    result.map(|m| (m, size))
}

/// Download an archive in parallel, checksummed, resumable chunks while
/// `apply` extracts it as the contiguous prefix arrives. Falls back to the
/// legacy single-stream pull (download, then apply) for older peers.
#[allow(clippy::too_many_arguments)]
async fn receive_archive<F>(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    archive: PathBuf,
    request: serde_json::Value,
    parallel: usize,
    reporter: &rustic_app::cloud_sync::SyncReporter,
    apply: F,
) -> Result<Manifest, String>
where
    F: Fn(Box<dyn std::io::Read + Send>) -> Result<Manifest, String> + Send + Sync + 'static,
{
    use rustic_app::transfer::{client as tc, Assembler};
    let apply = Arc::new(apply);
    let asm = Arc::new(Assembler::create(&archive).map_err(|e| e.to_string())?);
    let reader = asm.growing().reader().map_err(|e| e.to_string())?;
    let a = Arc::clone(&apply);
    let extractor = tauri::async_runtime::spawn_blocking(move || a(Box::new(reader)));
    let ep = tc::Endpoint {
        client: client.clone(),
        base: base.to_string(),
        token: token.to_string(),
        parallel,
    };
    let rep = reporter.clone();
    let progress: tc::Progress = Arc::new(move |_phase: &str, done: u64, total: u64| {
        rep.tick("downloading", &format_transfer(done, total), done, total);
    });
    reporter.stage("packing", "the other side is building the archive", 0, 0);
    let got = tc::download(&ep, request.clone(), Arc::clone(&asm), progress).await;
    let result = match got {
        Ok(()) => extractor.await.map_err(|e| e.to_string()).and_then(|r| r),
        Err(tc::TransferError::Unsupported) => {
            // The extractor is still waiting for its first byte — stop it
            // before it touches anything, then do the legacy round-trip.
            asm.fail("switching to single-stream sync");
            let _ = extractor.await;
            drop(asm);
            match download_legacy(client, base, token, &request, &archive, reporter).await {
                Ok(()) => {
                    let (a, path) = (Arc::clone(&apply), archive.clone());
                    tauri::async_runtime::spawn_blocking(move || {
                        let file = std::fs::File::open(&path).map_err(|e| e.to_string())?;
                        a(Box::new(file))
                    })
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(|r| r)
                }
                Err(e) => Err(e),
            }
        }
        Err(tc::TransferError::Failed(e)) => {
            let _ = extractor.await;
            Err(format!("Download failed: {e}"))
        }
    };
    let _ = tokio::fs::remove_file(&archive).await;
    result
}

/// Legacy single-stream `/api/sync/pull` into `archive`, reporting progress.
async fn download_legacy(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    request: &serde_json::Value,
    archive: &std::path::Path,
    reporter: &rustic_app::cloud_sync::SyncReporter,
) -> Result<(), String> {
    use tokio::io::AsyncWriteExt;
    let mut resp = client
        .post(format!("{base}/api/sync/pull"))
        .bearer_auth(token)
        .json(request)
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
    let total = resp.content_length().unwrap_or(0);
    let mut got: u64 = 0;
    reporter.stage("downloading", &format_transfer(0, total), 0, total);
    let mut file = tokio::fs::File::create(archive)
        .await
        .map_err(|e| e.to_string())?;
    while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
        got += chunk.len() as u64;
        reporter.tick("downloading", &format_transfer(got, total), got, total);
        file.write_all(&chunk).await.map_err(|e| e.to_string())?;
    }
    file.flush().await.map_err(|e| e.to_string())?;
    reporter.stage("downloading", &format_transfer(got, got), got, got);
    Ok(())
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
    let emitter: Arc<dyn rustic_app::EventEmitter> = Arc::new(TauriEmitter::new(app.clone()));
    let reporter = rustic_app::cloud_sync::SyncReporter::new("pull", emitter);
    reporter.stage("connecting", &base, 0, 0);
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;
    let token = login(&client, &base, &password).await?;
    pull_env(&app, &client, &base, &token, &reporter, REMOTE_STREAMS).await
}

/// Full-environment pull over an authenticated connection (remote backend or LAN peer).
pub(crate) async fn pull_env(
    app: &AppHandle,
    client: &reqwest::Client,
    base: &str,
    token: &str,
    reporter: &rustic_app::cloud_sync::SyncReporter,
    parallel: usize,
) -> Result<String, String> {
    let app = app.clone();
    let reporter = reporter.clone();

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

    let app_apply = app.clone();
    let reporter_apply = reporter.clone();
    let request = serde_json::json!({ "projects": local_state });
    let manifest = receive_archive(
        client,
        base,
        token,
        archive,
        request,
        parallel,
        &reporter,
        move |source| {
            use rustic_app::cloud_sync::{
                apply_sync_archive_from, safe_dir_name, SyncProjectEntry,
            };

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
            apply_sync_archive_from(
                state.inner(),
                &data_dir,
                &KeychainSecretStore,
                source,
                emitter,
                &resolve,
                &reporter_apply,
            )
        },
    )
    .await?;

    let skipped = manifest.projects.iter().filter(|p| p.files_skipped).count();
    Ok(format!(
        "Pulled {} project(s) from the cloud ({} unchanged & skipped)",
        manifest.projects.len(),
        skipped
    ))
}

/// True when `p` looks like an absolute path of THIS machine's OS flavor.
pub(crate) fn path_is_native(p: &str) -> bool {
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

/// Keychain account holding the cloud-sync password, so the explorer's
/// per-project push/pull doesn't have to ask for it on every use. Written by
/// [`cloud_sync_remember`] after the Remote Backend settings verify a
/// connection.
const CLOUD_PASSWORD_ACCOUNT: &str = "cloud-sync-password";

/// Remember the verified cloud-sync password in the OS keychain.
#[tauri::command]
pub async fn cloud_sync_remember(password: String) -> Result<(), String> {
    use rustic_app::secrets::SecretStore;
    if password.is_empty() {
        return KeychainSecretStore.delete(CLOUD_PASSWORD_ACCOUNT);
    }
    KeychainSecretStore.set(CLOUD_PASSWORD_ACCOUNT, &password)
}

/// Whether a remembered cloud-sync password exists (drives the explorer menu).
#[tauri::command]
pub async fn cloud_sync_has_credentials() -> Result<bool, String> {
    use rustic_app::secrets::SecretStore;
    Ok(KeychainSecretStore
        .get(CLOUD_PASSWORD_ACCOUNT)
        .ok()
        .flatten()
        .is_some_and(|p| !p.is_empty()))
}

fn remembered_password() -> Result<String, String> {
    use rustic_app::secrets::SecretStore;
    KeychainSecretStore
        .get(CLOUD_PASSWORD_ACCOUNT)
        .ok()
        .flatten()
        .filter(|p| !p.is_empty())
        .ok_or_else(|| {
            "No cloud password saved — verify the connection in Settings › Remote Backend first"
                .to_string()
        })
}

/// Upload an archive to `/api/sync/push`, reporting byte progress.
async fn upload_archive(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    archive: &std::path::Path,
    reporter: &rustic_app::cloud_sync::SyncReporter,
) -> Result<(), String> {
    let size = std::fs::metadata(archive).map(|m| m.len()).unwrap_or(0);
    let file = tokio::fs::File::open(archive)
        .await
        .map_err(|e| e.to_string())?;
    let reporter_up = reporter.clone();
    let mut sent: u64 = 0;
    let stream = futures_util::StreamExt::map(
        tokio_util::io::ReaderStream::with_capacity(file, 256 * 1024),
        move |chunk| {
            if let Ok(c) = &chunk {
                sent += c.len() as u64;
                reporter_up.tick("uploading", &format_transfer(sent, size), sent, size);
            }
            chunk
        },
    );
    reporter.stage("uploading", &format_transfer(0, size), 0, size);
    let resp = client
        .post(format!("{base}/api/sync/push"))
        .bearer_auth(token)
        .header(reqwest::header::CONTENT_TYPE, "application/zstd")
        .header(reqwest::header::CONTENT_LENGTH, size)
        .body(reqwest::Body::wrap_stream(stream))
        .send()
        .await
        .map_err(|e| format!("Upload failed: {e}"))?;
    reporter.stage("applying", "server is applying the sync", size, size);
    let status = resp.status();
    if !status.is_success() {
        let body: serde_json::Value = resp.json().await.unwrap_or_default();
        let msg = body
            .get("error")
            .and_then(|e| e.as_str())
            .unwrap_or("unknown error")
            .to_string();
        return Err(format!("Server rejected the sync (HTTP {status}): {msg}"));
    }
    Ok(())
}

/// Push ONE project's files to the server. Nothing else on the server changes:
/// its database, keys, tasks and other projects are untouched.
#[tauri::command]
pub async fn cloud_sync_push_project(
    app: AppHandle,
    url: String,
    project_id: String,
) -> Result<String, String> {
    let base = normalize_base(&url)?;
    let password = remembered_password()?;
    let emitter: Arc<dyn rustic_app::EventEmitter> = Arc::new(TauriEmitter::new(app.clone()));
    let reporter = rustic_app::cloud_sync::SyncReporter::new("push", emitter);
    reporter.stage("connecting", &base, 0, 1);

    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;
    let token = login(&client, &base, &password).await?;
    push_project_env(
        &app,
        &client,
        &base,
        &token,
        project_id,
        &reporter,
        REMOTE_STREAMS,
    )
    .await
}

/// Single-project push over an authenticated connection (remote backend or LAN peer).
pub(crate) async fn push_project_env(
    app: &AppHandle,
    client: &reqwest::Client,
    base: &str,
    token: &str,
    project_id: String,
    reporter: &rustic_app::cloud_sync::SyncReporter,
    parallel: usize,
) -> Result<String, String> {
    let app = app.clone();
    let reporter = reporter.clone();

    let data_dir = crate::app_paths::app_data_dir(&app).map_err(|e| e.to_string())?;
    let archive = data_dir.join("sync-push-project.tar.zst");
    let app_build = app.clone();
    let reporter_build = reporter.clone();
    let project_for_build = project_id.clone();
    let (manifest, size) = send_archive(
        client,
        base,
        token,
        archive,
        "project",
        parallel,
        &reporter,
        move |sink| {
            let state = app_build.state::<AppState>();
            rustic_app::cloud_sync::build_project_archive_into(
                state.inner(),
                &project_for_build,
                sink,
                &reporter_build,
            )
        },
    )
    .await?;

    let name = manifest
        .projects
        .first()
        .map(|p| p.name.clone())
        .unwrap_or_default();
    reporter.stage("done", "sync complete", 1, 1);
    Ok(format!(
        "Pushed “{name}” to the cloud ({:.1} MB)",
        size as f64 / (1024.0 * 1024.0)
    ))
}

/// Pull ONE project's files from the server, replacing that project's local
/// tree. Everything else on this machine is left alone.
#[tauri::command]
pub async fn cloud_sync_pull_project(
    app: AppHandle,
    url: String,
    project_id: String,
    target_parent: Option<String>,
) -> Result<String, String> {
    let base = normalize_base(&url)?;
    let password = remembered_password()?;
    let emitter: Arc<dyn rustic_app::EventEmitter> = Arc::new(TauriEmitter::new(app.clone()));
    let reporter = rustic_app::cloud_sync::SyncReporter::new("pull", emitter);
    reporter.stage("connecting", &base, 0, 1);

    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;
    let token = login(&client, &base, &password).await?;
    pull_project_env(
        &app,
        &client,
        &base,
        &token,
        project_id,
        target_parent,
        &reporter,
        REMOTE_STREAMS,
    )
    .await
}

/// Single-project pull over an authenticated connection (remote backend or LAN
/// peer). `target_parent` places a project new to this machine there.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn pull_project_env(
    app: &AppHandle,
    client: &reqwest::Client,
    base: &str,
    token: &str,
    project_id: String,
    target_parent: Option<String>,
    reporter: &rustic_app::cloud_sync::SyncReporter,
    parallel: usize,
) -> Result<String, String> {
    let app = app.clone();
    let reporter = reporter.clone();

    let data_dir = crate::app_paths::app_data_dir(&app).map_err(|e| e.to_string())?;
    let archive = data_dir.join("sync-pull-project.tar.zst");
    let app_apply = app.clone();
    let reporter_apply = reporter.clone();
    let request = serde_json::json!({ "projectId": project_id });
    let manifest = receive_archive(
        client,
        base,
        token,
        archive,
        request,
        parallel,
        &reporter,
        move |source| {
            use rustic_app::cloud_sync::{
                apply_project_archive_from, safe_dir_name, SyncProjectEntry,
            };

            let state = app_apply.state::<AppState>();
            let data_dir = crate::app_paths::app_data_dir(&app_apply).map_err(|e| e.to_string())?;
            let emitter: Arc<dyn rustic_app::EventEmitter> =
                Arc::new(TauriEmitter::new(app_apply.clone()));
            let home = app_apply
                .path()
                .home_dir()
                .unwrap_or_else(|_| PathBuf::from("."));
            let default_root = home.join("projects");
            let chosen_parent = target_parent
                .clone()
                .filter(|p| !p.trim().is_empty())
                .map(PathBuf::from);
            let resolve = |entry: &SyncProjectEntry, old: Option<&str>| -> PathBuf {
                if let Some(old) = old {
                    return PathBuf::from(old);
                }
                // A project that only exists on the server goes where the user
                // picked (issue #15): <chosen folder>/<project name>.
                if let Some(parent) = chosen_parent.as_ref() {
                    return parent.join(safe_dir_name(&entry.name));
                }
                if path_is_native(&entry.origin_root_path) {
                    return PathBuf::from(&entry.origin_root_path);
                }
                default_root.join(safe_dir_name(&entry.name))
            };
            apply_project_archive_from(
                state.inner(),
                &data_dir,
                source,
                emitter,
                &resolve,
                &reporter_apply,
            )
        },
    )
    .await?;

    let name = manifest
        .projects
        .first()
        .map(|p| p.name.clone())
        .unwrap_or_default();
    Ok(format!("Pulled “{name}” from the cloud"))
}

/// One project that lives on the remote backend.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct RemoteProject {
    pub id: String,
    pub name: String,
    pub root_path: String,
}

/// List the projects on the remote backend (uses the remembered password), so
/// the Pull picker can offer projects that don't exist on this machine yet.
#[tauri::command]
pub async fn cloud_list_remote_projects(url: String) -> Result<Vec<RemoteProject>, String> {
    let base = normalize_base(&url)?;
    let password = remembered_password()?;
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;
    let token = login(&client, &base, &password).await?;
    list_projects_env(&client, &base, &token).await
}

/// List the projects of an authenticated peer (remote backend or LAN peer).
pub(crate) async fn list_projects_env(
    client: &reqwest::Client,
    base: &str,
    token: &str,
) -> Result<Vec<RemoteProject>, String> {
    let resp = client
        .post(format!("{base}/api/list_projects"))
        .bearer_auth(token)
        .json(&serde_json::json!({}))
        .send()
        .await
        .map_err(|e| format!("Listing server projects failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!(
            "Server refused the project list (HTTP {})",
            resp.status()
        ));
    }
    let rows: Vec<serde_json::Value> = resp.json().await.map_err(|e| e.to_string())?;
    Ok(rows
        .into_iter()
        .filter_map(|r| {
            Some(RemoteProject {
                id: r.get("id")?.as_str()?.to_string(),
                name: r
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                root_path: r
                    .get("root_path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            })
        })
        .collect())
}

/// Fetch the remote backend's metadata bundle.
async fn fetch_remote_meta(
    client: &reqwest::Client,
    base: &str,
    token: &str,
) -> Result<rustic_app::meta_sync::MetaBundle, String> {
    let resp = client
        .get(format!("{base}/api/sync/meta"))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| format!("Fetching server metadata failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!(
            "Server refused the metadata request (HTTP {})",
            resp.status()
        ));
    }
    resp.json().await.map_err(|e| e.to_string())
}

/// This machine's metadata bundle (blocking work off the async runtime).
async fn local_meta(app: &AppHandle) -> Result<rustic_app::meta_sync::MetaBundle, String> {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let data_dir = crate::app_paths::app_data_dir(&app).map_err(|e| e.to_string())?;
        let state = app.state::<AppState>();
        Ok(rustic_app::meta_sync::export_bundle(
            state.inner(),
            &data_dir,
            &KeychainSecretStore,
        ))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Preview a metadata-only sync: per-item status (new / conflict / same /
/// local_only) from the perspective of the side that RECEIVES the items.
#[tauri::command]
pub async fn cloud_meta_preview(
    app: AppHandle,
    url: String,
    direction: String,
) -> Result<Vec<rustic_app::meta_sync::MetaDiffEntry>, String> {
    let base = normalize_base(&url)?;
    let password = remembered_password()?;
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;
    let token = login(&client, &base, &password).await?;
    meta_preview_env(&app, &client, &base, &token, &direction).await
}

/// Metadata diff against an authenticated peer (remote backend or LAN peer).
pub(crate) async fn meta_preview_env(
    app: &AppHandle,
    client: &reqwest::Client,
    base: &str,
    token: &str,
    direction: &str,
) -> Result<Vec<rustic_app::meta_sync::MetaDiffEntry>, String> {
    let remote = fetch_remote_meta(client, base, token).await?;
    let local = local_meta(app).await?;
    Ok(if direction == "push" {
        rustic_app::meta_sync::diff_bundles(&local, &remote)
    } else {
        rustic_app::meta_sync::diff_bundles(&remote, &local)
    })
}

/// Apply a metadata-only sync. `overwrite` lists the conflict keys the user
/// chose to replace; every other conflicting item is kept as it is.
#[tauri::command]
pub async fn cloud_meta_apply(
    app: AppHandle,
    url: String,
    direction: String,
    overwrite: Vec<String>,
) -> Result<rustic_app::meta_sync::MetaApplySummary, String> {
    let base = normalize_base(&url)?;
    let password = remembered_password()?;
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;
    let token = login(&client, &base, &password).await?;
    meta_apply_env(&app, &client, &base, &token, &direction, overwrite).await
}

/// Metadata merge with an authenticated peer (remote backend or LAN peer).
pub(crate) async fn meta_apply_env(
    app: &AppHandle,
    client: &reqwest::Client,
    base: &str,
    token: &str,
    direction: &str,
    overwrite: Vec<String>,
) -> Result<rustic_app::meta_sync::MetaApplySummary, String> {
    let app = app.clone();
    if direction == "push" {
        let local = local_meta(&app).await?;
        let resp = client
            .post(format!("{base}/api/sync/meta"))
            .bearer_auth(token)
            .json(&serde_json::json!({ "bundle": local, "overwrite": overwrite }))
            .send()
            .await
            .map_err(|e| format!("Sending metadata failed: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!(
                "Server refused the metadata (HTTP {})",
                resp.status()
            ));
        }
        return resp.json().await.map_err(|e| e.to_string());
    }
    let remote = fetch_remote_meta(client, base, token).await?;
    let app2 = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let data_dir = crate::app_paths::app_data_dir(&app2).map_err(|e| e.to_string())?;
        let state = app2.state::<AppState>();
        let set: std::collections::HashSet<String> = overwrite.into_iter().collect();
        Ok(rustic_app::meta_sync::apply_bundle(
            state.inner(),
            &data_dir,
            &KeychainSecretStore,
            &remote,
            &set,
        ))
    })
    .await
    .map_err(|e| e.to_string())?
}

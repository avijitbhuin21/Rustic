//! Local-network sync commands (issue #15). Sync operations reuse the same
//! client code as the remote backend (`commands::cloud_sync::*_env`), pointed
//! at a paired device over certificate-pinned TLS.

use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::commands::cloud_sync as cs;
use crate::lan::{self, LanState, Peer};
use crate::transport::TauriEmitter;

/// Status of local-network sync on this machine.
#[derive(Serialize)]
pub struct LanStatus {
    pub enabled: bool,
    pub device_id: Option<String>,
    pub device_name: Option<String>,
    pub port: u16,
}

/// A device shown in the Cloud → Local network list.
#[derive(Serialize)]
pub struct LanDevice {
    pub device_id: String,
    pub name: String,
    pub addr: Option<String>,
    pub online: bool,
    pub paired: bool,
}

/// App data dir as a String error.
fn data_dir(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    crate::app_paths::app_data_dir(app).map_err(|e| e.to_string())
}

/// Start the listener + mDNS (idempotent).
pub async fn start_lan(app: &AppHandle, lan: &LanState) -> Result<(), String> {
    if lan.lock().identity.is_some() {
        return Ok(());
    }
    let dir = data_dir(app)?;
    let id = tauri::async_runtime::spawn_blocking(move || lan::load_or_create_identity(&dir))
        .await
        .map_err(|e| e.to_string())??;
    lan.lock().identity = Some(id.clone());
    let port = match lan::server::start(app.clone(), lan.clone(), id.clone()).await {
        Ok(p) => p,
        Err(e) => {
            lan.lock().identity = None;
            return Err(e);
        }
    };
    match lan::discovery::start(lan, &id, port) {
        Ok(daemon) => lan.lock().mdns = Some(daemon),
        Err(e) => tracing::warn!("LAN discovery unavailable: {e}"),
    }
    Ok(())
}

/// Stop the listener + mDNS.
fn stop_lan(lan: &LanState) {
    let mut inner = lan.lock();
    if let Some(tx) = inner.shutdown.take() {
        let _ = tx.send(());
    }
    if let Some(d) = inner.mdns.take() {
        let _ = d.shutdown();
    }
    inner.identity = None;
    inner.port = 0;
    inner.discovered.clear();
    inner.pending_pairs.clear();
}

#[tauri::command]
pub async fn lan_status(lan: State<'_, LanState>) -> Result<LanStatus, String> {
    let inner = lan.lock();
    Ok(LanStatus {
        enabled: inner.identity.is_some(),
        device_id: inner.identity.as_ref().map(|i| i.device_id.clone()),
        device_name: inner.identity.as_ref().map(|i| i.device_name.clone()),
        port: inner.port,
    })
}

/// Switch "Allow local-network sync" on/off (persisted across restarts).
#[tauri::command]
pub async fn lan_set_enabled(
    app: AppHandle,
    lan: State<'_, LanState>,
    enabled: bool,
) -> Result<(), String> {
    lan::set_enabled_persisted(&data_dir(&app)?, enabled)?;
    if enabled {
        start_lan(&app, lan.inner()).await
    } else {
        stop_lan(lan.inner());
        Ok(())
    }
}

/// Discovered devices merged with paired ones.
#[tauri::command]
pub async fn lan_devices(
    app: AppHandle,
    lan: State<'_, LanState>,
) -> Result<Vec<LanDevice>, String> {
    let peers = lan::load_peers(&data_dir(&app)?);
    let discovered = lan.lock().discovered.clone();
    let mut out: Vec<LanDevice> = discovered
        .values()
        .map(|d| LanDevice {
            device_id: d.device_id.clone(),
            name: d.name.clone(),
            addr: Some(d.addr.clone()),
            online: true,
            paired: peers.iter().any(|p| p.device_id == d.device_id),
        })
        .collect();
    for p in &peers {
        if !discovered.contains_key(&p.device_id) {
            out.push(LanDevice {
                device_id: p.device_id.clone(),
                name: p.name.clone(),
                addr: p.addr.clone(),
                online: false,
                paired: true,
            });
        }
    }
    out.sort_by(|a, b| (!a.online, a.name.to_lowercase()).cmp(&(!b.online, b.name.to_lowercase())));
    Ok(out)
}

/// The code this machine shows while pairing with `device_id`.
#[tauri::command]
pub async fn lan_pair_code(lan: State<'_, LanState>, device_id: String) -> Result<String, String> {
    let inner = lan.lock();
    let me = inner
        .identity
        .as_ref()
        .ok_or("Turn on local-network sync first")?;
    let d = inner
        .discovered
        .get(&device_id)
        .ok_or("That device is not on the network right now")?;
    Ok(lan::pairing_code(&me.fingerprint, &d.fingerprint))
}

/// Ask `device_id` to pair. Waits for the other machine's Accept/Decline.
#[tauri::command]
pub async fn lan_pair(
    app: AppHandle,
    lan: State<'_, LanState>,
    device_id: String,
) -> Result<String, String> {
    let (me, target) = {
        let inner = lan.lock();
        let me = inner
            .identity
            .clone()
            .ok_or("Turn on local-network sync first")?;
        let d = inner
            .discovered
            .get(&device_id)
            .cloned()
            .ok_or("That device is not on the network right now")?;
        (me, d)
    };
    let client = lan::pinned_client(&target.fingerprint)?;
    let token_in = lan::random_token()?;
    let resp = client
        .post(format!("https://{}/lan/pair", target.addr))
        .timeout(std::time::Duration::from_secs(100))
        .json(&serde_json::json!({
            "device_id": me.device_id,
            "name": me.device_name,
            "fingerprint": me.fingerprint,
            "token": token_in,
        }))
        .send()
        .await
        .map_err(|e| format!("Could not reach {}: {e}", target.name))?;
    let body: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    if !body
        .get("accepted")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return Err(format!(
            "{} declined the pairing request (or it timed out)",
            target.name
        ));
    }
    let token_out = body
        .get("token")
        .and_then(|v| v.as_str())
        .ok_or("pairing response carried no token")?;
    lan::upsert_peer(
        &data_dir(&app)?,
        Peer {
            device_id: target.device_id.clone(),
            name: target.name.clone(),
            fingerprint: target.fingerprint.clone(),
            token_out: token_out.to_string(),
            token_in,
            addr: Some(target.addr.clone()),
        },
    )?;
    Ok(target.name)
}

/// Answer an incoming pairing request shown by the global prompt.
#[tauri::command]
pub async fn lan_respond_pair(
    lan: State<'_, LanState>,
    request_id: String,
    accept: bool,
) -> Result<(), String> {
    let tx = lan
        .lock()
        .pending_pairs
        .remove(&request_id)
        .ok_or("That pairing request has expired")?;
    let _ = tx.send(accept);
    Ok(())
}

/// Forget a paired device (it must pair again to sync).
#[tauri::command]
pub async fn lan_forget(app: AppHandle, device_id: String) -> Result<(), String> {
    let dir = data_dir(&app)?;
    let mut peers = lan::load_peers(&dir);
    peers.retain(|p| p.device_id != device_id);
    lan::save_peers(&dir, &peers)
}

/// Pinned client + base URL + token for a paired device.
fn connect(
    app: &AppHandle,
    lan: &LanState,
    device_id: &str,
) -> Result<(reqwest::Client, String, String), String> {
    let peer = lan::load_peers(&data_dir(app)?)
        .into_iter()
        .find(|p| p.device_id == device_id)
        .ok_or("That device is not paired — pair it first")?;
    let addr = lan
        .lock()
        .discovered
        .get(device_id)
        .map(|d| d.addr.clone())
        .or(peer.addr.clone())
        .ok_or("That device is not on the network right now")?;
    let client = lan::pinned_client(&peer.fingerprint)?;
    Ok((client, format!("https://{addr}"), peer.token_out))
}

/// Reporter emitting `rustic:sync-progress` for a LAN transfer.
fn reporter(app: &AppHandle, direction: &str) -> rustic_app::cloud_sync::SyncReporter {
    let emitter: Arc<dyn rustic_app::EventEmitter> = Arc::new(TauriEmitter::new(app.clone()));
    rustic_app::cloud_sync::SyncReporter::new(direction, emitter)
}

/// Push to a paired device: everything, or one project.
#[tauri::command]
pub async fn lan_push(
    app: AppHandle,
    lan: State<'_, LanState>,
    device_id: String,
    project_id: Option<String>,
) -> Result<String, String> {
    let (client, base, token) = connect(&app, lan.inner(), &device_id)?;
    let rep = reporter(&app, "push");
    match project_id {
        Some(pid) => {
            cs::push_project_env(&app, &client, &base, &token, pid, &rep, cs::LAN_STREAMS).await
        }
        None => cs::push_env(&app, &client, &base, &token, &rep, cs::LAN_STREAMS).await,
    }
}

/// Pull from a paired device: everything, or one project (optionally into a chosen folder).
#[tauri::command]
pub async fn lan_pull(
    app: AppHandle,
    lan: State<'_, LanState>,
    device_id: String,
    project_id: Option<String>,
    target_parent: Option<String>,
) -> Result<String, String> {
    let (client, base, token) = connect(&app, lan.inner(), &device_id)?;
    let rep = reporter(&app, "pull");
    match project_id {
        Some(pid) => {
            cs::pull_project_env(
                &app,
                &client,
                &base,
                &token,
                pid,
                target_parent,
                &rep,
                cs::LAN_STREAMS,
            )
            .await
        }
        None => cs::pull_env(&app, &client, &base, &token, &rep, cs::LAN_STREAMS).await,
    }
}

/// Projects on a paired device (for the Pull picker).
#[tauri::command]
pub async fn lan_list_projects(
    app: AppHandle,
    lan: State<'_, LanState>,
    device_id: String,
) -> Result<Vec<cs::RemoteProject>, String> {
    let (client, base, token) = connect(&app, lan.inner(), &device_id)?;
    cs::list_projects_env(&client, &base, &token).await
}

/// Metadata diff against a paired device.
#[tauri::command]
pub async fn lan_meta_preview(
    app: AppHandle,
    lan: State<'_, LanState>,
    device_id: String,
    direction: String,
) -> Result<Vec<rustic_app::meta_sync::MetaDiffEntry>, String> {
    let (client, base, token) = connect(&app, lan.inner(), &device_id)?;
    cs::meta_preview_env(&app, &client, &base, &token, &direction).await
}

/// Metadata merge with a paired device.
#[tauri::command]
pub async fn lan_meta_apply(
    app: AppHandle,
    lan: State<'_, LanState>,
    device_id: String,
    direction: String,
    overwrite: Vec<String>,
) -> Result<rustic_app::meta_sync::MetaApplySummary, String> {
    let (client, base, token) = connect(&app, lan.inner(), &device_id)?;
    cs::meta_apply_env(&app, &client, &base, &token, &direction, overwrite).await
}

/// Called from app setup: restore "Allow local-network sync" if it was on.
pub fn restore_on_startup(app: &AppHandle) {
    let Ok(dir) = data_dir(app) else { return };
    if !lan::is_enabled_persisted(&dir) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let lan = app.state::<LanState>().inner().clone();
        if let Err(e) = start_lan(&app, &lan).await {
            tracing::warn!("LAN sync did not start: {e}");
        }
    });
}

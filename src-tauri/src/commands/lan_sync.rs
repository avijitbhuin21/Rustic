//! Local-network / peer sync commands (issue #15). Thin Tauri wrappers over
//! `rustic_app::peer::ops` with the desktop host (`lan::DesktopPeerHost`).

use tauri::{AppHandle, Manager, State};

use crate::lan::{self, ops, DesktopPeerHost, LanState};

pub use ops::{LanDevice, LanStatus, LocalMetaItem, SyncItemsResult, SyncProject};

#[tauri::command]
pub async fn lan_status(app: AppHandle, lan: State<'_, LanState>) -> Result<LanStatus, String> {
    ops::status(&DesktopPeerHost::arc(&app), lan.inner()).await
}

/// Switch "Allow local-network sync" on/off (persisted across restarts).
#[tauri::command]
pub async fn lan_set_enabled(
    app: AppHandle,
    lan: State<'_, LanState>,
    enabled: bool,
) -> Result<(), String> {
    ops::set_enabled(&DesktopPeerHost::arc(&app), lan.inner(), enabled).await
}

/// Discovered, manually added and paired devices.
#[tauri::command]
pub async fn lan_devices(
    app: AppHandle,
    lan: State<'_, LanState>,
) -> Result<Vec<LanDevice>, String> {
    ops::devices(&DesktopPeerHost::arc(&app), lan.inner()).await
}

/// "Add machine" by address (LAN `ip[:port]`, tunnel / server URL).
#[tauri::command]
pub async fn lan_add_manual(
    app: AppHandle,
    lan: State<'_, LanState>,
    address: String,
) -> Result<LanDevice, String> {
    ops::add_manual(&DesktopPeerHost::arc(&app), lan.inner(), &address).await
}

/// Set (or clear, when empty) the local nickname for a paired device.
#[tauri::command]
pub async fn lan_rename(app: AppHandle, device_id: String, nickname: String) -> Result<(), String> {
    ops::rename(&DesktopPeerHost::arc(&app), &device_id, &nickname)
}

/// Rename this machine (what other devices see).
#[tauri::command]
pub async fn lan_set_device_name(
    app: AppHandle,
    lan: State<'_, LanState>,
    name: String,
) -> Result<(), String> {
    ops::set_device_name(&DesktopPeerHost::arc(&app), lan.inner(), &name).await
}

/// The code this machine shows while pairing with `device_id`.
#[tauri::command]
pub async fn lan_pair_code(lan: State<'_, LanState>, device_id: String) -> Result<String, String> {
    ops::pair_code(lan.inner(), &device_id)
}

/// Ask `device_id` to pair. Waits for the other machine's Accept/Decline.
#[tauri::command]
pub async fn lan_pair(
    app: AppHandle,
    lan: State<'_, LanState>,
    device_id: String,
) -> Result<String, String> {
    ops::pair(&DesktopPeerHost::arc(&app), lan.inner(), &device_id).await
}

/// Answer an incoming pairing request shown by the global prompt.
#[tauri::command]
pub async fn lan_respond_pair(
    lan: State<'_, LanState>,
    request_id: String,
    accept: bool,
) -> Result<(), String> {
    ops::respond_pair(lan.inner(), &request_id, accept)
}

/// Forget a paired device (it must pair again to sync).
#[tauri::command]
pub async fn lan_forget(app: AppHandle, device_id: String) -> Result<(), String> {
    ops::forget(&DesktopPeerHost::arc(&app), &device_id)
}

/// Push to a paired device: everything, or one project.
#[tauri::command]
pub async fn lan_push(
    app: AppHandle,
    lan: State<'_, LanState>,
    device_id: String,
    project_id: Option<String>,
) -> Result<String, String> {
    ops::push(&DesktopPeerHost::arc(&app), lan.inner(), &device_id, project_id).await
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
    ops::pull(&DesktopPeerHost::arc(&app), lan.inner(), &device_id, project_id, target_parent).await
}

/// Projects a paired device shares with us.
#[tauri::command]
pub async fn lan_list_projects(
    app: AppHandle,
    lan: State<'_, LanState>,
    device_id: String,
) -> Result<Vec<lan::client::RemoteProject>, String> {
    ops::list_projects(&DesktopPeerHost::arc(&app), lan.inner(), &device_id).await
}

/// Metadata diff against a paired device.
#[tauri::command]
pub async fn lan_meta_preview(
    app: AppHandle,
    lan: State<'_, LanState>,
    device_id: String,
    direction: String,
) -> Result<Vec<rustic_app::meta_sync::MetaDiffEntry>, String> {
    ops::meta_preview(&DesktopPeerHost::arc(&app), lan.inner(), &device_id, &direction).await
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
    ops::meta_apply(&DesktopPeerHost::arc(&app), lan.inner(), &device_id, &direction, overwrite).await
}

/// Called from app setup: restore "Allow local-network sync" (and the
/// Cloudflare tunnel) if they were on.
pub fn restore_on_startup(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let lan = app.state::<LanState>().inner().clone();
        ops::restore(&DesktopPeerHost::arc(&app), &lan).await;
    });
}

/// This machine's metadata items, without their content.
#[tauri::command]
pub async fn lan_local_meta(app: AppHandle) -> Result<Vec<LocalMetaItem>, String> {
    ops::local_meta(&DesktopPeerHost::arc(&app)).await
}

/// What this machine shares with a paired device.
#[tauri::command]
pub async fn lan_get_share(app: AppHandle, device_id: String) -> Result<lan::Share, String> {
    ops::get_share(&DesktopPeerHost::arc(&app), &device_id)
}

/// Replace what this machine shares with a paired device.
#[tauri::command]
pub async fn lan_set_share(app: AppHandle, device_id: String, share: lan::Share) -> Result<(), String> {
    ops::set_share(&DesktopPeerHost::arc(&app), &device_id, share)
}

/// Answer an incoming push / pull approval prompt.
#[tauri::command]
pub async fn lan_respond_transfer(
    lan: State<'_, LanState>,
    request_id: String,
    accept: bool,
) -> Result<(), String> {
    ops::respond_transfer(lan.inner(), &request_id, accept)
}

/// Push or pull the picked projects and metadata items with a paired device
/// (approved on the other machine first).
#[tauri::command]
pub async fn lan_sync_items(
    app: AppHandle,
    lan: State<'_, LanState>,
    device_id: String,
    direction: String,
    projects: Vec<SyncProject>,
    meta: Vec<lan::consent::RequestedMeta>,
) -> Result<SyncItemsResult, String> {
    ops::sync_items(&DesktopPeerHost::arc(&app), lan.inner(), &device_id, &direction, projects, meta).await
}

/// `"cloudflare"` | `"portforward"` | `"off"`; returns the tunnel URL in
/// Cloudflare mode.
#[tauri::command]
pub async fn lan_set_internet_mode(
    app: AppHandle,
    lan: State<'_, LanState>,
    mode: String,
) -> Result<Option<String>, String> {
    ops::set_internet_mode(&DesktopPeerHost::arc(&app), lan.inner(), &mode).await
}

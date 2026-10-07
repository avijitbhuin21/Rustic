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
pub async fn lan_pair_code(
    app: AppHandle,
    lan: State<'_, LanState>,
    device_id: String,
) -> Result<String, String> {
    ops::pair_code_for(&DesktopPeerHost::arc(&app), lan.inner(), &device_id)
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

/// Forget a paired device (it must pair again to sync); tells it first.
#[tauri::command]
pub async fn lan_forget(app: AppHandle, lan: State<'_, LanState>, device_id: String) -> Result<(), String> {
    ops::forget(&DesktopPeerHost::arc(&app), lan.inner(), &device_id).await
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


/// Stop waiting on our pairing / approval request to `device_id` (dismisses its prompt).
#[tauri::command]
pub async fn lan_cancel_outgoing(lan: State<'_, LanState>, device_id: String) -> Result<(), String> {
    ops::cancel_outgoing(lan.inner(), &device_id)
}

/// One folder of a project a paired device shares with us.
#[tauri::command]
pub async fn lan_list_files(
    app: AppHandle,
    lan: State<'_, LanState>,
    device_id: String,
    project_id: String,
    path: Option<String>,
) -> Result<Vec<lan::files::FsEntry>, String> {
    ops::list_files(&DesktopPeerHost::arc(&app), lan.inner(), &device_id, &project_id, path.as_deref().unwrap_or("")).await
}

/// One file of a shared project, loaded for the preview pane.
#[tauri::command]
pub async fn lan_preview_file(
    app: AppHandle,
    lan: State<'_, LanState>,
    device_id: String,
    project_id: String,
    path: String,
) -> Result<lan::files::Preview, String> {
    ops::preview_file(&DesktopPeerHost::arc(&app), lan.inner(), &device_id, &project_id, &path).await
}

/// Total size of a selection on a paired device.
#[tauri::command]
pub async fn lan_remote_size(
    app: AppHandle,
    lan: State<'_, LanState>,
    device_id: String,
    items: Vec<lan::consent::RequestedFile>,
) -> Result<lan::files::SizeInfo, String> {
    ops::remote_size(&DesktopPeerHost::arc(&app), lan.inner(), &device_id, &items).await
}

/// Names a pull into `dest_dir` would collide with.
#[tauri::command]
pub async fn lan_local_conflicts(dest_dir: String, items: Vec<lan::consent::RequestedFile>) -> Result<Vec<String>, String> {
    Ok(ops::local_conflicts(&dest_dir, &items))
}

/// Names an upload into a paired device's folder would collide with.
#[tauri::command]
pub async fn lan_remote_conflicts(
    app: AppHandle,
    lan: State<'_, LanState>,
    device_id: String,
    project_id: String,
    dir: Option<String>,
    names: Vec<String>,
) -> Result<Vec<String>, String> {
    ops::remote_conflicts(&DesktopPeerHost::arc(&app), lan.inner(), &device_id, &project_id, dir.as_deref().unwrap_or(""), names).await
}

/// Pull files / folders from a paired device into `dest_dir` (one approval).
#[tauri::command]
pub async fn lan_pull_files(
    app: AppHandle,
    lan: State<'_, LanState>,
    device_id: String,
    items: Vec<lan::consent::RequestedFile>,
    dest_dir: String,
    opts: Option<ops::FileTransferOpts>,
) -> Result<lan::files::UnpackSummary, String> {
    ops::pull_files(&DesktopPeerHost::arc(&app), lan.inner(), &device_id, items, dest_dir, opts.unwrap_or_default()).await
}

/// Upload local files / folders to a paired device's project folder (one approval).
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn lan_push_files(
    app: AppHandle,
    lan: State<'_, LanState>,
    device_id: String,
    local_paths: Vec<String>,
    project_id: String,
    dir: Option<String>,
    opts: Option<ops::FileTransferOpts>,
) -> Result<serde_json::Value, String> {
    ops::push_files(
        &DesktopPeerHost::arc(&app),
        lan.inner(),
        &device_id,
        local_paths,
        project_id,
        dir.unwrap_or_default(),
        opts.unwrap_or_default(),
    )
    .await
}

/// Ask a paired device for one-time permission to browse its metadata.
#[tauri::command]
pub async fn lan_request_meta_access(app: AppHandle, lan: State<'_, LanState>, device_id: String) -> Result<(), String> {
    ops::request_meta_access(&DesktopPeerHost::arc(&app), lan.inner(), &device_id).await
}

/// A paired device's metadata for browsing (`granted` + items).
#[tauri::command]
pub async fn lan_meta_browse(app: AppHandle, lan: State<'_, LanState>, device_id: String) -> Result<serde_json::Value, String> {
    ops::meta_browse(&DesktopPeerHost::arc(&app), lan.inner(), &device_id).await
}

/// Grant / revoke a paired device's metadata browsing on this machine.
#[tauri::command]
pub async fn lan_set_meta_view(app: AppHandle, device_id: String, allowed: bool) -> Result<(), String> {
    ops::set_meta_view(&DesktopPeerHost::arc(&app), &device_id, allowed)
}

/// Whether a paired device may browse this machine's metadata.
#[tauri::command]
pub async fn lan_get_meta_view(app: AppHandle, device_id: String) -> Result<bool, String> {
    ops::paired(&DesktopPeerHost::arc(&app), &device_id).map(|p| p.meta_view)
}

/// Check in with every paired device now.
#[tauri::command]
pub async fn lan_announce(app: AppHandle, lan: State<'_, LanState>) -> Result<(), String> {
    ops::announce(&DesktopPeerHost::arc(&app), lan.inner()).await;
    Ok(())
}

/// Running and finished transfers (tray).
#[tauri::command]
pub async fn lan_transfers() -> Result<Vec<rustic_app::transfers::TransferInfo>, String> {
    Ok(rustic_app::transfers::list())
}

/// Cancel a running transfer.
#[tauri::command]
pub async fn lan_transfer_cancel(id: String) -> Result<(), String> {
    rustic_app::transfers::cancel(&id)
}

/// Pause a running file transfer (resumable).
#[tauri::command]
pub async fn lan_transfer_pause(id: String) -> Result<(), String> {
    rustic_app::transfers::pause(&id)
}

/// Resume a paused file transfer.
#[tauri::command]
pub async fn lan_transfer_resume(id: String) -> Result<(), String> {
    rustic_app::transfers::resume(&id)
}

/// Clear one finished transfer (or all finished when `id` is omitted).
#[tauri::command]
pub async fn lan_transfer_clear(id: Option<String>) -> Result<(), String> {
    rustic_app::transfers::clear(id.as_deref());
    Ok(())
}

/// This build's version (what peers must match).
#[tauri::command]
pub async fn lan_version() -> Result<String, String> {
    Ok(lan::app_version().to_string())
}
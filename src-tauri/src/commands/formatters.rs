//! External formatter management — desktop adapter.
//!
//! All logic lives in `rustic_app::host_ops::formatters` (shared with the
//! server host); this file only resolves the profile-scoped app data dir and
//! exposes the Tauri commands.

use crate::state::AppState;
use rustic_app::host_ops::formatters as ops;
use std::path::PathBuf;
use tauri::{AppHandle, State};

pub use ops::{
    BuiltinDescriptor, CustomFormatter, FormatRequest, FormatResponse, FormatterListEntry,
    InstalledRecord, UpdateInfo,
};

fn data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    crate::app_paths::app_data_dir(app).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn formatter_registry() -> Vec<BuiltinDescriptor> {
    ops::registry()
}

#[tauri::command(async)]
pub fn formatter_list(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<FormatterListEntry>, String> {
    ops::list(&state, &data_dir(&app)?)
}

#[tauri::command]
pub async fn formatter_install(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<InstalledRecord, String> {
    ops::install(&state, &data_dir(&app)?, &id).await
}

#[tauri::command]
pub async fn formatter_update(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<InstalledRecord, String> {
    // Same as install: always fetches the latest release and overwrites.
    ops::install(&state, &data_dir(&app)?, &id).await
}

#[tauri::command]
pub async fn formatter_check_update(id: String) -> Result<UpdateInfo, String> {
    ops::check_update(&id).await
}

#[tauri::command]
pub fn formatter_uninstall(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    ops::uninstall(&state, &data_dir(&app)?, &id)
}

#[tauri::command]
pub fn formatter_add_custom(
    state: State<'_, AppState>,
    formatter: CustomFormatter,
) -> Result<(), String> {
    ops::add_custom(&state, formatter)
}

#[tauri::command]
pub fn formatter_update_custom(
    state: State<'_, AppState>,
    formatter: CustomFormatter,
) -> Result<(), String> {
    ops::update_custom(&state, formatter)
}

#[tauri::command]
pub fn formatter_remove_custom(state: State<'_, AppState>, id: String) -> Result<(), String> {
    ops::remove_custom(&state, &id)
}

#[tauri::command]
pub async fn formatter_format(
    app: AppHandle,
    state: State<'_, AppState>,
    req: FormatRequest,
) -> Result<FormatResponse, String> {
    ops::format(&state, &data_dir(&app)?, req).await
}

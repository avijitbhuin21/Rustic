//! Settings / themes / keybindings — desktop adapter. Legacy settings logic
//! lives in `rustic_app::host_ops::settings`, theme packs in `rustic_app::themes`.

use crate::state::AppState;
use rustic_app::host_ops::settings as ops;
use rustic_core::config::{Keybinding, Theme, UserSettings};
use tauri::State;

pub use ops::{ThemeInfo, VsCodeDetection};

#[tauri::command(async)]
pub fn get_settings(state: State<'_, AppState>) -> Result<UserSettings, String> {
    ops::get_settings(&state)
}

#[tauri::command(async)]
pub fn update_settings(state: State<'_, AppState>, settings: UserSettings) -> Result<(), String> {
    ops::update_settings(&state, &settings)
}

#[tauri::command(async)]
pub fn get_active_theme(state: State<'_, AppState>) -> Result<Theme, String> {
    ops::get_active_theme(&state)
}

#[tauri::command(async)]
pub fn list_themes(state: State<'_, AppState>) -> Result<Vec<ThemeInfo>, String> {
    ops::list_themes(&state)
}

#[tauri::command(async)]
pub fn import_theme(state: State<'_, AppState>, path: String) -> Result<Theme, String> {
    ops::import_theme(&state, &path)
}

#[tauri::command(async)]
pub fn import_theme_json(state: State<'_, AppState>, json: String) -> Result<Theme, String> {
    ops::import_theme_json(&state, &json)
}

#[tauri::command(async)]
pub fn get_theme(state: State<'_, AppState>, name: String) -> Result<Theme, String> {
    ops::get_theme(&state, &name)
}

#[tauri::command(async)]
pub fn delete_theme(state: State<'_, AppState>, name: String) -> Result<(), String> {
    ops::delete_theme(&state, &name)
}

#[tauri::command(async)]
pub fn import_keybindings(
    state: State<'_, AppState>,
    path: String,
) -> Result<Vec<Keybinding>, String> {
    ops::import_keybindings(&state, &path)
}

#[tauri::command(async)]
pub fn detect_vscode_keybindings() -> Result<VsCodeDetection, String> {
    Ok(ops::detect_vscode_keybindings())
}

// ---- Theme packs (format 2, file-based; see rustic_app::themes) ----

fn theme_dir(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    crate::app_paths::app_data_dir(app).map_err(|e| e.to_string())
}

/// Built-in + installed themes.
#[tauri::command]
pub fn theme_list(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<rustic_app::themes::Entry>, String> {
    Ok(rustic_app::themes::list(&state, &theme_dir(&app)?))
}

/// One theme with its file text, hash and scan report.
#[tauri::command]
pub fn theme_get(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<rustic_app::themes::Detail, String> {
    rustic_app::themes::get(&state, &theme_dir(&app)?, &id)
}

/// Install a theme from a file (untrusted until the user trusts it).
#[tauri::command]
pub fn theme_import_file(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    path: String,
) -> Result<rustic_app::themes::Detail, String> {
    rustic_app::themes::import_path(&state, &theme_dir(&app)?, &path)
}

/// Install a theme from its text (paste / browser upload).
#[tauri::command]
pub fn theme_import_text(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    text: String,
) -> Result<rustic_app::themes::Detail, String> {
    rustic_app::themes::import_text(&state, &theme_dir(&app)?, &text)
}

/// Replace an installed theme's file (drops its trust).
#[tauri::command]
pub fn theme_write(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    id: String,
    text: String,
) -> Result<rustic_app::themes::Detail, String> {
    rustic_app::themes::write(&state, &theme_dir(&app)?, &id, &text)
}

/// Save a theme's file to `path`.
#[tauri::command]
pub fn theme_export(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    id: String,
    path: String,
) -> Result<(), String> {
    let d = rustic_app::themes::get(&state, &theme_dir(&app)?, &id)?;
    std::fs::write(&path, d.text).map_err(|e| format!("Couldn't save {path}: {e}"))
}

/// The starter theme file; also saved to `path` when given.
#[tauri::command]
pub fn theme_template(path: Option<String>) -> Result<String, String> {
    let text = rustic_app::themes::template();
    if let Some(path) = path {
        std::fs::write(&path, &text).map_err(|e| format!("Couldn't save {path}: {e}"))?;
    }
    Ok(text)
}

/// Delete an installed theme.
#[tauri::command]
pub fn theme_delete(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    rustic_app::themes::delete(&state, &theme_dir(&app)?, &id)
}

/// Trust the exact theme file the user reviewed (`hash`).
#[tauri::command]
pub fn theme_trust(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    id: String,
    hash: String,
) -> Result<(), String> {
    rustic_app::themes::trust(&state, &theme_dir(&app)?, &id, &hash)
}

/// Make a built-in / trusted theme active.
#[tauri::command]
pub fn theme_apply(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    rustic_app::themes::apply(&state, &theme_dir(&app)?, &id)
}

/// `"dark" | "light" | "system"`.
#[tauri::command]
pub fn theme_set_mode(state: State<'_, AppState>, mode: String) -> Result<(), String> {
    rustic_app::themes::set_mode(&state, &mode)
}

/// The active theme, resolved for painting.
#[tauri::command]
pub fn theme_active(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<rustic_app::themes::Active, String> {
    Ok(rustic_app::themes::active(&state, &theme_dir(&app)?))
}

/// Folder holding installed theme files.
#[tauri::command]
pub fn theme_folder(app: tauri::AppHandle) -> Result<String, String> {
    let d = rustic_app::themes::dir(&theme_dir(&app)?);
    std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    Ok(d.to_string_lossy().into_owned())
}

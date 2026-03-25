use crate::state::AppState;
use rustic_core::config::{KeybindingSet, Theme, UserSettings};
use serde::{Deserialize, Serialize};
use tauri::State;

#[derive(Clone, Serialize, Deserialize)]
pub struct ThemeInfo {
    pub name: String,
    pub kind: String,
    pub is_builtin: bool,
}

/// Get the full user settings from SQLite.
#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Result<UserSettings, String> {
    let db = state.db.lock().unwrap();
    let json = db.get_setting("user_settings").map_err(|e| e.to_string())?;
    match json {
        Some(j) => serde_json::from_str(&j).map_err(|e| format!("Invalid settings JSON: {}", e)),
        None => Ok(UserSettings::default()),
    }
}

/// Update user settings (full replace).
#[tauri::command]
pub fn update_settings(state: State<'_, AppState>, settings: UserSettings) -> Result<(), String> {
    let db = state.db.lock().unwrap();
    let json = serde_json::to_string(&settings).map_err(|e| e.to_string())?;
    db.set_setting("user_settings", &json).map_err(|e| e.to_string())
}

/// Get the active theme (resolved from settings).
#[tauri::command]
pub fn get_active_theme(state: State<'_, AppState>) -> Result<Theme, String> {
    let db = state.db.lock().unwrap();
    let settings: UserSettings = match db.get_setting("user_settings").map_err(|e| e.to_string())? {
        Some(j) => serde_json::from_str(&j).map_err(|e| e.to_string())?,
        None => UserSettings::default(),
    };

    // Try built-in first
    if let Some(theme) = Theme::builtin(&settings.theme.active_theme) {
        return Ok(theme);
    }

    // Try custom theme from DB
    let key = format!("theme:{}", settings.theme.active_theme);
    match db.get_setting(&key).map_err(|e| e.to_string())? {
        Some(j) => serde_json::from_str(&j).map_err(|e| e.to_string()),
        None => Ok(Theme::gruvbox_dark()), // fallback
    }
}

/// List all available themes (built-in + custom).
#[tauri::command]
pub fn list_themes(state: State<'_, AppState>) -> Result<Vec<ThemeInfo>, String> {
    let db = state.db.lock().unwrap();
    let mut themes: Vec<ThemeInfo> = Theme::builtin_names()
        .into_iter()
        .map(|name| ThemeInfo {
            name: name.to_string(),
            kind: if name.contains("Light") { "light".to_string() } else { "dark".to_string() },
            is_builtin: true,
        })
        .collect();

    // Load custom theme names from settings
    let settings: UserSettings = match db.get_setting("user_settings").map_err(|e| e.to_string())? {
        Some(j) => serde_json::from_str(&j).map_err(|e| e.to_string())?,
        None => UserSettings::default(),
    };
    for name in &settings.theme.custom_themes {
        themes.push(ThemeInfo {
            name: name.clone(),
            kind: "dark".to_string(),
            is_builtin: false,
        });
    }

    Ok(themes)
}

/// Import a theme from a file path (TOML or JSON).
#[tauri::command]
pub fn import_theme(state: State<'_, AppState>, path: String) -> Result<Theme, String> {
    let content = std::fs::read_to_string(&path)
        .map_err(|e| format!("Failed to read theme file: {}", e))?;

    let theme = if path.ends_with(".toml") {
        Theme::from_toml(&content)?
    } else {
        Theme::from_json(&content)?
    };

    // Store in DB
    let db = state.db.lock().unwrap();
    let key = format!("theme:{}", theme.name);
    let json = serde_json::to_string(&theme).map_err(|e| e.to_string())?;
    db.set_setting(&key, &json).map_err(|e| e.to_string())?;

    // Add to custom themes list in settings
    let mut settings: UserSettings = match db.get_setting("user_settings").map_err(|e| e.to_string())? {
        Some(j) => serde_json::from_str(&j).map_err(|e| e.to_string())?,
        None => UserSettings::default(),
    };
    if !settings.theme.custom_themes.contains(&theme.name) {
        settings.theme.custom_themes.push(theme.name.clone());
        let settings_json = serde_json::to_string(&settings).map_err(|e| e.to_string())?;
        db.set_setting("user_settings", &settings_json).map_err(|e| e.to_string())?;
    }

    Ok(theme)
}

/// Import keybindings from a VS Code-compatible JSON file.
#[tauri::command]
pub fn import_keybindings(state: State<'_, AppState>, path: String) -> Result<Vec<rustic_core::config::Keybinding>, String> {
    let content = std::fs::read_to_string(&path)
        .map_err(|e| format!("Failed to read keybindings file: {}", e))?;

    let keybinding_set = KeybindingSet::from_vscode_json(&content)?;

    // Merge into settings
    let db = state.db.lock().unwrap();
    let mut settings: UserSettings = match db.get_setting("user_settings").map_err(|e| e.to_string())? {
        Some(j) => serde_json::from_str(&j).map_err(|e| e.to_string())?,
        None => UserSettings::default(),
    };
    settings.keybindings = keybinding_set.bindings.clone();
    let json = serde_json::to_string(&settings).map_err(|e| e.to_string())?;
    db.set_setting("user_settings", &json).map_err(|e| e.to_string())?;

    Ok(keybinding_set.bindings)
}

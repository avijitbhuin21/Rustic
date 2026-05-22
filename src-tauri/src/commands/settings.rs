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

    if let Some(theme) = Theme::builtin(&settings.theme.active_theme) {
        return Ok(theme);
    }

    let key = format!("theme:{}", settings.theme.active_theme);
    match db.get_setting(&key).map_err(|e| e.to_string())? {
        Some(j) => serde_json::from_str(&j).map_err(|e| e.to_string()),
        None => Ok(Theme::obsidian()), // fallback
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

    let db = state.db.lock().unwrap();
    let key = format!("theme:{}", theme.name);
    let json = serde_json::to_string(&theme).map_err(|e| e.to_string())?;
    db.set_setting(&key, &json).map_err(|e| e.to_string())?;

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

/// Import a theme directly from a JSON string (paste in UI).
#[tauri::command]
pub fn import_theme_json(state: State<'_, AppState>, json: String) -> Result<Theme, String> {
    let theme = Theme::from_json(&json)?;
    let db = state.db.lock().unwrap();
    let key = format!("theme:{}", theme.name);
    let serialized = serde_json::to_string(&theme).map_err(|e| e.to_string())?;
    db.set_setting(&key, &serialized).map_err(|e| e.to_string())?;
    let mut settings: UserSettings = match db.get_setting("user_settings").map_err(|e| e.to_string())? {
        Some(j) => serde_json::from_str(&j).map_err(|e| e.to_string())?,
        None => UserSettings::default(),
    };
    if !settings.theme.custom_themes.contains(&theme.name) {
        settings.theme.custom_themes.push(theme.name.clone());
        let json = serde_json::to_string(&settings).map_err(|e| e.to_string())?;
        db.set_setting("user_settings", &json).map_err(|e| e.to_string())?;
    }
    Ok(theme)
}

/// Get a single theme by name (built-in or custom).
#[tauri::command]
pub fn get_theme(state: State<'_, AppState>, name: String) -> Result<Theme, String> {
    if let Some(theme) = Theme::builtin(&name) {
        return Ok(theme);
    }
    let db = state.db.lock().unwrap();
    let key = format!("theme:{}", name);
    match db.get_setting(&key).map_err(|e| e.to_string())? {
        Some(j) => serde_json::from_str(&j).map_err(|e| e.to_string()),
        None => Err(format!("Theme '{}' not found", name)),
    }
}

/// Delete a custom theme by name.
#[tauri::command]
pub fn delete_theme(state: State<'_, AppState>, name: String) -> Result<(), String> {
    let db = state.db.lock().unwrap();
    db.delete_setting(&format!("theme:{}", name)).map_err(|e| e.to_string())?;
    let mut settings: UserSettings = match db.get_setting("user_settings").map_err(|e| e.to_string())? {
        Some(j) => serde_json::from_str(&j).map_err(|e| e.to_string())?,
        None => UserSettings::default(),
    };
    settings.theme.custom_themes.retain(|n| n != &name);
    if settings.theme.active_theme == name {
        settings.theme.active_theme = "Obsidian".to_string();
    }
    let json = serde_json::to_string(&settings).map_err(|e| e.to_string())?;
    db.set_setting("user_settings", &json).map_err(|e| e.to_string())
}

/// Import keybindings from a VS Code-compatible JSON file.
#[tauri::command]
pub fn import_keybindings(state: State<'_, AppState>, path: String) -> Result<Vec<rustic_core::config::Keybinding>, String> {
    let content = std::fs::read_to_string(&path)
        .map_err(|e| format!("Failed to read keybindings file: {}", e))?;

    let keybinding_set = KeybindingSet::from_vscode_json(&content)?;

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

#[derive(Clone, Serialize, Deserialize)]
pub struct VsCodeVariant {
    pub name: String,
    pub path: String,
    /// Number of parsed keybinding overrides; shown as "12 shortcuts" in the UI.
    pub binding_count: usize,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct VsCodeDetection {
    /// Variants whose `keybindings.json` exists and parsed.
    pub importable: Vec<VsCodeVariant>,
    /// Variants detected but with no `keybindings.json` (VS Code writes it lazily).
    pub detected_without_overrides: Vec<String>,
}

#[tauri::command]
pub fn detect_vscode_keybindings() -> Result<VsCodeDetection, String> {
    const VARIANTS: &[(&str, &str)] = &[
        ("Visual Studio Code", "Code"),
        ("VS Code Insiders", "Code - Insiders"),
        ("Code - OSS", "Code - OSS"),
        ("VSCodium", "VSCodium"),
        ("Cursor", "Cursor"),
        ("Windsurf", "Windsurf"),
    ];

    let bases = vscode_config_bases();
    let mut importable = Vec::new();
    let mut detected_without_overrides = Vec::new();
    for (display, folder) in VARIANTS {
        for base in &bases {
            let user_dir = base.join(folder).join("User");
            if !user_dir.is_dir() {
                continue;
            }
            let path = user_dir.join("keybindings.json");
            if !path.is_file() {
                detected_without_overrides.push((*display).to_string());
                break;
            }
            let content = match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let count = match KeybindingSet::from_vscode_json(&content) {
                Ok(set) => set.bindings.len(),
                Err(_) => continue,
            };
            importable.push(VsCodeVariant {
                name: (*display).to_string(),
                path: path.to_string_lossy().to_string(),
                binding_count: count,
            });
            break; // first matching base wins
        }
    }
    Ok(VsCodeDetection { importable, detected_without_overrides })
}

/// Returns all candidate config roots (Linux snap/flatpak vs apt differ).
fn vscode_config_bases() -> Vec<std::path::PathBuf> {
    let mut bases = Vec::new();
    #[cfg(target_os = "windows")]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            bases.push(std::path::PathBuf::from(appdata));
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) {
            bases.push(home.join("Library").join("Application Support"));
        }
    }
    #[cfg(target_os = "linux")]
    {
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            bases.push(std::path::PathBuf::from(xdg));
        }
        if let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) {
            bases.push(home.join(".config"));
        }
    }
    bases
}

//! Legacy user settings / themes / keybindings stored in SQLite
//! (`user_settings`, `theme:<name>`) — shared by both hosts. Theme packs
//! (format 2) live in `crate::themes`.

use rustic_core::config::{Keybinding, KeybindingSet, Theme, UserSettings};
use rustic_db::Database;
use serde::{Deserialize, Serialize};

use crate::state::AppState;
use crate::sync_ext::MutexExt;

#[derive(Clone, Serialize, Deserialize)]
pub struct ThemeInfo {
    pub name: String,
    pub kind: String,
    pub is_builtin: bool,
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

fn load(db: &Database) -> Result<UserSettings, String> {
    match db.get_setting("user_settings").map_err(|e| e.to_string())? {
        Some(j) => serde_json::from_str(&j).map_err(|e| e.to_string()),
        None => Ok(UserSettings::default()),
    }
}

fn save(db: &Database, settings: &UserSettings) -> Result<(), String> {
    let json = serde_json::to_string(settings).map_err(|e| e.to_string())?;
    db.set_setting("user_settings", &json)
        .map_err(|e| e.to_string())
}

pub fn get_settings(state: &AppState) -> Result<UserSettings, String> {
    let db = state.db.lock_safe();
    match db.get_setting("user_settings").map_err(|e| e.to_string())? {
        Some(j) => serde_json::from_str(&j).map_err(|e| format!("Invalid settings JSON: {}", e)),
        None => Ok(UserSettings::default()),
    }
}

/// Full replace.
pub fn update_settings(state: &AppState, settings: &UserSettings) -> Result<(), String> {
    save(&state.db.lock_safe(), settings)
}

pub fn get_active_theme(state: &AppState) -> Result<Theme, String> {
    let db = state.db.lock_safe();
    let settings = load(&db)?;
    if let Some(theme) = Theme::builtin(&settings.theme.active_theme) {
        return Ok(theme);
    }
    match db
        .get_setting(&format!("theme:{}", settings.theme.active_theme))
        .map_err(|e| e.to_string())?
    {
        Some(j) => serde_json::from_str(&j).map_err(|e| e.to_string()),
        None => Ok(Theme::obsidian()),
    }
}

pub fn list_themes(state: &AppState) -> Result<Vec<ThemeInfo>, String> {
    let db = state.db.lock_safe();
    let mut themes: Vec<ThemeInfo> = Theme::builtin_names()
        .into_iter()
        .map(|name| ThemeInfo {
            name: name.to_string(),
            kind: if name.contains("Light") {
                "light"
            } else {
                "dark"
            }
            .to_string(),
            is_builtin: true,
        })
        .collect();
    for name in load(&db)?.theme.custom_themes {
        themes.push(ThemeInfo {
            name,
            kind: "dark".to_string(),
            is_builtin: false,
        });
    }
    Ok(themes)
}

/// Store a custom theme and register it in `custom_themes`.
fn store_theme(state: &AppState, theme: &Theme) -> Result<(), String> {
    let db = state.db.lock_safe();
    let json = serde_json::to_string(theme).map_err(|e| e.to_string())?;
    db.set_setting(&format!("theme:{}", theme.name), &json)
        .map_err(|e| e.to_string())?;
    let mut settings = load(&db)?;
    if !settings.theme.custom_themes.contains(&theme.name) {
        settings.theme.custom_themes.push(theme.name.clone());
        save(&db, &settings)?;
    }
    Ok(())
}

/// Import a theme from a file path (TOML or JSON).
pub fn import_theme(state: &AppState, path: &str) -> Result<Theme, String> {
    let content =
        std::fs::read_to_string(path).map_err(|e| format!("Failed to read theme file: {}", e))?;
    let theme = if path.ends_with(".toml") {
        Theme::from_toml(&content)?
    } else {
        Theme::from_json(&content)?
    };
    store_theme(state, &theme)?;
    Ok(theme)
}

/// Import a theme from a JSON string (paste in UI).
pub fn import_theme_json(state: &AppState, json: &str) -> Result<Theme, String> {
    let theme = Theme::from_json(json)?;
    store_theme(state, &theme)?;
    Ok(theme)
}

pub fn get_theme(state: &AppState, name: &str) -> Result<Theme, String> {
    if let Some(theme) = Theme::builtin(name) {
        return Ok(theme);
    }
    let db = state.db.lock_safe();
    match db
        .get_setting(&format!("theme:{}", name))
        .map_err(|e| e.to_string())?
    {
        Some(j) => serde_json::from_str(&j).map_err(|e| e.to_string()),
        None => Err(format!("Theme '{}' not found", name)),
    }
}

pub fn delete_theme(state: &AppState, name: &str) -> Result<(), String> {
    let db = state.db.lock_safe();
    db.delete_setting(&format!("theme:{}", name))
        .map_err(|e| e.to_string())?;
    let mut settings = load(&db)?;
    settings.theme.custom_themes.retain(|n| n != name);
    if settings.theme.active_theme == name {
        settings.theme.active_theme = "Obsidian".to_string();
    }
    save(&db, &settings)
}

/// Import keybindings from a VS Code-compatible JSON file.
pub fn import_keybindings(state: &AppState, path: &str) -> Result<Vec<Keybinding>, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read keybindings file: {}", e))?;
    let keybinding_set = KeybindingSet::from_vscode_json(&content)?;
    let db = state.db.lock_safe();
    let mut settings = load(&db)?;
    settings.keybindings = keybinding_set.bindings.clone();
    save(&db, &settings)?;
    Ok(keybinding_set.bindings)
}

pub fn detect_vscode_keybindings() -> VsCodeDetection {
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
            let Ok(content) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(set) = KeybindingSet::from_vscode_json(&content) else {
                continue;
            };
            importable.push(VsCodeVariant {
                name: (*display).to_string(),
                path: path.to_string_lossy().to_string(),
                binding_count: set.bindings.len(),
            });
            break; // first matching base wins
        }
    }
    VsCodeDetection {
        importable,
        detected_without_overrides,
    }
}

/// All candidate config roots (Linux snap/flatpak vs apt differ).
fn vscode_config_bases() -> Vec<std::path::PathBuf> {
    #[allow(unused_mut)]
    let mut bases = Vec::new();
    #[cfg(target_os = "windows")]
    if let Ok(appdata) = std::env::var("APPDATA") {
        bases.push(std::path::PathBuf::from(appdata));
    }
    #[cfg(target_os = "macos")]
    if let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) {
        bases.push(home.join("Library").join("Application Support"));
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

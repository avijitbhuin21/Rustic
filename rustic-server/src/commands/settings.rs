//! settings commands — server dispatch.
//!
//! Mirrors `src-tauri/src/commands/settings.rs`. Settings/themes/keybindings
//! persist through the same SQLite keys (`user_settings`, `theme:<name>`) the
//! desktop uses, calling the identical `rustic_core::config` functions.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use rustic_app::context::AppContext;
use rustic_app::state::AppState;
use rustic_app::sync_ext::MutexExt;
use rustic_core::config::{KeybindingSet, Theme, UserSettings};

use crate::api::{ok, parse, ApiError, PathArg};
use crate::context::{ServerContext, TunnelConfig};

pub async fn dispatch(
    ctx: &ServerContext,
    command: &str,
    args: &Value,
) -> Option<Result<Value, ApiError>> {
    Some(match command {
        "get_settings" => get_settings(ctx.state()),
        "update_settings" => update_settings(ctx.state(), args),
        "get_active_theme" => get_active_theme(ctx.state()),
        "list_themes" => list_themes(ctx.state()),
        "import_theme" => import_theme(ctx.state(), args),
        "import_theme_json" => import_theme_json(ctx.state(), args),
        "get_theme" => get_theme(ctx.state(), args),
        "delete_theme" => delete_theme(ctx.state(), args),
        "import_keybindings" => import_keybindings(ctx.state(), args),
        "detect_vscode_keybindings" => detect_vscode_keybindings(),
        "get_tunnel_config" => get_tunnel_config(ctx),
        "set_tunnel_config" => set_tunnel_config(ctx, args),
        _ => return None,
    })
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ThemeInfo {
    pub name: String,
    pub kind: String,
    pub is_builtin: bool,
}

/// Get the full user settings from SQLite.
fn get_settings(state: &AppState) -> Result<Value, ApiError> {
    let db = state.db.lock_safe();
    let json = db.get_setting("user_settings").map_err(|e| e.to_string())?;
    let settings: UserSettings = match json {
        Some(j) => serde_json::from_str(&j)
            .map_err(|e| ApiError::from(format!("Invalid settings JSON: {}", e)))?,
        None => UserSettings::default(),
    };
    ok(settings)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SettingsArg {
    settings: UserSettings,
}

/// Update user settings (full replace).
fn update_settings(state: &AppState, args: &Value) -> Result<Value, ApiError> {
    let a: SettingsArg = parse(args)?;
    let db = state.db.lock_safe();
    let json = serde_json::to_string(&a.settings).map_err(|e| e.to_string())?;
    db.set_setting("user_settings", &json)
        .map_err(|e| e.to_string())?;
    ok(())
}

/// Get the active theme (resolved from settings).
fn get_active_theme(state: &AppState) -> Result<Value, ApiError> {
    let db = state.db.lock_safe();
    let settings: UserSettings = match db.get_setting("user_settings").map_err(|e| e.to_string())? {
        Some(j) => serde_json::from_str(&j).map_err(|e| e.to_string())?,
        None => UserSettings::default(),
    };

    if let Some(theme) = Theme::builtin(&settings.theme.active_theme) {
        return ok(theme);
    }

    let key = format!("theme:{}", settings.theme.active_theme);
    let theme: Theme = match db.get_setting(&key).map_err(|e| e.to_string())? {
        Some(j) => serde_json::from_str(&j).map_err(|e| e.to_string())?,
        None => Theme::obsidian(), // fallback
    };
    ok(theme)
}

/// List all available themes (built-in + custom).
fn list_themes(state: &AppState) -> Result<Value, ApiError> {
    let db = state.db.lock_safe();
    let mut themes: Vec<ThemeInfo> = Theme::builtin_names()
        .into_iter()
        .map(|name| ThemeInfo {
            name: name.to_string(),
            kind: if name.contains("Light") {
                "light".to_string()
            } else {
                "dark".to_string()
            },
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

    ok(themes)
}

/// Import a theme from a file path (TOML or JSON).
fn import_theme(state: &AppState, args: &Value) -> Result<Value, ApiError> {
    let a: PathArg = parse(args)?;
    let path = a.path;
    let content = std::fs::read_to_string(&path)
        .map_err(|e| ApiError::from(format!("Failed to read theme file: {}", e)))?;

    let theme = if path.ends_with(".toml") {
        Theme::from_toml(&content)?
    } else {
        Theme::from_json(&content)?
    };

    let db = state.db.lock_safe();
    let key = format!("theme:{}", theme.name);
    let json = serde_json::to_string(&theme).map_err(|e| e.to_string())?;
    db.set_setting(&key, &json).map_err(|e| e.to_string())?;

    let mut settings: UserSettings =
        match db.get_setting("user_settings").map_err(|e| e.to_string())? {
            Some(j) => serde_json::from_str(&j).map_err(|e| e.to_string())?,
            None => UserSettings::default(),
        };
    if !settings.theme.custom_themes.contains(&theme.name) {
        settings.theme.custom_themes.push(theme.name.clone());
        let settings_json = serde_json::to_string(&settings).map_err(|e| e.to_string())?;
        db.set_setting("user_settings", &settings_json)
            .map_err(|e| e.to_string())?;
    }

    ok(theme)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct JsonArg {
    json: String,
}

/// Import a theme directly from a JSON string (paste in UI).
fn import_theme_json(state: &AppState, args: &Value) -> Result<Value, ApiError> {
    let a: JsonArg = parse(args)?;
    let theme = Theme::from_json(&a.json)?;
    let db = state.db.lock_safe();
    let key = format!("theme:{}", theme.name);
    let serialized = serde_json::to_string(&theme).map_err(|e| e.to_string())?;
    db.set_setting(&key, &serialized)
        .map_err(|e| e.to_string())?;
    let mut settings: UserSettings =
        match db.get_setting("user_settings").map_err(|e| e.to_string())? {
            Some(j) => serde_json::from_str(&j).map_err(|e| e.to_string())?,
            None => UserSettings::default(),
        };
    if !settings.theme.custom_themes.contains(&theme.name) {
        settings.theme.custom_themes.push(theme.name.clone());
        let json = serde_json::to_string(&settings).map_err(|e| e.to_string())?;
        db.set_setting("user_settings", &json)
            .map_err(|e| e.to_string())?;
    }
    ok(theme)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NameArg {
    name: String,
}

/// Get a single theme by name (built-in or custom).
fn get_theme(state: &AppState, args: &Value) -> Result<Value, ApiError> {
    let a: NameArg = parse(args)?;
    let name = a.name;
    if let Some(theme) = Theme::builtin(&name) {
        return ok(theme);
    }
    let db = state.db.lock_safe();
    let key = format!("theme:{}", name);
    let theme: Theme = match db.get_setting(&key).map_err(|e| e.to_string())? {
        Some(j) => serde_json::from_str(&j).map_err(|e| e.to_string())?,
        None => return Err(ApiError::from(format!("Theme '{}' not found", name))),
    };
    ok(theme)
}

/// Delete a custom theme by name.
fn delete_theme(state: &AppState, args: &Value) -> Result<Value, ApiError> {
    let a: NameArg = parse(args)?;
    let name = a.name;
    let db = state.db.lock_safe();
    db.delete_setting(&format!("theme:{}", name))
        .map_err(|e| e.to_string())?;
    let mut settings: UserSettings =
        match db.get_setting("user_settings").map_err(|e| e.to_string())? {
            Some(j) => serde_json::from_str(&j).map_err(|e| e.to_string())?,
            None => UserSettings::default(),
        };
    settings.theme.custom_themes.retain(|n| n != &name);
    if settings.theme.active_theme == name {
        settings.theme.active_theme = "Obsidian".to_string();
    }
    let json = serde_json::to_string(&settings).map_err(|e| e.to_string())?;
    db.set_setting("user_settings", &json)
        .map_err(|e| e.to_string())?;
    ok(())
}

/// Import keybindings from a VS Code-compatible JSON file.
fn import_keybindings(state: &AppState, args: &Value) -> Result<Value, ApiError> {
    let a: PathArg = parse(args)?;
    let path = a.path;
    let content = std::fs::read_to_string(&path)
        .map_err(|e| ApiError::from(format!("Failed to read keybindings file: {}", e)))?;

    let keybinding_set = KeybindingSet::from_vscode_json(&content)?;

    let db = state.db.lock_safe();
    let mut settings: UserSettings =
        match db.get_setting("user_settings").map_err(|e| e.to_string())? {
            Some(j) => serde_json::from_str(&j).map_err(|e| e.to_string())?,
            None => UserSettings::default(),
        };
    settings.keybindings = keybinding_set.bindings.clone();
    let json = serde_json::to_string(&settings).map_err(|e| e.to_string())?;
    db.set_setting("user_settings", &json)
        .map_err(|e| e.to_string())?;

    ok(keybinding_set.bindings)
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

fn detect_vscode_keybindings() -> Result<Value, ApiError> {
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
    ok(VsCodeDetection {
        importable,
        detected_without_overrides,
    })
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

/// Return the live tunnel config (mode + preview/cookie domains) for the
/// Settings form and the frontend "open in my browser" URL builder.
fn get_tunnel_config(ctx: &ServerContext) -> Result<Value, ApiError> {
    let tc = ctx
        .tunnel
        .read()
        .map_err(|_| ApiError::from("tunnel config lock poisoned".to_string()))?
        .clone();
    ok(serde_json::json!({
        "mode": tc.mode,
        "previewDomain": tc.preview_domain,
        "cookieDomain": tc.cookie_domain,
        "autoExpose": tc.auto_expose,
    }))
}

/// Persist + live-apply a new tunnel config. Subdomain mode requires both a
/// preview domain and a cookie domain (the latter so the session cookie reaches
/// the preview subdomains).
fn set_tunnel_config(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct A {
        mode: String,
        preview_domain: Option<String>,
        cookie_domain: Option<String>,
        #[serde(default = "default_true")]
        auto_expose: bool,
    }
    let a: A = parse(args)?;

    let mode = match a.mode.as_str() {
        "subdomain" | "cloudflare" | "path" => a.mode,
        other => return Err(ApiError::bad(format!("unknown tunnel mode: {other}"))),
    };
    let preview_domain = a
        .preview_domain
        .map(|s| s.trim().trim_start_matches('.').to_string())
        .filter(|s| !s.is_empty());
    let cookie_domain = a
        .cookie_domain
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    if mode == "subdomain" {
        if preview_domain.is_none() {
            return Err(ApiError::bad(
                "subdomain mode needs a preview domain".to_string(),
            ));
        }
        if cookie_domain.is_none() {
            return Err(ApiError::bad(
                "subdomain mode needs a cookie domain (e.g. .example.com)".to_string(),
            ));
        }
    }

    let tc = TunnelConfig {
        mode,
        preview_domain,
        cookie_domain,
        auto_expose: a.auto_expose,
    };

    let json = serde_json::to_string(&tc).map_err(|e| e.to_string())?;
    ctx.state()
        .db
        .lock_safe()
        .set_setting("tunnel_config", &json)
        .map_err(|e| e.to_string())?;

    *ctx.tunnel
        .write()
        .map_err(|_| ApiError::from("tunnel config lock poisoned".to_string()))? = tc.clone();

    ok(serde_json::json!({
        "mode": tc.mode,
        "previewDomain": tc.preview_domain,
        "cookieDomain": tc.cookie_domain,
        "autoExpose": tc.auto_expose,
    }))
}

fn default_true() -> bool {
    true
}

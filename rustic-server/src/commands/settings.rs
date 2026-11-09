//! settings commands — server dispatch.
//!
//! Legacy settings/themes/keybindings: `rustic_app::host_ops::settings`
//! (shared with desktop). Theme packs: `rustic_app::themes`. The tunnel
//! config commands are web-only and live here.

use serde::Deserialize;
use serde_json::Value;

use rustic_app::context::AppContext;
use rustic_app::host_ops::settings as ops;
use rustic_app::sync_ext::MutexExt;
use rustic_core::config::UserSettings;

use super::rules::handle;
use crate::api::{ok, parse, ApiError, PathArg};
use crate::context::{ServerContext, TunnelConfig};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SettingsArg {
    settings: UserSettings,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct JsonArg {
    json: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NameArg {
    name: String,
}

pub async fn dispatch(
    ctx: &ServerContext,
    command: &str,
    args: &Value,
) -> Option<Result<Value, ApiError>> {
    let state = ctx.state();
    Some(match command {
        "get_settings" => handle(args, |_: Value| ops::get_settings(state)),
        "update_settings" => handle(args, |a: SettingsArg| {
            ops::update_settings(state, &a.settings)
        }),
        "get_active_theme" => handle(args, |_: Value| ops::get_active_theme(state)),
        "list_themes" => handle(args, |_: Value| ops::list_themes(state)),
        "import_theme" => handle(args, |a: PathArg| ops::import_theme(state, &a.path)),
        "import_theme_json" => handle(args, |a: JsonArg| ops::import_theme_json(state, &a.json)),
        "get_theme" => handle(args, |a: NameArg| ops::get_theme(state, &a.name)),
        "delete_theme" => handle(args, |a: NameArg| ops::delete_theme(state, &a.name)),
        "import_keybindings" => handle(args, |a: PathArg| ops::import_keybindings(state, &a.path)),
        "detect_vscode_keybindings" => ok(ops::detect_vscode_keybindings()),
        "get_tunnel_config" => get_tunnel_config(ctx),
        "set_tunnel_config" => set_tunnel_config(ctx, args),
        c if c.starts_with("theme_") => theme_command(ctx, c, args),
        _ => return None,
    })
}

#[derive(Deserialize)]
struct ThemeIdArg {
    id: String,
}

#[derive(Deserialize)]
struct ThemeTextArg {
    #[serde(default)]
    id: Option<String>,
    text: String,
}

#[derive(Deserialize)]
struct ThemeTrustArg {
    id: String,
    hash: String,
}

#[derive(Deserialize)]
struct ThemeModeArg {
    mode: String,
}

/// Theme packs (format 2) — same commands as the desktop, against the
/// server's data dir. Import / export go through text (browser uploads and
/// downloads) or a server-side path.
fn theme_command(ctx: &ServerContext, command: &str, args: &Value) -> Result<Value, ApiError> {
    use rustic_app::themes as t;
    let (state, dir) = (ctx.state(), ctx.data_dir.as_path());
    match command {
        "theme_list" => ok(t::list(state, dir)),
        "theme_get" => {
            let a: ThemeIdArg = parse(args)?;
            ok(t::get(state, dir, &a.id)?)
        }
        "theme_import_file" => {
            let a: PathArg = parse(args)?;
            ok(t::import_path(state, dir, &a.path)?)
        }
        "theme_import_text" => {
            let a: ThemeTextArg = parse(args)?;
            ok(t::import_text(state, dir, &a.text)?)
        }
        "theme_write" => {
            let a: ThemeTextArg = parse(args)?;
            let id =
                a.id.ok_or_else(|| ApiError::from("id is required".to_string()))?;
            ok(t::write(state, dir, &id, &a.text)?)
        }
        "theme_delete" => {
            let a: ThemeIdArg = parse(args)?;
            ok(t::delete(state, dir, &a.id)?)
        }
        "theme_trust" => {
            let a: ThemeTrustArg = parse(args)?;
            ok(t::trust(state, dir, &a.id, &a.hash)?)
        }
        "theme_apply" => {
            let a: ThemeIdArg = parse(args)?;
            ok(t::apply(state, dir, &a.id)?)
        }
        "theme_set_mode" => {
            let a: ThemeModeArg = parse(args)?;
            ok(t::set_mode(state, &a.mode)?)
        }
        "theme_active" => ok(t::active(state, dir)),
        "theme_template" => ok(t::template()),
        "theme_folder" => {
            let d = t::dir(dir);
            std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
            ok(d.to_string_lossy().into_owned())
        }
        other => Err(ApiError::from(format!(
            "{other} isn't available on the server"
        ))),
    }
}

fn tunnel_json(tc: &TunnelConfig) -> Value {
    serde_json::json!({
        "mode": tc.mode,
        "previewDomain": tc.preview_domain,
        "cookieDomain": tc.cookie_domain,
        "autoExpose": tc.auto_expose,
    })
}

/// Return the live tunnel config (mode + preview/cookie domains) for the
/// Settings form and the frontend "open in my browser" URL builder.
fn get_tunnel_config(ctx: &ServerContext) -> Result<Value, ApiError> {
    let tc = ctx
        .tunnel
        .read()
        .map_err(|_| ApiError::from("tunnel config lock poisoned".to_string()))?
        .clone();
    ok(tunnel_json(&tc))
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

    ok(tunnel_json(&tc))
}

fn default_true() -> bool {
    true
}

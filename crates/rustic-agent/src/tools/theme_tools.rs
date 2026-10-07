//! Agent theme tools: `list_themes`, `read_theme`, `write_theme`,
//! `delete_theme`. Themes are machine-wide files owned by the app layer
//! (`rustic_app::themes`), which this crate can't depend on — so the app
//! registers a [`ThemeHost`] at startup and these tools only validate
//! arguments and format results.
//!
//! Safety model (user decision): the agent can create and edit theme files,
//! but nothing it writes ever applies by itself — every saved theme is
//! untrusted until the user reviews and trusts it in the app (the host shows
//! that prompt). Deleting a theme always asks the user first.

use std::sync::{Arc, OnceLock};

use crate::provider::ToolDef;
use crate::task::permissions::PermissionLevel;
use crate::task::PermissionOp;
use crate::tools::{ToolContext, ToolOutput};
use anyhow::Result;
use serde_json::{json, Value};

/// App-side theme store the tools talk to.
pub trait ThemeHost: Send + Sync + 'static {
    /// `{ active, mode, themes: [entry…] }`.
    fn list(&self) -> Result<Value, String>;
    /// `{ entry, text, scan }` for theme `id`.
    fn read(&self, id: &str) -> Result<Value, String>;
    /// The starter template file text.
    fn template(&self) -> String;
    /// Create (`id` = None) or replace an installed theme; saved untrusted and
    /// the user is prompted to review it. Returns `{ entry, scan }`.
    fn write(&self, id: Option<&str>, text: &str) -> Result<Value, String>;
    /// Delete installed theme `id`.
    fn delete(&self, id: &str) -> Result<(), String>;
}

static HOST: OnceLock<Arc<dyn ThemeHost>> = OnceLock::new();

/// Register the app's theme store (first call wins).
pub fn set_theme_host(host: Arc<dyn ThemeHost>) {
    let _ = HOST.set(host);
}

fn host() -> Option<&'static Arc<dyn ThemeHost>> {
    HOST.get()
}

const NAMES: [&str; 4] = ["list_themes", "read_theme", "write_theme", "delete_theme"];

/// Whether `name` is one of these tools.
pub fn handles(name: &str) -> bool {
    NAMES.contains(&name)
}

/// Tool definitions (deferred behind tool_search).
pub fn definitions() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "list_themes".into(),
            description: "List Rustic's UI themes (built-in and installed) with the active theme and \
                 color mode. Each entry has id, name, builtin, modes (dark/light), trusted, has_css \
                 and its style recipes. Read-only. Load the `rustic-themes` skill before designing \
                 or editing a theme."
                .into(),
            parameters: json!({ "type": "object", "properties": {} }),
        },
        ToolDef {
            name: "read_theme".into(),
            description: "Read a theme file (`*.rustic-theme.json`) by id, plus its safety scan \
                 (blocked patterns / warnings / remote hosts). Built-ins are readable as references. \
                 Pass id \"template\" for a complete starter file with every key filled in. Read-only."
                .into(),
            parameters: json!({
                "type": "object",
                "required": ["id"],
                "properties": {
                    "id": { "type": "string", "description": "Theme id from list_themes, or \"template\"." }
                }
            }),
        },
        ToolDef {
            name: "write_theme".into(),
            description: "Create a new theme, or replace an installed (non built-in) theme's file. \
                 `content` is the full theme JSON (format 2). It is validated and safety-scanned; \
                 the saved theme is NOT applied — the user is prompted to review and trust it in \
                 Settings › Appearance › Themes. Built-in themes are read-only: omit `id` to save an \
                 edited copy as a new theme."
                .into(),
            parameters: json!({
                "type": "object",
                "required": ["content"],
                "properties": {
                    "id": { "type": "string", "description": "Installed theme to replace. Omit to create a new theme (id derived from its name)." },
                    "content": { "type": "string", "description": "Full theme file JSON." }
                }
            }),
        },
        ToolDef {
            name: "delete_theme".into(),
            description: "Delete an installed (non built-in) theme file. Always asks the user for \
                 approval. If it was active, Rustic falls back to the default theme."
                .into(),
            parameters: json!({
                "type": "object",
                "required": ["id"],
                "properties": {
                    "id": { "type": "string", "description": "Installed theme id from list_themes." }
                }
            }),
        },
    ]
}

/// Route a theme-tool call.
pub async fn execute(name: &str, params: Value, context: &ToolContext) -> Result<ToolOutput> {
    let Some(host) = host() else {
        return Ok(ToolOutput::text(
            "THEMES_UNAVAILABLE: theme management isn't available in this host.",
            true,
        ));
    };
    let id = params["id"].as_str().map(str::trim).filter(|s| !s.is_empty());
    match name {
        "list_themes" => Ok(json_result(host.list())),
        "read_theme" => {
            let Some(id) = id else { return Ok(missing("id")) };
            if id == "template" {
                return Ok(ToolOutput::text(host.template(), false));
            }
            Ok(match host.read(id) {
                Ok(v) => ToolOutput::text(format_read(&v), false),
                Err(e) => ToolOutput::text(format!("THEME_ERROR: {e}"), true),
            })
        }
        "write_theme" | "delete_theme" => {
            if let Some(denied) = write_gate(context) {
                return Ok(denied);
            }
            if name == "write_theme" {
                let Some(content) = params["content"].as_str().filter(|s| !s.trim().is_empty()) else {
                    return Ok(missing("content"));
                };
                return Ok(match host.write(id, content) {
                    Ok(v) => ToolOutput::text(format_write(&v), false),
                    Err(e) => ToolOutput::text(format!("THEME_INVALID: {e}"), true),
                });
            }
            let Some(id) = id else { return Ok(missing("id")) };
            let approved = context
                .permission_broker
                .request(
                    &context.event_tx,
                    &context.task_id,
                    PermissionOp::ExtensionChange {
                        action: "delete".into(),
                        kind: "theme".into(),
                        name: id.to_string(),
                        preview: format!("Delete the theme file `{id}`. This can't be undone."),
                    },
                )
                .await;
            if !approved {
                return Ok(ToolOutput::text(
                    format!("PERMISSION_DENIED: the user declined deleting theme `{id}`. Don't retry without asking."),
                    true,
                ));
            }
            Ok(match host.delete(id) {
                Ok(()) => ToolOutput::text(format!("Deleted theme `{id}`."), false),
                Err(e) => ToolOutput::text(format!("THEME_ERROR: {e}"), true),
            })
        }
        _ => Ok(ToolOutput::text(format!("Unknown theme tool: {name}"), true)),
    }
}

/// Reject writes in plan mode and Chat mode.
fn write_gate(context: &ToolContext) -> Option<ToolOutput> {
    if context.is_plan_mode {
        return Some(ToolOutput::text(
            "PLAN_MODE: theme changes are write operations and are disabled in plan mode.",
            true,
        ));
    }
    if matches!(context.permissions(), PermissionLevel::Chat) {
        return Some(ToolOutput::text(
            "PERMISSION_DENIED: Chat mode is read-only; theme changes are not allowed.",
            true,
        ));
    }
    None
}

fn missing(field: &str) -> ToolOutput {
    ToolOutput::text(format!("INVALID_PARAMS: `{field}` is required"), true)
}

fn json_result(r: Result<Value, String>) -> ToolOutput {
    match r {
        Ok(v) => ToolOutput::text(serde_json::to_string_pretty(&v).unwrap_or_default(), false),
        Err(e) => ToolOutput::text(format!("THEME_ERROR: {e}"), true),
    }
}

/// Scan findings as bullet lines.
fn format_scan(scan: &Value) -> String {
    let mut out = String::new();
    for (key, label) in [("blocked", "BLOCKED"), ("warnings", "warning")] {
        for f in scan[key].as_array().into_iter().flatten() {
            out.push_str(&format!("- {label}: {}\n", f["message"].as_str().unwrap_or("")));
        }
    }
    if let Some(hosts) = scan["remote_hosts"].as_array().filter(|h| !h.is_empty()) {
        let hosts: Vec<&str> = hosts.iter().filter_map(|h| h.as_str()).collect();
        out.push_str(&format!("- connects to: {}\n", hosts.join(", ")));
    }
    if out.is_empty() {
        out.push_str("- no findings\n");
    }
    out
}

fn format_read(v: &Value) -> String {
    let e = &v["entry"];
    format!(
        "Theme `{}` ({}){}\nSafety scan:\n{}\nFile:\n{}",
        e["id"].as_str().unwrap_or(""),
        e["name"].as_str().unwrap_or(""),
        if e["builtin"].as_bool() == Some(true) { " — built-in, read-only" } else { "" },
        format_scan(&v["scan"]),
        v["text"].as_str().unwrap_or("")
    )
}

fn format_write(v: &Value) -> String {
    let e = &v["entry"];
    let blocked = v["scan"]["blocked"].as_array().is_some_and(|b| !b.is_empty());
    format!(
        "Saved theme `{}` ({}). It is NOT applied: {}\nSafety scan:\n{}",
        e["id"].as_str().unwrap_or(""),
        e["name"].as_str().unwrap_or(""),
        if blocked {
            "the scan BLOCKED it, so the user can't trust it — fix the blocked items and write it again."
        } else {
            "the user has been prompted to review and trust it (Settings › Appearance › Themes). Tell them it's ready to review."
        },
        format_scan(&v["scan"])
    )
}

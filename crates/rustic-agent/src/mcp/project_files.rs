//! Per-project MCP files Rustic keeps in sync so other agent CLIs see the same
//! servers: `.mcp.json` (Claude Code / Rustic), `.gemini/settings.json`
//! (Gemini CLI) and `.codex/config.toml` (Codex). Rustic-only state — the
//! per-project disabled list — lives in `.rustic/mcp.json`.

use anyhow::{anyhow, Result};
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Paths added to the project's `.gitignore` because they can hold API keys.
pub const GITIGNORE_ENTRIES: [&str; 4] = [".mcp.json", ".gemini/", ".codex/", ".rustic/"];

/// Path of the project's Claude-Code-format `.mcp.json`.
pub fn mcp_json_path(root: &Path) -> PathBuf {
    root.join(".mcp.json")
}

/// Path of the project's Gemini CLI settings file.
fn gemini_path(root: &Path) -> PathBuf {
    root.join(".gemini").join("settings.json")
}

/// Path of the project's Codex config file.
fn codex_path(root: &Path) -> PathBuf {
    root.join(".codex").join("config.toml")
}

/// Path of Rustic's per-project MCP state file.
fn rustic_state_path(root: &Path) -> PathBuf {
    root.join(".rustic").join("mcp.json")
}

/// Check that a server entry is a JSON object with a `command` or `url`.
pub fn validate_entry(name: &str, entry: &Value) -> Result<()> {
    if name.trim().is_empty() {
        return Err(anyhow!("Server name must not be empty"));
    }
    let obj = entry
        .as_object()
        .ok_or_else(|| anyhow!("Server \"{}\" must be a JSON object", name))?;
    let has_command = obj.get("command").and_then(|v| v.as_str()).is_some();
    let has_url = ["url", "serverUrl", "httpUrl"]
        .iter()
        .any(|k| obj.get(*k).and_then(|v| v.as_str()).is_some());
    if !has_command && !has_url {
        return Err(anyhow!(
            "Server \"{}\" needs either a \"command\" (stdio) or a \"url\" (http/sse)",
            name
        ));
    }
    Ok(())
}

/// Read a JSON file as an object; a missing file yields an empty object.
fn read_json_object(path: &Path) -> Result<Value> {
    if !path.exists() {
        return Ok(json!({}));
    }
    let text = std::fs::read_to_string(path)?;
    if text.trim().is_empty() {
        return Ok(json!({}));
    }
    let v: Value = serde_json::from_str(&text)
        .map_err(|e| anyhow!("{} is not valid JSON: {}", path.display(), e))?;
    if !v.is_object() {
        return Err(anyhow!("{} must contain a JSON object", path.display()));
    }
    Ok(v)
}

/// Pretty-print and atomically write a JSON value.
fn write_json(path: &Path, value: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut text = serde_json::to_string_pretty(value)?;
    text.push('\n');
    crate::io_util::atomic_write(path, text.as_bytes())?;
    Ok(())
}

/// Mutable access to `obj[key]` as an object, creating it when absent.
fn object_entry<'a>(obj: &'a mut Value, key: &str) -> Result<&'a mut Map<String, Value>> {
    let map = obj
        .as_object_mut()
        .ok_or_else(|| anyhow!("expected a JSON object"))?;
    let slot = map.entry(key.to_string()).or_insert_with(|| json!({}));
    if !slot.is_object() {
        *slot = json!({});
    }
    Ok(slot.as_object_mut().expect("just ensured object"))
}

/// Read one server entry from the project's `.mcp.json`, if present.
pub fn read_project_entry(root: &Path, name: &str) -> Result<Option<Value>> {
    let doc = read_json_object(&mcp_json_path(root))?;
    Ok(doc.get("mcpServers").and_then(|s| s.get(name)).cloned())
}

/// Write `entry` for `name` into `.mcp.json`, `.gemini/settings.json` and
/// `.codex/config.toml`, then make sure those files are git-ignored.
pub fn upsert_server(root: &Path, name: &str, entry: &Value) -> Result<()> {
    validate_entry(name, entry)?;

    let path = mcp_json_path(root);
    let mut doc = read_json_object(&path)?;
    object_entry(&mut doc, "mcpServers")?.insert(name.to_string(), entry.clone());
    write_json(&path, &doc)?;

    let path = gemini_path(root);
    let mut doc = read_json_object(&path)?;
    object_entry(&mut doc, "mcpServers")?.insert(name.to_string(), to_gemini_entry(entry));
    write_json(&path, &doc)?;

    codex_upsert(&codex_path(root), name, entry)?;
    ensure_gitignore(root)?;
    Ok(())
}

/// Remove `name` from every synced project file that exists. Files that
/// don't mention the server are left untouched.
pub fn remove_server(root: &Path, name: &str) -> Result<()> {
    for path in [mcp_json_path(root), gemini_path(root)] {
        if !path.exists() {
            continue;
        }
        let mut doc = read_json_object(&path)?;
        let removed = doc
            .get_mut("mcpServers")
            .and_then(|s| s.as_object_mut())
            .map(|s| s.remove(name).is_some())
            .unwrap_or(false);
        if removed {
            write_json(&path, &doc)?;
        }
    }
    codex_remove(&codex_path(root), name)?;
    Ok(())
}

/// Server names disabled for this project in `.rustic/mcp.json`.
pub fn read_disabled(root: &Path) -> BTreeSet<String> {
    read_json_object(&rustic_state_path(root))
        .ok()
        .and_then(|doc| doc.get("disabled").and_then(|v| v.as_array()).cloned())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Add or remove `name` from the project's disabled list.
pub fn set_disabled(root: &Path, name: &str, disabled: bool) -> Result<()> {
    let path = rustic_state_path(root);
    let mut current = read_disabled(root);
    let changed = if disabled {
        current.insert(name.to_string())
    } else {
        current.remove(name)
    };
    if !changed {
        return Ok(());
    }
    let mut doc = read_json_object(&path).unwrap_or_else(|_| json!({}));
    doc.as_object_mut()
        .expect("read_json_object returns objects")
        .insert(
            "disabled".into(),
            Value::Array(current.into_iter().map(Value::String).collect()),
        );
    write_json(&path, &doc)?;
    ensure_gitignore(root)
}

/// Append any missing `GITIGNORE_ENTRIES` to the project's `.gitignore`.
/// Only touches git repositories (a `.git` dir/file or an existing `.gitignore`).
pub fn ensure_gitignore(root: &Path) -> Result<()> {
    let path = root.join(".gitignore");
    if !path.exists() && !root.join(".git").exists() {
        return Ok(());
    }
    let existing = if path.exists() {
        std::fs::read_to_string(&path)?
    } else {
        String::new()
    };
    let present: BTreeSet<String> = existing
        .lines()
        .map(|l| {
            l.trim()
                .trim_start_matches('/')
                .trim_end_matches('/')
                .to_string()
        })
        .collect();
    let missing: Vec<&str> = GITIGNORE_ENTRIES
        .iter()
        .copied()
        .filter(|e| !present.contains(e.trim_end_matches('/')))
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    let mut out = existing;
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str("# Agent config written by Rustic (may contain API keys)\n");
    for e in missing {
        out.push_str(e);
        out.push('\n');
    }
    crate::io_util::atomic_write(&path, out.as_bytes())?;
    Ok(())
}

/// Convert a Claude-Code-format entry to Gemini CLI's shape. Gemini uses
/// `url` for SSE and `httpUrl` for Streamable HTTP.
fn to_gemini_entry(entry: &Value) -> Value {
    let mut out = Map::new();
    if let Some(url) = entry.get("url").and_then(|v| v.as_str()) {
        let is_sse = entry.get("type").and_then(|v| v.as_str()) == Some("sse");
        out.insert(
            if is_sse { "url" } else { "httpUrl" }.into(),
            Value::String(url.to_string()),
        );
        if let Some(h) = entry.get("headers").filter(|h| h.is_object()) {
            out.insert("headers".into(), h.clone());
        }
    } else {
        for key in ["command", "args", "env", "cwd"] {
            if let Some(v) = entry.get(key) {
                out.insert(key.into(), v.clone());
            }
        }
    }
    Value::Object(out)
}

/// String-valued object → TOML inline table.
fn toml_inline_table(obj: &Map<String, Value>) -> toml_edit::InlineTable {
    let mut t = toml_edit::InlineTable::new();
    for (k, v) in obj {
        if let Some(s) = v.as_str() {
            t.insert(k.as_str(), toml_edit::Value::from(s));
        }
    }
    t
}

/// Parse a Codex config file, or start an empty document if missing.
fn read_toml(path: &Path) -> Result<toml_edit::DocumentMut> {
    if !path.exists() {
        return Ok(toml_edit::DocumentMut::new());
    }
    let text = std::fs::read_to_string(path)?;
    text.parse::<toml_edit::DocumentMut>()
        .map_err(|e| anyhow!("{} is not valid TOML: {}", path.display(), e))
}

/// Write `[mcp_servers.<name>]` into Codex's `config.toml`, keeping everything else.
fn codex_upsert(path: &Path, name: &str, entry: &Value) -> Result<()> {
    let mut doc = read_toml(path)?;
    if !doc.contains_key("mcp_servers") {
        let mut t = toml_edit::Table::new();
        t.set_implicit(true);
        doc.insert("mcp_servers", toml_edit::Item::Table(t));
    }
    let servers = doc["mcp_servers"]
        .as_table_mut()
        .ok_or_else(|| anyhow!("{}: `mcp_servers` is not a table", path.display()))?;

    let mut t = toml_edit::Table::new();
    if let Some(url) = entry.get("url").and_then(|v| v.as_str()) {
        t.insert("url", toml_edit::value(url));
        if let Some(h) = entry.get("headers").and_then(|v| v.as_object()) {
            if !h.is_empty() {
                t.insert("http_headers", toml_edit::value(toml_inline_table(h)));
            }
        }
    } else if let Some(cmd) = entry.get("command").and_then(|v| v.as_str()) {
        t.insert("command", toml_edit::value(cmd));
        let mut args = toml_edit::Array::new();
        for a in entry
            .get("args")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str())
        {
            args.push(a);
        }
        t.insert("args", toml_edit::value(args));
        if let Some(env) = entry.get("env").and_then(|v| v.as_object()) {
            if !env.is_empty() {
                t.insert("env", toml_edit::value(toml_inline_table(env)));
            }
        }
        if let Some(cwd) = entry.get("cwd").and_then(|v| v.as_str()) {
            t.insert("cwd", toml_edit::value(cwd));
        }
    }
    servers.insert(name, toml_edit::Item::Table(t));

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    crate::io_util::atomic_write(path, doc.to_string().as_bytes())?;
    Ok(())
}

/// Remove `[mcp_servers.<name>]` from Codex's `config.toml` if present.
fn codex_remove(path: &Path, name: &str) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let mut doc = read_toml(path)?;
    let removed = doc
        .get_mut("mcp_servers")
        .and_then(|i| i.as_table_mut())
        .map(|t| t.remove(name).is_some())
        .unwrap_or(false);
    if removed {
        crate::io_util::atomic_write(path, doc.to_string().as_bytes())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fresh scratch project dir with a `.git` marker so gitignore is touched.
    fn scratch() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rustic-mcp-pf-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        dir
    }

    #[test]
    fn upsert_writes_all_three_formats_and_gitignore() {
        let root = scratch();
        std::fs::create_dir_all(root.join(".codex")).unwrap();
        std::fs::write(codex_path(&root), "model = \"o3\"\n").unwrap();

        let stdio = json!({"command": "npx", "args": ["-y", "srv"], "env": {"API_KEY": "k1"}});
        upsert_server(&root, "stdio-srv", &stdio).unwrap();
        let remote = json!({"type": "http", "url": "https://x/mcp", "headers": {"Authorization": "Bearer t"}});
        upsert_server(&root, "remote", &remote).unwrap();

        assert_eq!(read_project_entry(&root, "stdio-srv").unwrap(), Some(stdio));
        let gemini = read_json_object(&gemini_path(&root)).unwrap();
        assert_eq!(gemini["mcpServers"]["remote"]["httpUrl"], "https://x/mcp");
        assert_eq!(gemini["mcpServers"]["stdio-srv"]["env"]["API_KEY"], "k1");

        let codex = std::fs::read_to_string(codex_path(&root)).unwrap();
        assert!(
            codex.contains("model = \"o3\""),
            "existing Codex settings kept: {codex}"
        );
        let doc = codex.parse::<toml_edit::DocumentMut>().unwrap();
        assert_eq!(
            doc["mcp_servers"]["stdio-srv"]["command"].as_str(),
            Some("npx")
        );
        assert_eq!(
            doc["mcp_servers"]["remote"]["url"].as_str(),
            Some("https://x/mcp")
        );

        let gi = std::fs::read_to_string(root.join(".gitignore")).unwrap();
        for e in GITIGNORE_ENTRIES {
            assert!(gi.lines().any(|l| l == e), "missing {e} in {gi}");
        }
        ensure_gitignore(&root).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join(".gitignore")).unwrap(),
            gi,
            "idempotent"
        );

        remove_server(&root, "stdio-srv").unwrap();
        assert_eq!(read_project_entry(&root, "stdio-srv").unwrap(), None);
        let codex = std::fs::read_to_string(codex_path(&root)).unwrap();
        assert!(!codex.contains("stdio-srv"));
        assert!(codex.contains("remote"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn disabled_list_round_trips() {
        let root = scratch();
        assert!(read_disabled(&root).is_empty());
        set_disabled(&root, "a", true).unwrap();
        set_disabled(&root, "b", true).unwrap();
        set_disabled(&root, "a", false).unwrap();
        assert_eq!(
            read_disabled(&root).into_iter().collect::<Vec<_>>(),
            vec!["b"]
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn gitignore_skipped_outside_git_repos() {
        let root = std::env::temp_dir().join(format!("rustic-mcp-pf-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        ensure_gitignore(&root).unwrap();
        assert!(!root.join(".gitignore").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn validate_rejects_entries_without_command_or_url() {
        assert!(validate_entry("x", &json!({"args": []})).is_err());
        assert!(validate_entry("", &json!({"command": "a"})).is_err());
        assert!(validate_entry("x", &json!({"url": "https://a"})).is_ok());
    }
}

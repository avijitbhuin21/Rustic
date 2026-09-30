//! Metadata-only sync (issue #15): providers + API keys, model capability
//! overrides, global rules / skills / workflows and the MCP server pool —
//! no projects. Merge semantics: items only on the receiving side are kept;
//! new items are added; items with the same name but different content are
//! only replaced when the user ticked them (`overwrite`).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::secrets::{provider_account, SecretStore};
use crate::state::AppState;
use crate::sync_ext::MutexExt;

/// One syncable metadata item. `key()` = `category/name` identifies it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MetaItem {
    pub category: String,
    pub name: String,
    pub payload: Value,
}

impl MetaItem {
    /// Stable identity used for diffing and overwrite selection.
    pub fn key(&self) -> String {
        format!("{}/{}", self.category, self.name)
    }
}

/// Every metadata item on one side.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MetaBundle {
    pub items: Vec<MetaItem>,
}

/// How an incoming item relates to the local one.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MetaStatus {
    /// Only on the incoming side — will be added.
    New,
    /// Same name, different content — replaced only if the user ticks it.
    Conflict,
    /// Identical on both sides — nothing to do.
    Same,
    /// Only on the receiving side — always kept.
    LocalOnly,
}

/// One row of the review list shown before applying.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetaDiffEntry {
    pub key: String,
    pub category: String,
    pub name: String,
    pub status: MetaStatus,
}

/// Result of applying a bundle.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MetaApplySummary {
    pub added: usize,
    pub replaced: usize,
    pub kept: usize,
    pub errors: Vec<String>,
}

/// Compare `incoming` against `local` (sorted by category, then name).
pub fn diff_bundles(incoming: &MetaBundle, local: &MetaBundle) -> Vec<MetaDiffEntry> {
    let local_by_key: HashMap<String, &MetaItem> =
        local.items.iter().map(|i| (i.key(), i)).collect();
    let incoming_keys: HashSet<String> = incoming.items.iter().map(|i| i.key()).collect();
    let mut out: Vec<MetaDiffEntry> = incoming
        .items
        .iter()
        .map(|i| {
            let status = match local_by_key.get(&i.key()) {
                None => MetaStatus::New,
                Some(l) if l.payload == i.payload => MetaStatus::Same,
                Some(_) => MetaStatus::Conflict,
            };
            MetaDiffEntry {
                key: i.key(),
                category: i.category.clone(),
                name: i.name.clone(),
                status,
            }
        })
        .collect();
    out.extend(
        local
            .items
            .iter()
            .filter(|i| !incoming_keys.contains(&i.key()))
            .map(|i| MetaDiffEntry {
                key: i.key(),
                category: i.category.clone(),
                name: i.name.clone(),
                status: MetaStatus::LocalOnly,
            }),
    );
    out.sort_by(|a, b| {
        (a.category.as_str(), a.name.as_str()).cmp(&(b.category.as_str(), b.name.as_str()))
    });
    out
}

/// Items of `incoming` to write: new ones, plus conflicts the user ticked.
pub fn items_to_write<'a>(
    incoming: &'a MetaBundle,
    local: &MetaBundle,
    overwrite: &HashSet<String>,
) -> (Vec<&'a MetaItem>, usize) {
    let local_by_key: HashMap<String, &MetaItem> =
        local.items.iter().map(|i| (i.key(), i)).collect();
    let mut write = Vec::new();
    let mut kept = 0;
    for item in &incoming.items {
        match local_by_key.get(&item.key()) {
            None => write.push(item),
            Some(l) if l.payload == item.payload => {}
            Some(_) if overwrite.contains(&item.key()) => write.push(item),
            Some(_) => kept += 1,
        }
    }
    (write, kept)
}

/// Read the `ai_config` setting as JSON (empty object when unset).
fn read_ai_config(state: &AppState) -> Value {
    let db = state.db.lock_safe();
    db.get_setting("ai_config")
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| json!({}))
}

/// Provider identity: `<provider_type>:<name>` (name may be empty).
fn provider_name(entry: &Value) -> Option<(String, Option<String>)> {
    let pt = entry.get("provider_type")?.as_str()?.to_string();
    let name = entry
        .get("name")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string);
    Some((pt, name))
}

/// Read every regular file under `dir` as `{relative/path: base64}`.
fn read_tree_b64(dir: &Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if let (Ok(bytes), Ok(rel)) = (std::fs::read(&p), p.strip_prefix(dir)) {
                out.insert(
                    rel.to_string_lossy().replace('\\', "/"),
                    base64::engine::general_purpose::STANDARD.encode(bytes),
                );
            }
        }
    }
    out
}

/// Collect this side's metadata: providers (with keys), model capability
/// overrides, global rules / workflows / skills and the MCP pool.
pub fn export_bundle(state: &AppState, data_dir: &Path, secrets: &dyn SecretStore) -> MetaBundle {
    let mut items = Vec::new();
    let cfg = read_ai_config(state);

    for entry in cfg
        .get("providers")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
    {
        let Some((pt, name)) = provider_name(&entry) else {
            continue;
        };
        let mut payload = entry.clone();
        let stored = entry.get("api_key").and_then(|v| v.as_str()).unwrap_or("");
        let key = if stored.is_empty() {
            secrets
                .get(&provider_account(&pt, name.as_deref()))
                .ok()
                .flatten()
                .unwrap_or_default()
        } else {
            stored.to_string()
        };
        payload["api_key"] = Value::String(key);
        items.push(MetaItem {
            category: "provider".into(),
            name: format!("{}:{}", pt, name.unwrap_or_default()),
            payload,
        });
    }
    if let Some(caps) = cfg.get("model_capabilities").and_then(|v| v.as_object()) {
        for (model, caps) in caps {
            items.push(MetaItem {
                category: "model".into(),
                name: model.clone(),
                payload: caps.clone(),
            });
        }
    }
    for rule in rustic_agent::rules::discover_global_rules() {
        if let Ok(content) = std::fs::read_to_string(&rule.path) {
            let file = rule
                .path
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_default();
            items.push(MetaItem {
                category: "rule".into(),
                name: rule.name,
                payload: json!({ "file": file, "content": content }),
            });
        }
    }
    if let Some(dir) = rustic_agent::workflows::global_workflows_dir() {
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("md") {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(&p) else {
                continue;
            };
            let file = p
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_default();
            let name = rustic_agent::workflows::parse_workflow_frontmatter(&content)
                .map(|(n, _)| n)
                .unwrap_or_else(|| file.trim_end_matches(".md").to_string());
            items.push(MetaItem {
                category: "workflow".into(),
                name,
                payload: json!({ "file": file, "content": content }),
            });
        }
    }
    if let Some(dir) = rustic_agent::skills::global_skills_dir() {
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() && p.join("SKILL.md").is_file() {
                let name = e.file_name().to_string_lossy().to_string();
                items.push(MetaItem {
                    category: "skill".into(),
                    name,
                    payload: json!({ "files": read_tree_b64(&p) }),
                });
            }
        }
    }
    let pool: Value = std::fs::read_to_string(data_dir.join("mcp.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| json!({}));
    if let Some(servers) = pool.get("mcpServers").and_then(|v| v.as_object()) {
        for (name, entry) in servers {
            items.push(MetaItem {
                category: "mcp_server".into(),
                name: name.clone(),
                payload: entry.clone(),
            });
        }
    }
    MetaBundle { items }
}

/// Move an existing file/dir to the extensions trash before replacing it.
fn trash_existing(path: &Path) {
    if path.exists() {
        if let Err(e) = rustic_agent::extensions::move_to_trash(path) {
            tracing::warn!(path = %path.display(), "meta sync: could not back up before overwrite: {e}");
        }
    }
}

/// Write one item into the local environment. `cfg` / `pool` are edited in
/// memory and saved by the caller.
fn write_item(
    item: &MetaItem,
    cfg: &mut Value,
    pool: &mut Value,
    secrets: &dyn SecretStore,
) -> Result<(), String> {
    match item.category.as_str() {
        "provider" => {
            let (pt, name) =
                provider_name(&item.payload).ok_or("provider entry without provider_type")?;
            let key = item
                .payload
                .get("api_key")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let mut stored = item.payload.clone();
            stored["api_key"] = Value::String(String::new());
            if !key.is_empty() {
                secrets
                    .set(&provider_account(&pt, name.as_deref()), &key)
                    .map_err(|e| e.to_string())?;
            }
            if !cfg.get("providers").is_some_and(|v| v.is_array()) {
                cfg["providers"] = json!([]);
            }
            let list = cfg["providers"].as_array_mut().expect("just ensured array");
            match list
                .iter()
                .position(|e| provider_name(e) == Some((pt.clone(), name.clone())))
            {
                Some(i) => list[i] = stored,
                None => list.push(stored),
            }
        }
        "model" => {
            if !cfg.get("model_capabilities").is_some_and(|v| v.is_object()) {
                cfg["model_capabilities"] = json!({});
            }
            cfg["model_capabilities"][&item.name] = item.payload.clone();
        }
        "rule" | "workflow" => {
            let dir = if item.category == "rule" {
                rustic_agent::rules::global_rules_dir()
            } else {
                rustic_agent::workflows::global_workflows_dir()
            }
            .ok_or("cannot resolve home directory")?;
            let file = item
                .payload
                .get("file")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let file = if file.is_empty() {
                format!("{}.md", item.name)
            } else {
                file.to_string()
            };
            if file.contains('/') || file.contains('\\') || file.contains("..") {
                return Err(format!("unsafe file name `{file}`"));
            }
            let content = item
                .payload
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let path = dir.join(&file);
            trash_existing(&path);
            std::fs::write(&path, content).map_err(|e| e.to_string())?;
        }
        "skill" => {
            if item.name.contains('/') || item.name.contains('\\') || item.name.contains("..") {
                return Err(format!("unsafe skill name `{}`", item.name));
            }
            let base =
                rustic_agent::skills::global_skills_dir().ok_or("cannot resolve home directory")?;
            let dir = base.join(&item.name);
            trash_existing(&dir);
            let files = item
                .payload
                .get("files")
                .and_then(|v| v.as_object())
                .cloned()
                .unwrap_or_default();
            for (rel, b64) in files {
                if rel.split('/').any(|seg| seg == "..") || Path::new(&rel).is_absolute() {
                    return Err(format!("unsafe path `{rel}` in skill {}", item.name));
                }
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(b64.as_str().unwrap_or(""))
                    .map_err(|e| e.to_string())?;
                let path = dir.join(&rel);
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
            }
        }
        "mcp_server" => {
            if !pool.get("mcpServers").is_some_and(|v| v.is_object()) {
                pool["mcpServers"] = json!({});
            }
            pool["mcpServers"][&item.name] = item.payload.clone();
        }
        other => return Err(format!("unknown metadata category `{other}`")),
    }
    Ok(())
}

/// Merge `incoming` into this environment: add new items, replace only the
/// conflicting items whose key is in `overwrite`, keep everything else.
pub fn apply_bundle(
    state: &AppState,
    data_dir: &Path,
    secrets: &dyn SecretStore,
    incoming: &MetaBundle,
    overwrite: &HashSet<String>,
) -> MetaApplySummary {
    let local = export_bundle(state, data_dir, secrets);
    let local_keys: HashSet<String> = local.items.iter().map(|i| i.key()).collect();
    let (to_write, kept) = items_to_write(incoming, &local, overwrite);
    let mut summary = MetaApplySummary {
        kept,
        ..Default::default()
    };

    let mut cfg = read_ai_config(state);
    let pool_path = data_dir.join("mcp.json");
    let mut pool: Value = std::fs::read_to_string(&pool_path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| json!({ "mcpServers": {} }));
    let (mut cfg_changed, mut pool_changed) = (false, false);

    for item in to_write {
        match write_item(item, &mut cfg, &mut pool, secrets) {
            Ok(()) => {
                if local_keys.contains(&item.key()) {
                    summary.replaced += 1;
                } else {
                    summary.added += 1;
                }
                cfg_changed |= matches!(item.category.as_str(), "provider" | "model");
                pool_changed |= item.category == "mcp_server";
            }
            Err(e) => summary.errors.push(format!("{}: {}", item.key(), e)),
        }
    }

    if cfg_changed {
        let text = serde_json::to_string(&cfg).unwrap_or_default();
        let saved = state.db.lock_safe().set_setting("ai_config", &text);
        if let Err(e) = saved {
            summary.errors.push(format!("saving providers failed: {e}"));
        }
        crate::bootstrap::hydrate_config_and_secrets(state, secrets);
    }
    if pool_changed {
        match serde_json::to_string_pretty(&pool) {
            Ok(text) => {
                if let Err(e) = rustic_agent::io_util::atomic_write(&pool_path, text.as_bytes()) {
                    summary
                        .errors
                        .push(format!("saving MCP servers failed: {e}"));
                } else {
                    let mgr = std::sync::Arc::clone(&state.agent.lock_safe().mcp_manager);
                    let mut m = mgr.lock_safe();
                    m.set_user_path(pool_path.clone());
                    let _ = m.load_scope(rustic_agent::McpScope::User, &pool_path);
                }
            }
            Err(e) => summary.errors.push(e.to_string()),
        }
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shorthand for a test item.
    fn item(cat: &str, name: &str, v: Value) -> MetaItem {
        MetaItem {
            category: cat.into(),
            name: name.into(),
            payload: v,
        }
    }

    #[test]
    fn diff_classifies_new_conflict_same_and_local_only() {
        let incoming = MetaBundle {
            items: vec![
                item("rule", "a", json!({"content": "1"})),
                item("rule", "b", json!({"content": "2"})),
                item("mcp_server", "c", json!({"command": "x"})),
            ],
        };
        let local = MetaBundle {
            items: vec![
                item("rule", "b", json!({"content": "CHANGED"})),
                item("mcp_server", "c", json!({"command": "x"})),
                item("skill", "mine", json!({})),
            ],
        };
        let d = diff_bundles(&incoming, &local);
        let status = |k: &str| d.iter().find(|e| e.key == k).map(|e| e.status.clone());
        assert_eq!(status("rule/a"), Some(MetaStatus::New));
        assert_eq!(status("rule/b"), Some(MetaStatus::Conflict));
        assert_eq!(status("mcp_server/c"), Some(MetaStatus::Same));
        assert_eq!(status("skill/mine"), Some(MetaStatus::LocalOnly));
    }

    #[test]
    fn only_ticked_conflicts_are_overwritten() {
        let incoming = MetaBundle {
            items: vec![
                item("rule", "new", json!(1)),
                item("rule", "keep", json!(2)),
                item("rule", "replace", json!(3)),
            ],
        };
        let local = MetaBundle {
            items: vec![
                item("rule", "keep", json!(20)),
                item("rule", "replace", json!(30)),
            ],
        };
        let overwrite: HashSet<String> = ["rule/replace".to_string()].into_iter().collect();
        let (write, kept) = items_to_write(&incoming, &local, &overwrite);
        let keys: Vec<String> = write.iter().map(|i| i.key()).collect();
        assert_eq!(keys, vec!["rule/new", "rule/replace"]);
        assert_eq!(kept, 1);
    }

    #[test]
    fn unsafe_names_are_rejected() {
        let mut cfg = json!({});
        let mut pool = json!({});
        struct NoSecrets;
        impl SecretStore for NoSecrets {
            fn get(&self, _: &str) -> Result<Option<String>, String> {
                Ok(None)
            }
            fn set(&self, _: &str, _: &str) -> Result<(), String> {
                Ok(())
            }
            fn delete(&self, _: &str) -> Result<(), String> {
                Ok(())
            }
        }
        let bad = item("skill", "../evil", json!({"files": {}}));
        assert!(write_item(&bad, &mut cfg, &mut pool, &NoSecrets).is_err());
        let bad_rule = item("rule", "x", json!({"file": "../x.md", "content": ""}));
        assert!(write_item(&bad_rule, &mut cfg, &mut pool, &NoSecrets).is_err());
    }
}

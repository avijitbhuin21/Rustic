pub mod client;
pub mod config;

use crate::provider::ToolDef;
use anyhow::{anyhow, Result};
use client::McpClient;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub use config::{McpScope, McpServerConfig as ServerConfig, McpTransport};
use config::McpServerConfig;

/// Result of connecting a single server during save.
#[derive(Debug, Clone)]
pub struct McpConnectResult {
    pub name: String,
    pub connected: bool,
    pub tool_count: usize,
    pub error: Option<String>,
}

/// Last-known reachability for a server. `Unknown` means we haven't tried yet —
/// the panel treats that the same as a stale status and triggers a connect.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum McpConnectionStatus {
    Unknown,
    Connected { tool_count: usize },
    Failed { error: String },
}

/// Config paired with its last-known connection status — what the frontend renders.
#[derive(Debug, Clone, Serialize)]
pub struct McpServerWithStatus {
    #[serde(flatten)]
    pub config: McpServerConfig,
    pub status: McpConnectionStatus,
}

/// Manages MCP server connections loaded from per-scope JSON files.
///
/// Two sources of truth:
/// - **User** scope: a `mcp.json` in the app data dir — shared across projects.
/// - **Project** scope: a `.mcp.json` in the project root — committed to source.
///
/// Both files share the Claude Code format:
/// ```json
/// { "mcpServers": { "name": { "command": "npx", "args": [...], "env": {} } } }
/// ```
pub struct McpManager {
    configs: Vec<McpServerConfig>,
    clients: HashMap<String, McpClient>,
    /// Last-known connection status per server id. Refreshed on every connect attempt.
    status: HashMap<String, McpConnectionStatus>,
    /// Paths of the files the manager has loaded, so we know where to write back.
    user_path: Option<PathBuf>,
    project_path: Option<PathBuf>,
    /// Per-path cache of the on-disk file mtime (or `None` if the file was
    /// absent at load time). Presence in the map means we've successfully
    /// parsed that path at least once — `load_scope` uses this to skip the
    /// destructive kill-and-respawn cycle when the file hasn't changed.
    /// Keyed by path so switching to a different project's `.mcp.json` still
    /// triggers a real reload.
    loaded_mtime: HashMap<PathBuf, Option<SystemTime>>,
    /// Path to the persistent consent JSON file, set once at app init.
    consent_path: Option<PathBuf>,
    /// project_path → sha256(file content) of the last user-approved `.mcp.json`
    /// at that path. Project scope is auto-RCE-equivalent (we spawn arbitrary
    /// child processes named by the file), so we refuse to load it until the
    /// user has explicitly approved this exact byte sequence. Re-prompt on hash
    /// change so a malicious modification doesn't ride in on previous trust.
    project_consents: HashMap<PathBuf, String>,
}

/// Outcome of attempting to auto-load the project-scope `.mcp.json` under
/// the consent gate (F-10).
#[derive(Debug, Clone)]
pub enum LoadProjectScopeResult {
    /// File was loaded and `count` servers added to the manager.
    Loaded(usize),
    /// File exists but the user has not approved this content. Frontend must
    /// show the modal and call `approve_project_consent`. No servers loaded.
    ConsentRequired {
        project_path: PathBuf,
        content_hash: String,
        /// Raw content for the consent UI to display.
        content: String,
    },
    /// No `.mcp.json` present at the project root.
    NotPresent,
}

/// Compute a stable lowercase-hex SHA-256 of bytes. Used as the content-hash
/// for the `.mcp.json` consent gate (F-10). The hash is over the raw file
/// bytes — any whitespace / comment / formatting change is a new consent.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

impl McpManager {
    pub fn new() -> Self {
        Self {
            configs: Vec::new(),
            clients: HashMap::new(),
            status: HashMap::new(),
            user_path: None,
            project_path: None,
            loaded_mtime: HashMap::new(),
            consent_path: None,
            project_consents: HashMap::new(),
        }
    }

    /// Wire the manager to a persistent consent JSON at `path` (typically
    /// `<app_data_dir>/mcp_consent.json`). Reads existing entries; missing
    /// file is fine.
    pub fn set_consent_path(&mut self, path: PathBuf) {
        if path.exists() {
            if let Ok(text) = std::fs::read_to_string(&path) {
                if let Ok(map) = serde_json::from_str::<HashMap<PathBuf, String>>(&text) {
                    self.project_consents = map;
                }
            }
        }
        self.consent_path = Some(path);
    }

    fn persist_consents(&self) -> Result<()> {
        let Some(p) = self.consent_path.as_ref() else {
            return Ok(());
        };
        let text = serde_json::to_string_pretty(&self.project_consents)?;
        write_text_atomic(p, &text)
    }

    /// True iff the user has previously approved this exact content hash for
    /// the given project path.
    pub fn is_project_consented(&self, project_path: &Path, content_hash: &str) -> bool {
        self.project_consents
            .get(project_path)
            .map(|h| h == content_hash)
            .unwrap_or(false)
    }

    /// Record the user's approval of `content_hash` for `project_path`.
    pub fn approve_project_consent(
        &mut self,
        project_path: PathBuf,
        content_hash: String,
    ) -> Result<()> {
        self.project_consents.insert(project_path, content_hash);
        self.persist_consents()
    }

    /// Forget the user's prior approval at `project_path` (e.g. on project removal).
    pub fn revoke_project_consent(&mut self, project_path: &Path) -> Result<()> {
        if self.project_consents.remove(project_path).is_some() {
            self.persist_consents()?;
        }
        Ok(())
    }

    /// Like `load_scope(Project, path)` but gated on the per-project consent
    /// store (F-10). Returns `Loaded(n)` only when the user has approved this
    /// exact file content. Otherwise returns `ConsentRequired` and does NOT
    /// add any servers — the caller is responsible for surfacing the consent
    /// UI to the user.
    pub fn load_project_scope_gated(&mut self, path: &Path) -> Result<LoadProjectScopeResult> {
        self.project_path = Some(path.to_path_buf());
        if !path.exists() {
            // Clear out anything we may have loaded previously for this scope.
            self.remove_scope(McpScope::Project);
            self.loaded_mtime.insert(path.to_path_buf(), None);
            return Ok(LoadProjectScopeResult::NotPresent);
        }

        let text = std::fs::read_to_string(path)?;
        let content_hash = sha256_hex(text.as_bytes());

        if !self.is_project_consented(path, &content_hash) {
            // Refuse to load. Drop any previously-loaded project servers so
            // a stale-trusted hash from before the file changed can't keep
            // running.
            self.remove_scope(McpScope::Project);
            return Ok(LoadProjectScopeResult::ConsentRequired {
                project_path: path.to_path_buf(),
                content_hash,
                content: text,
            });
        }

        // Consent matches — load normally. Re-use the same parsing logic as
        // `load_scope` rather than duplicating; we already have the text.
        let on_disk = Self::current_mtime(path);
        if let Some(cached) = self.loaded_mtime.get(path) {
            if *cached == on_disk {
                let count = self
                    .configs
                    .iter()
                    .filter(|c| c.scope == McpScope::Project)
                    .count();
                return Ok(LoadProjectScopeResult::Loaded(count));
            }
        }

        self.remove_scope(McpScope::Project);
        let parsed = parse_mcp_json(&text)?;
        let mut count = 0;
        for (name, transport) in parsed {
            let id = format!("{}-{}", scope_prefix(McpScope::Project), name);
            self.configs.push(McpServerConfig {
                id,
                name,
                transport,
                enabled: true,
                scope: McpScope::Project,
            });
            count += 1;
        }
        self.loaded_mtime.insert(path.to_path_buf(), on_disk);
        Ok(LoadProjectScopeResult::Loaded(count))
    }

    fn current_mtime(path: &Path) -> Option<SystemTime> {
        std::fs::metadata(path).and_then(|m| m.modified()).ok()
    }

    pub fn set_user_path(&mut self, path: PathBuf) {
        self.user_path = Some(path);
    }

    pub fn set_project_path(&mut self, path: PathBuf) {
        self.project_path = Some(path);
    }

    pub fn path_for(&self, scope: McpScope) -> Option<&Path> {
        match scope {
            McpScope::User => self.user_path.as_deref(),
            McpScope::Project => self.project_path.as_deref(),
        }
    }

    pub fn list_servers(&self) -> Vec<McpServerConfig> {
        self.configs.clone()
    }

    /// Configs paired with their last-known connection status — what the UI shows.
    pub fn list_servers_with_status(&self) -> Vec<McpServerWithStatus> {
        self.configs
            .iter()
            .map(|c| McpServerWithStatus {
                config: c.clone(),
                status: self
                    .status
                    .get(&c.id)
                    .cloned()
                    .unwrap_or(McpConnectionStatus::Unknown),
            })
            .collect()
    }

    /// Drop every config tagged with `scope`, disconnecting any live clients
    /// and clearing their status.
    fn remove_scope(&mut self, scope: McpScope) {
        let to_remove: Vec<String> = self
            .configs
            .iter()
            .filter(|c| c.scope == scope)
            .map(|c| c.id.clone())
            .collect();
        for id in &to_remove {
            if let Some(mut client) = self.clients.remove(id) {
                client.disconnect();
            }
            self.status.remove(id);
        }
        self.configs.retain(|c| c.scope != scope);
    }

    /// Load `.mcp.json` content for a scope, replacing any existing entries for that scope.
    /// Missing file is not an error — it clears the scope.
    pub fn load_scope(&mut self, scope: McpScope, path: &Path) -> Result<usize> {
        match scope {
            McpScope::User => self.user_path = Some(path.to_path_buf()),
            McpScope::Project => self.project_path = Some(path.to_path_buf()),
        }

        let on_disk = Self::current_mtime(path);
        if let Some(cached) = self.loaded_mtime.get(path) {
            if *cached == on_disk {
                let count = self.configs.iter().filter(|c| c.scope == scope).count();
                return Ok(count);
            }
        }

        self.remove_scope(scope);

        if !path.exists() {
            self.loaded_mtime.insert(path.to_path_buf(), None);
            return Ok(0);
        }

        let text = std::fs::read_to_string(path)?;
        let parsed = parse_mcp_json(&text)?;

        let mut count = 0;
        for (name, transport) in parsed {
            let id = format!("{}-{}", scope_prefix(scope), name);
            self.configs.push(McpServerConfig {
                id,
                name,
                transport,
                enabled: true,
                scope,
            });
            count += 1;
        }
        self.loaded_mtime.insert(path.to_path_buf(), on_disk);
        Ok(count)
    }

    /// Write the current configs for `scope` back to its JSON file (atomic).
    /// If no configs remain in the scope, writes an empty `{"mcpServers": {}}` file.
    pub fn save_scope(&mut self, scope: McpScope) -> Result<()> {
        let path = self
            .path_for(scope)
            .ok_or_else(|| anyhow!("No path set for scope {:?}", scope))?
            .to_path_buf();

        let mut servers = Map::new();
        for cfg in self.configs.iter().filter(|c| c.scope == scope) {
            servers.insert(cfg.name.clone(), transport_to_json(&cfg.transport));
        }
        let root = json!({ "mcpServers": Value::Object(servers) });
        write_json_atomic(&path, &root)?;
        self.loaded_mtime
            .insert(path.clone(), Self::current_mtime(&path));
        Ok(())
    }

    /// Overwrite a scope's JSON file with raw content, validate it, then reload.
    /// Returns the names of servers that are now in the scope.
    pub fn save_scope_raw(&mut self, scope: McpScope, content: &str) -> Result<Vec<String>> {
        let parsed = parse_mcp_json(content)?;
        let path = self
            .path_for(scope)
            .ok_or_else(|| anyhow!("No path set for scope {:?}", scope))?
            .to_path_buf();

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Write bytes as the user typed them so formatting/comments-sans are preserved.
        write_text_atomic(&path, content)?;
        self.loaded_mtime
            .insert(path.clone(), Self::current_mtime(&path));

        // F-10: an explicit save through this command IS the user's consent —
        // they typed (or pasted-and-saved) the content themselves through our
        // own UI, so record their approval of this exact byte sequence. The
        // auto-load path on agent task start will then proceed without
        // re-prompting.
        if scope == McpScope::Project {
            let _ = self.approve_project_consent(path.clone(), sha256_hex(content.as_bytes()));
        }

        self.remove_scope(scope);
        let mut names = Vec::new();
        for (name, transport) in parsed {
            let id = format!("{}-{}", scope_prefix(scope), name);
            self.configs.push(McpServerConfig {
                id,
                name: name.clone(),
                transport,
                enabled: true,
                scope,
            });
            names.push(name);
        }
        Ok(names)
    }

    /// Connect to each server in `scope` and return per-server success/failure.
    /// Replaces any existing live clients for the scope and refreshes their status.
    pub fn test_scope(&mut self, scope: McpScope) -> Vec<McpConnectResult> {
        let targets: Vec<McpServerConfig> = self
            .configs
            .iter()
            .filter(|c| c.scope == scope)
            .cloned()
            .collect();

        let mut results = Vec::with_capacity(targets.len());
        for cfg in targets {
            if let Some(mut c) = self.clients.remove(&cfg.id) {
                c.disconnect();
            }
            let result = self.connect_one(&cfg);
            results.push(McpConnectResult {
                name: cfg.name.clone(),
                connected: result.0,
                tool_count: result.1,
                error: result.2,
            });
        }
        results
    }

    /// Connect to a single server by id and list its tools (for the per-row Test button).
    /// Also refreshes the cached status.
    pub fn test_server(&mut self, id: &str) -> Result<Vec<ToolDef>> {
        let config = self
            .configs
            .iter()
            .find(|c| c.id == id)
            .cloned()
            .ok_or_else(|| anyhow!("Server not found: {}", id))?;

        // Close the previous live connection so we re-test cleanly.
        if let Some(mut c) = self.clients.remove(&config.id) {
            c.disconnect();
        }

        match McpClient::connect(config.clone()) {
            Ok(mut client) => match client.list_tools() {
                Ok(tools) => {
                    self.status.insert(
                        config.id.clone(),
                        McpConnectionStatus::Connected { tool_count: tools.len() },
                    );
                    self.clients.insert(config.id, client);
                    Ok(tools)
                }
                Err(e) => {
                    client.disconnect();
                    self.status.insert(
                        config.id,
                        McpConnectionStatus::Failed { error: e.to_string() },
                    );
                    Err(e)
                }
            },
            Err(e) => {
                self.status.insert(
                    config.id,
                    McpConnectionStatus::Failed { error: e.to_string() },
                );
                Err(e)
            }
        }
    }

    /// Remove a single server by id — strips it from the backing scope file and disconnects.
    pub fn remove_server(&mut self, id: &str) -> Result<()> {
        let scope = self
            .configs
            .iter()
            .find(|c| c.id == id)
            .map(|c| c.scope)
            .ok_or_else(|| anyhow!("Server not found: {}", id))?;

        self.configs.retain(|c| c.id != id);
        if let Some(mut client) = self.clients.remove(id) {
            client.disconnect();
        }
        self.status.remove(id);
        self.save_scope(scope)
    }

    /// Connect every server with status `Unknown` (or that has no live client yet).
    /// Already-connected servers are left alone. Updates the status map for all attempts.
    pub fn connect_all(&mut self) -> Result<()> {
        let targets: Vec<McpServerConfig> = self
            .configs
            .iter()
            .filter(|c| c.enabled && !self.clients.contains_key(&c.id))
            .cloned()
            .collect();
        for cfg in targets {
            let _ = self.connect_one(&cfg);
        }
        Ok(())
    }

    /// Connect a single config, update the status map, and return (connected, tool_count, error).
    fn connect_one(&mut self, cfg: &McpServerConfig) -> (bool, usize, Option<String>) {
        match McpClient::connect(cfg.clone()) {
            Ok(mut client) => match client.list_tools() {
                Ok(tools) => {
                    let count = tools.len();
                    self.status.insert(
                        cfg.id.clone(),
                        McpConnectionStatus::Connected { tool_count: count },
                    );
                    self.clients.insert(cfg.id.clone(), client);
                    (true, count, None)
                }
                Err(e) => {
                    client.disconnect();
                    let msg = e.to_string();
                    self.status.insert(
                        cfg.id.clone(),
                        McpConnectionStatus::Failed { error: msg.clone() },
                    );
                    (false, 0, Some(msg))
                }
            },
            Err(e) => {
                let msg = e.to_string();
                self.status.insert(
                    cfg.id.clone(),
                    McpConnectionStatus::Failed { error: msg.clone() },
                );
                (false, 0, Some(msg))
            }
        }
    }

    /// Get tool definitions from all connected servers.
    ///
    /// Iteration order is **sorted by server id** for cache stability.
    /// `clients` is a `HashMap` (RandomState), so naive iteration produces a
    /// different order each call — which then permutes the `tools` block of
    /// the API request and invalidates Anthropic's prompt-prefix cache
    /// between user messages (R.2 F3 / P0.6). Sorting by id makes the order
    /// deterministic for a fixed set of configured servers.
    pub fn all_tools(&mut self) -> Vec<ToolDef> {
        let mut ids: Vec<String> = self.clients.keys().cloned().collect();
        ids.sort();
        let mut tools = Vec::new();
        for id in ids {
            if let Some(client) = self.clients.get_mut(&id) {
                if let Ok(t) = client.list_tools() {
                    tools.extend(t);
                }
            }
        }
        tools
    }

    /// Call a tool on the appropriate server.
    pub fn call_tool(&mut self, name: &str, arguments: Value) -> Result<Value> {
        for (_, client) in &mut self.clients {
            if let Ok(tools) = client.list_tools() {
                if tools.iter().any(|t| t.name == name) {
                    return client.call_tool(name, arguments);
                }
            }
        }
        Err(anyhow!("No MCP server provides tool: {}", name))
    }

    pub fn disconnect_all(&mut self) {
        for (_, mut client) in self.clients.drain() {
            client.disconnect();
        }
    }
}

fn scope_prefix(scope: McpScope) -> &'static str {
    match scope {
        McpScope::User => "user",
        McpScope::Project => "project",
    }
}

/// Parse `.mcp.json` content into a list of (name, transport) pairs.
///
/// The accepted shape matches Claude Code:
/// - `command` + `args` + optional `env` → stdio transport
/// - `url` + optional `headers` → sse transport
fn parse_mcp_json(text: &str) -> Result<Vec<(String, McpTransport)>> {
    let json: Value = serde_json::from_str(text)
        .map_err(|e| anyhow!("Invalid JSON: {}", e))?;

    let servers_map = json
        .get("mcpServers")
        .and_then(|v| v.as_object())
        .ok_or_else(|| anyhow!("Missing or invalid \"mcpServers\" object"))?;

    let mut out = Vec::new();
    for (name, def) in servers_map {
        let transport = if let Some(url) = def.get("url").and_then(|v| v.as_str()) {
            let headers = def
                .get("headers")
                .and_then(|v| v.as_object())
                .map(|obj| {
                    obj.iter()
                        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                        .collect()
                })
                .unwrap_or_default();
            McpTransport::Sse {
                url: url.to_string(),
                headers,
            }
        } else {
            let command = def
                .get("command")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("Server \"{}\" is missing both \"command\" and \"url\"", name))?
                .to_string();
            let args = def
                .get("args")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            let env = def
                .get("env")
                .and_then(|v| v.as_object())
                .map(|obj| {
                    obj.iter()
                        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                        .collect()
                })
                .unwrap_or_default();
            McpTransport::Stdio { command, args, env }
        };
        out.push((name.clone(), transport));
    }
    Ok(out)
}

fn transport_to_json(transport: &McpTransport) -> Value {
    match transport {
        McpTransport::Stdio { command, args, env } => {
            let mut obj = Map::new();
            obj.insert("command".into(), Value::String(command.clone()));
            obj.insert(
                "args".into(),
                Value::Array(args.iter().map(|s| Value::String(s.clone())).collect()),
            );
            if !env.is_empty() {
                let env_obj: Map<String, Value> = env
                    .iter()
                    .map(|(k, v)| (k.clone(), Value::String(v.clone())))
                    .collect();
                obj.insert("env".into(), Value::Object(env_obj));
            }
            Value::Object(obj)
        }
        McpTransport::Sse { url, headers } => {
            let mut obj = Map::new();
            obj.insert("url".into(), Value::String(url.clone()));
            if !headers.is_empty() {
                let h_obj: Map<String, Value> = headers
                    .iter()
                    .map(|(k, v)| (k.clone(), Value::String(v.clone())))
                    .collect();
                obj.insert("headers".into(), Value::Object(h_obj));
            }
            Value::Object(obj)
        }
    }
}

fn write_json_atomic(path: &Path, value: &Value) -> Result<()> {
    let text = serde_json::to_string_pretty(value)?;
    write_text_atomic(path, &text)
}

fn write_text_atomic(path: &Path, text: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    crate::io_util::atomic_write(path, text.as_bytes())?;
    Ok(())
}

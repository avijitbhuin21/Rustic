pub mod client;
pub mod config;
pub mod project_files;

use crate::provider::ToolDef;
use anyhow::{anyhow, Result};
use client::McpClient;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::SystemTime;

use config::McpServerConfig;
pub use config::{McpScope, McpServerConfig as ServerConfig, McpTransport};

/// Last-known reachability for a server. `Unknown` means we haven't tried yet —
/// the panel treats that the same as a stale status and triggers a connect.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum McpConnectionStatus {
    Unknown,
    /// A connect or re-test is running on a background thread.
    Connecting,
    Connected {
        tool_count: usize,
    },
    Failed {
        error: String,
    },
}

/// Host callback fired with `(server id, status, sequence)` whenever a server's
/// status changes. The sequence only grows, so a client can tell a status push
/// from an older list snapshot that reaches it later.
pub type McpStatusListener = Arc<dyn Fn(&str, &McpConnectionStatus, u64) + Send + Sync>;

/// Config paired with its last-known connection status — what the frontend renders.
#[derive(Debug, Clone, Serialize)]
pub struct McpServerWithStatus {
    #[serde(flatten)]
    pub config: McpServerConfig,
    pub status: McpConnectionStatus,
    /// Status sequence when this row was read; a push with a higher one is newer.
    pub status_seq: u64,
}

/// One server as seen from one project, for the per-project Configure UI.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpProjectServerView {
    /// Entry JSON the project uses (its `.mcp.json` override, else the pool entry).
    pub entry: Value,
    pub enabled: bool,
    /// True when the project's `.mcp.json` has its own entry for this server.
    pub overridden: bool,
    pub status: McpConnectionStatus,
}

/// Outcome of saving a project's server config.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpProjectSaveResult {
    pub enabled: bool,
    pub connected: bool,
    pub tool_count: usize,
    pub error: Option<String>,
    /// The project's `.mcp.json` has content the user hasn't approved yet, so
    /// project overrides were not loaded.
    pub consent_required: bool,
}

/// Tool caches older than this are re-listed (in the background at turn start,
/// inline in `connect_all`), making mid-session server tool changes visible
/// without reconnect.
const TOOL_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(300);

type SharedClient = std::sync::Arc<std::sync::Mutex<McpClient>>;

/// What a turn needs from MCP, served from the connect-time cache.
#[derive(Debug, Default)]
pub struct McpTurnTools {
    pub tools: Vec<ToolDef>,
    /// Read-only tool name → id of the server its calls are routed to.
    pub read_only: HashMap<String, String>,
}

/// A connected server whose stale tool cache was claimed for a re-list.
struct ToolRefreshJob {
    id: String,
    client: SharedClient,
}

enum ToolRefreshOutcome {
    Listed(Vec<ToolDef>, std::collections::HashSet<String>),
    Failed(String),
    /// The client is mid-call; nothing was listed.
    Busy,
}

/// Re-list one server's tools, skipping a client that is busy with a call.
fn list_for_refresh(client: &std::sync::Mutex<McpClient>) -> ToolRefreshOutcome {
    match client.try_lock() {
        Ok(mut c) => match c.list_tools() {
            Ok(tools) => ToolRefreshOutcome::Listed(tools, c.read_only_tools().clone()),
            Err(e) => ToolRefreshOutcome::Failed(e.to_string()),
        },
        Err(std::sync::TryLockError::WouldBlock) => ToolRefreshOutcome::Busy,
        Err(std::sync::TryLockError::Poisoned(_)) => {
            ToolRefreshOutcome::Failed("client mutex poisoned".into())
        }
    }
}

/// One claimed connect attempt, run on a background thread off the manager lock.
#[derive(Clone)]
struct ConnectJob {
    cfg: McpServerConfig,
    /// Matches `McpManager::connecting[id]` while this attempt is the current one.
    token: u64,
}

type ConnectOutcome = std::result::Result<(McpClient, Vec<ToolDef>), String>;

/// Spawn/handshake a server and list its tools. Slow; never call under the manager lock.
fn connect_and_list(cfg: &McpServerConfig) -> ConnectOutcome {
    let mut client = McpClient::connect(cfg.clone()).map_err(|e| e.to_string())?;
    match client.list_tools() {
        Ok(tools) => Ok((client, tools)),
        Err(e) => {
            client.disconnect();
            Err(e.to_string())
        }
    }
}

/// Wakes threads waiting for background connects to settle.
#[derive(Default)]
struct ConnectSignal {
    generation: Mutex<u64>,
    changed: Condvar,
}

impl ConnectSignal {
    /// Current generation; read it BEFORE checking state, then `wait_past` it.
    fn generation(&self) -> u64 {
        *self.generation.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Record that a connect attempt finished and wake every waiter.
    fn bump(&self) {
        *self.generation.lock().unwrap_or_else(|p| p.into_inner()) += 1;
        self.changed.notify_all();
    }

    /// Block until the generation moves past `seen`.
    fn wait_past(&self, seen: u64) {
        let mut g = self.generation.lock().unwrap_or_else(|p| p.into_inner());
        while *g == seen {
            g = self.changed.wait(g).unwrap_or_else(|p| p.into_inner());
        }
    }
}

/// Disconnect a client that left the manager, on its own thread: a remote
/// teardown can take seconds and must not run under the manager lock. A client
/// busy with a call is left alone — it disconnects on drop when that call returns.
fn release_client(client: SharedClient) {
    let fallback = Arc::clone(&client);
    let spawned = std::thread::Builder::new()
        .name("mcp-disconnect".into())
        .spawn(move || {
            if let Ok(mut c) = client.try_lock() {
                c.disconnect();
            }
        });
    if spawned.is_err() {
        if let Ok(mut c) = fallback.try_lock() {
            c.disconnect();
        }
    }
}

/// Whether two transports describe the same server entry.
fn same_transport(a: &McpTransport, b: &McpTransport) -> bool {
    transport_to_json(a) == transport_to_json(b)
}

/// Manages MCP server connections loaded from per-scope JSON files (Claude Code format).
pub struct McpManager {
    configs: Vec<McpServerConfig>,
    /// Per-client mutexes so calls to DIFFERENT servers can run in parallel;
    /// calls to the same server stay serialized (single stdio pipe / session).
    clients: HashMap<String, std::sync::Arc<std::sync::Mutex<McpClient>>>,
    /// Tool lists captured at connect time. Used for dispatch routing and
    /// `all_tools` so neither costs an IPC/network round-trip per turn.
    tool_cache: HashMap<String, Vec<ToolDef>>,
    /// Per-server names of tools annotated `readOnlyHint`, refreshed with `tool_cache`.
    read_only_cache: HashMap<String, std::collections::HashSet<String>>,
    /// When each server's tool cache was last (re)listed — drives the
    /// per-turn staleness refresh in `connect_all`.
    tool_cache_at: HashMap<String, std::time::Instant>,
    status: HashMap<String, McpConnectionStatus>,
    /// Servers with a connect in flight → that attempt's token. A result whose
    /// token no longer matches belongs to a superseded attempt and is discarded.
    connecting: HashMap<String, u64>,
    connect_seq: u64,
    connect_signal: Arc<ConnectSignal>,
    status_listener: Option<McpStatusListener>,
    /// Bumped on every status push. Seeded from the clock so it keeps growing
    /// across a backend restart that a still-open web client lives through.
    status_seq: std::sync::atomic::AtomicU64,
    user_path: Option<PathBuf>,
    project_path: Option<PathBuf>,
    /// Keyed by path so switching to a different project's `.mcp.json` still triggers a reload.
    loaded_mtime: HashMap<PathBuf, Option<SystemTime>>,
    consent_path: Option<PathBuf>,
    /// Refuse to load project scope until the user has approved this exact byte sequence.
    /// Re-prompt on hash change so a malicious modification can't ride in on previous trust.
    project_consents: HashMap<PathBuf, String>,
    /// Per-project disabled server names (from `<root>/.rustic/mcp.json`),
    /// keyed by `project_key(root)`. A key being present also marks the
    /// project as loaded, which switches tool routing to project-aware mode.
    project_disabled: HashMap<String, BTreeSet<String>>,
}

/// Result of the consent-gated project-scope load (F-10).
#[derive(Debug, Clone)]
pub enum LoadProjectScopeResult {
    Loaded(usize),
    /// User has not approved this content; frontend must show the consent modal.
    ConsentRequired {
        project_path: PathBuf,
        content_hash: String,
        content: String,
    },
    NotPresent,
}

/// Stable lowercase-hex SHA-256. Hash is over raw bytes so any formatting change is a new consent.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

impl Default for McpManager {
    fn default() -> Self {
        Self::new()
    }
}

impl McpManager {
    pub fn new() -> Self {
        Self {
            configs: Vec::new(),
            clients: HashMap::new(),
            tool_cache: HashMap::new(),
            read_only_cache: HashMap::new(),
            tool_cache_at: HashMap::new(),
            status: HashMap::new(),
            connecting: HashMap::new(),
            connect_seq: 0,
            connect_signal: Arc::default(),
            status_listener: None,
            status_seq: std::sync::atomic::AtomicU64::new(
                SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0),
            ),
            user_path: None,
            project_path: None,
            loaded_mtime: HashMap::new(),
            consent_path: None,
            project_consents: HashMap::new(),
            project_disabled: HashMap::new(),
        }
    }

    /// Lock the shared manager, recovering from a poisoned mutex.
    fn locked(manager: &Arc<Mutex<Self>>) -> MutexGuard<'_, Self> {
        manager.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Register the host callback that pushes status changes to the UI.
    pub fn set_status_listener(&mut self, listener: McpStatusListener) {
        self.status_listener = Some(listener);
    }

    /// Status to show for `id`: an in-flight connect wins over the last result.
    fn status_of(&self, id: &str) -> McpConnectionStatus {
        if self.connecting.contains_key(id) {
            return McpConnectionStatus::Connecting;
        }
        self.status
            .get(id)
            .cloned()
            .unwrap_or(McpConnectionStatus::Unknown)
    }

    /// Tell the host that `id`'s status changed.
    fn notify_status(&self, id: &str) {
        let seq = self
            .status_seq
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1;
        if let Some(listener) = &self.status_listener {
            listener(id, &self.status_of(id), seq);
        }
    }

    /// One list row for `cfg`, stamped with the current status sequence.
    fn row(&self, cfg: &McpServerConfig) -> McpServerWithStatus {
        McpServerWithStatus {
            config: cfg.clone(),
            status: self.status_of(&cfg.id),
            status_seq: self.status_seq.load(std::sync::atomic::Ordering::Relaxed),
        }
    }

    /// Wire the manager to a persistent consent JSON. Reads existing entries; missing file is fine.
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

    pub fn is_project_consented(&self, project_path: &Path, content_hash: &str) -> bool {
        self.project_consents
            .get(project_path)
            .map(|h| h == content_hash)
            .unwrap_or(false)
    }

    pub fn approve_project_consent(
        &mut self,
        project_path: PathBuf,
        content_hash: String,
    ) -> Result<()> {
        self.project_consents.insert(project_path, content_hash);
        self.persist_consents()
    }

    pub fn revoke_project_consent(&mut self, project_path: &Path) -> Result<()> {
        if self.project_consents.remove(project_path).is_some() {
            self.persist_consents()?;
        }
        Ok(())
    }

    /// Snapshot of every approved (project `.mcp.json` path → content hash).
    pub fn consents(&self) -> HashMap<PathBuf, String> {
        self.project_consents.clone()
    }

    /// Merge approvals (e.g. carried by a sync archive, already remapped to
    /// this machine's paths) and persist them.
    pub fn import_consents(&mut self, entries: Vec<(PathBuf, String)>) -> Result<()> {
        if entries.is_empty() {
            return Ok(());
        }
        for (path, hash) in entries {
            self.project_consents.insert(path, hash);
        }
        self.persist_consents()
    }

    /// Like `load_scope(Project, path)` but gated on per-project consent (F-10).
    pub fn load_project_scope_gated(&mut self, path: &Path) -> Result<LoadProjectScopeResult> {
        self.project_path = Some(path.to_path_buf());
        if let Some(root) = path.parent() {
            self.project_disabled
                .insert(project_key(root), project_files::read_disabled(root));
        }
        if !path.exists() {
            self.remove_scope(McpScope::Project);
            self.loaded_mtime.insert(path.to_path_buf(), None);
            return Ok(LoadProjectScopeResult::NotPresent);
        }

        let text = std::fs::read_to_string(path)?;
        let content_hash = sha256_hex(text.as_bytes());

        if !self.is_project_consented(path, &content_hash) {
            // Drop previously-loaded servers so a stale-trusted hash can't keep running.
            self.remove_scope(McpScope::Project);
            self.loaded_mtime.remove(path);
            return Ok(LoadProjectScopeResult::ConsentRequired {
                project_path: path.to_path_buf(),
                content_hash,
                content: text,
            });
        }

        let on_disk = Self::current_mtime(path);
        if let Some(cached) = self.loaded_mtime.get(path) {
            if *cached == on_disk {
                let count = self
                    .configs
                    .iter()
                    .filter(|c| self.in_scope(c, McpScope::Project))
                    .count();
                return Ok(LoadProjectScopeResult::Loaded(count));
            }
        }

        let parsed = match parse_mcp_json(&text) {
            Ok(parsed) => parsed,
            Err(e) => {
                self.remove_scope(McpScope::Project);
                return Err(e);
            }
        };
        let count = self.replace_scope(McpScope::Project, parsed).len();
        self.loaded_mtime.insert(path.to_path_buf(), on_disk);
        Ok(LoadProjectScopeResult::Loaded(count))
    }

    fn current_mtime(path: &Path) -> Option<SystemTime> {
        std::fs::metadata(path).and_then(|m| m.modified()).ok()
    }

    /// Id prefix of the project whose `.mcp.json` is the current project path.
    fn current_project_prefix(&self) -> Option<String> {
        self.project_path
            .as_deref()
            .and_then(Path::parent)
            .map(project_id_prefix)
    }

    /// Whether `c` belongs to `scope`; project scope means the CURRENT project only.
    fn in_scope(&self, c: &McpServerConfig, scope: McpScope) -> bool {
        if c.scope != scope {
            return false;
        }
        match (scope, self.current_project_prefix()) {
            (McpScope::Project, Some(p)) => c.id.starts_with(&p),
            _ => true,
        }
    }

    /// Config id for `name` in `scope` (project ids carry the project prefix).
    fn scope_id(&self, scope: McpScope, name: &str) -> String {
        match (scope, self.current_project_prefix()) {
            (McpScope::Project, Some(p)) => format!("{}{}", p, name),
            _ => format!("{}-{}", scope_prefix(scope), name),
        }
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
        self.configs.iter().map(|c| self.row(c)).collect()
    }

    /// Servers of one scope (project scope = the current project) with status.
    pub fn list_scope_with_status(&self, scope: McpScope) -> Vec<McpServerWithStatus> {
        self.configs
            .iter()
            .filter(|c| self.in_scope(c, scope))
            .map(|c| self.row(c))
            .collect()
    }

    /// Drop everything known about `id`: client, caches, status, in-flight connect.
    fn forget_server(&mut self, id: &str) {
        self.drop_client(id);
        self.status.remove(id);
        self.connecting.remove(id);
    }

    /// Drop every config tagged with `scope`, disconnecting any live clients
    /// and clearing their status.
    fn remove_scope(&mut self, scope: McpScope) {
        let to_remove: Vec<String> = self
            .configs
            .iter()
            .filter(|c| self.in_scope(c, scope))
            .map(|c| c.id.clone())
            .collect();
        for id in &to_remove {
            self.forget_server(id);
        }
        self.configs.retain(|c| !to_remove.contains(&c.id));
    }

    /// Swap `scope`'s configs for `parsed`. A server whose entry is unchanged
    /// keeps its client, caches and status, so reloading a file only
    /// reconnects what actually changed. Returns the new server names.
    fn replace_scope(
        &mut self,
        scope: McpScope,
        parsed: Vec<(String, McpTransport)>,
    ) -> Vec<String> {
        let next: Vec<McpServerConfig> = parsed
            .into_iter()
            .map(|(name, transport)| McpServerConfig {
                id: self.scope_id(scope, &name),
                name,
                transport,
                enabled: true,
                scope,
            })
            .collect();
        let old: Vec<McpServerConfig> = self
            .configs
            .iter()
            .filter(|c| self.in_scope(c, scope))
            .cloned()
            .collect();
        for prev in &old {
            let unchanged = next
                .iter()
                .any(|n| n.id == prev.id && same_transport(&n.transport, &prev.transport));
            if !unchanged {
                self.forget_server(&prev.id);
            }
        }
        self.configs.retain(|c| !old.iter().any(|o| o.id == c.id));
        let names = next.iter().map(|c| c.name.clone()).collect();
        self.configs.extend(next);
        names
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
                let count = self
                    .configs
                    .iter()
                    .filter(|c| self.in_scope(c, scope))
                    .count();
                return Ok(count);
            }
        }

        if !path.exists() {
            self.remove_scope(scope);
            self.loaded_mtime.insert(path.to_path_buf(), None);
            return Ok(0);
        }

        let parsed = std::fs::read_to_string(path)
            .map_err(anyhow::Error::from)
            .and_then(|text| parse_mcp_json(&text));
        let parsed = match parsed {
            Ok(parsed) => parsed,
            Err(e) => {
                self.remove_scope(scope);
                return Err(e);
            }
        };
        let count = self.replace_scope(scope, parsed).len();
        self.loaded_mtime.insert(path.to_path_buf(), on_disk);
        Ok(count)
    }

    pub fn save_scope(&mut self, scope: McpScope) -> Result<()> {
        let path = self
            .path_for(scope)
            .ok_or_else(|| anyhow!("No path set for scope {:?}", scope))?
            .to_path_buf();

        let mut servers = Map::new();
        for cfg in self.configs.iter().filter(|c| self.in_scope(c, scope)) {
            servers.insert(cfg.name.clone(), transport_to_json(&cfg.transport));
        }
        let root = json!({ "mcpServers": Value::Object(servers) });
        write_json_atomic(&path, &root)?;
        self.loaded_mtime
            .insert(path.clone(), Self::current_mtime(&path));
        // F-10 parity with `save_scope_raw`: a save through the manager is an
        // explicit, already-authorized change (Settings UI or broker-approved
        // agent install) — approve the new content hash so the next gated
        // project-scope load doesn't re-prompt for our own write.
        if scope == McpScope::Project {
            if let Ok(text) = std::fs::read_to_string(&path) {
                let _ = self.approve_project_consent(path.clone(), sha256_hex(text.as_bytes()));
            }
        }
        Ok(())
    }

    pub fn save_scope_raw(&mut self, scope: McpScope, content: &str) -> Result<Vec<String>> {
        let parsed = parse_mcp_json(content)?;
        let path = self
            .path_for(scope)
            .ok_or_else(|| anyhow!("No path set for scope {:?}", scope))?
            .to_path_buf();

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        write_text_atomic(&path, content)?;
        self.loaded_mtime
            .insert(path.clone(), Self::current_mtime(&path));

        // F-10: explicit save = consent; auto-load on next task start won't re-prompt.
        if scope == McpScope::Project {
            let _ = self.approve_project_consent(path.clone(), sha256_hex(content.as_bytes()));
        }

        Ok(self.replace_scope(scope, parsed))
    }

    pub fn test_server(&mut self, id: &str) -> Result<Vec<ToolDef>> {
        let config = self
            .configs
            .iter()
            .find(|c| c.id == id)
            .cloned()
            .ok_or_else(|| anyhow!("Server not found: {}", id))?;

        self.drop_client(&config.id);

        match McpClient::connect(config.clone()) {
            Ok(mut client) => match client.list_tools() {
                Ok(tools) => {
                    self.status.insert(
                        config.id.clone(),
                        McpConnectionStatus::Connected {
                            tool_count: tools.len(),
                        },
                    );
                    self.read_only_cache
                        .insert(config.id.clone(), client.read_only_tools().clone());
                    self.tool_cache.insert(config.id.clone(), tools.clone());
                    self.tool_cache_at
                        .insert(config.id.clone(), std::time::Instant::now());
                    self.clients.insert(
                        config.id,
                        std::sync::Arc::new(std::sync::Mutex::new(client)),
                    );
                    Ok(tools)
                }
                Err(e) => {
                    client.disconnect();
                    self.status.insert(
                        config.id,
                        McpConnectionStatus::Failed {
                            error: e.to_string(),
                        },
                    );
                    Err(e)
                }
            },
            Err(e) => {
                self.status.insert(
                    config.id,
                    McpConnectionStatus::Failed {
                        error: e.to_string(),
                    },
                );
                Err(e)
            }
        }
    }

    pub fn remove_server(&mut self, id: &str) -> Result<()> {
        let scope = self
            .configs
            .iter()
            .find(|c| c.id == id)
            .map(|c| c.scope)
            .ok_or_else(|| anyhow!("Server not found: {}", id))?;

        self.configs.retain(|c| c.id != id);
        self.forget_server(id);
        self.save_scope(scope)
    }

    /// Add a server to a scope, persist the scope file, and connect it.
    /// Returns `(id, connect_result)` — the config is kept (with Failed
    /// status) even when the initial connection fails, matching the
    /// Settings-UI behavior for misconfigured servers.
    pub fn add_server(
        &mut self,
        scope: McpScope,
        name: &str,
        transport: McpTransport,
    ) -> Result<(String, std::result::Result<Vec<ToolDef>, String>)> {
        if self
            .configs
            .iter()
            .any(|c| self.in_scope(c, scope) && c.name == name)
        {
            return Err(anyhow!(
                "an MCP server named `{}` already exists in {:?} scope",
                name,
                scope
            ));
        }
        if self.path_for(scope).is_none() {
            return Err(anyhow!(
                "no config path set for {:?} scope in this host",
                scope
            ));
        }
        let id = self.scope_id(scope, name);
        self.configs.push(McpServerConfig {
            id: id.clone(),
            name: name.to_string(),
            transport,
            enabled: true,
            scope,
        });
        if let Err(e) = self.save_scope(scope) {
            // Roll the in-memory add back so a failed persist doesn't leave a
            // phantom server running until restart.
            self.configs.retain(|c| c.id != id);
            return Err(e);
        }
        let connect = self.test_server(&id).map_err(|e| e.to_string());
        Ok((id, connect))
    }

    /// Pool (user-scope) servers only, with status — the Settings list.
    pub fn list_pool_servers_with_status(&self) -> Vec<McpServerWithStatus> {
        self.list_servers_with_status()
            .into_iter()
            .filter(|s| s.config.scope == McpScope::User)
            .collect()
    }

    /// Raw JSON entry for a pool server (preserves fields Rustic doesn't model).
    pub fn pool_entry(&self, name: &str) -> Option<Value> {
        if let Some(path) = self.user_path.as_deref() {
            if let Ok(text) = std::fs::read_to_string(path) {
                if let Ok(v) = serde_json::from_str::<Value>(&text) {
                    if let Some(e) = v.get("mcpServers").and_then(|s| s.get(name)) {
                        return Some(e.clone());
                    }
                }
            }
        }
        self.configs
            .iter()
            .find(|c| c.scope == McpScope::User && c.name == name)
            .map(|c| transport_to_json(&c.transport))
    }

    /// Add one server entry to the global pool file and reload the pool.
    pub fn add_pool_server(&mut self, name: &str, entry: &Value) -> Result<()> {
        project_files::validate_entry(name, entry)?;
        parse_mcp_json(&json!({ "mcpServers": { name: entry } }).to_string())?;
        let path = self
            .user_path
            .clone()
            .ok_or_else(|| anyhow!("No path set for the global MCP config"))?;
        let mut doc: Value = if path.exists() {
            serde_json::from_str(&std::fs::read_to_string(&path)?)
                .map_err(|e| anyhow!("Global mcp.json is not valid JSON: {}", e))?
        } else {
            json!({ "mcpServers": {} })
        };
        let servers = doc
            .as_object_mut()
            .ok_or_else(|| anyhow!("Global mcp.json must be a JSON object"))?
            .entry("mcpServers")
            .or_insert_with(|| json!({}));
        let servers = servers
            .as_object_mut()
            .ok_or_else(|| anyhow!("\"mcpServers\" must be an object"))?;
        if servers.contains_key(name) {
            return Err(anyhow!("An MCP server named `{}` already exists", name));
        }
        servers.insert(name.to_string(), entry.clone());
        write_json_atomic(&path, &doc)?;
        self.loaded_mtime.remove(&path);
        self.load_scope(McpScope::User, &path)?;
        Ok(())
    }

    /// How server `name` looks from project `root` (entry, enabled, status).
    pub fn project_server_view(&mut self, root: &Path, name: &str) -> Result<McpProjectServerView> {
        let _ = self.load_project_scope_gated(&project_files::mcp_json_path(root));
        let project_entry = project_files::read_project_entry(root, name)?;
        let overridden = project_entry.is_some();
        let entry = project_entry
            .or_else(|| self.pool_entry(name))
            .ok_or_else(|| anyhow!("MCP server not found: {}", name))?;
        let enabled = !project_files::read_disabled(root).contains(name);
        let status = self
            .effective_config_for_project(root, name)
            .map(|c| self.status_of(&c.id))
            .unwrap_or(McpConnectionStatus::Unknown);
        Ok(McpProjectServerView {
            entry,
            enabled,
            overridden,
            status,
        })
    }

    /// Write server `name` into project `root` (or remove it when disabled),
    /// sync `.gemini`/`.codex` and reload the project. Makes no connection.
    /// Rustic's own write is auto-approved only if the file was already
    /// trusted (or absent), so unreviewed third-party entries stay gated.
    pub fn write_project_server(
        &mut self,
        root: &Path,
        name: &str,
        entry: Option<&Value>,
        enabled: bool,
    ) -> Result<McpProjectSaveResult> {
        let mcp_path = project_files::mcp_json_path(root);
        let trusted_before = match std::fs::read_to_string(&mcp_path) {
            Ok(text) => self.is_project_consented(&mcp_path, &sha256_hex(text.as_bytes())),
            Err(_) => true,
        };

        if enabled {
            let entry = match entry {
                Some(e) => e.clone(),
                None => self
                    .pool_entry(name)
                    .ok_or_else(|| anyhow!("MCP server not found: {}", name))?,
            };
            parse_mcp_json(&json!({ "mcpServers": { name: &entry } }).to_string())?;
            project_files::upsert_server(root, name, &entry)?;
            project_files::set_disabled(root, name, false)?;
        } else {
            project_files::remove_server(root, name)?;
            project_files::set_disabled(root, name, true)?;
        }

        if trusted_before {
            if let Ok(text) = std::fs::read_to_string(&mcp_path) {
                self.approve_project_consent(mcp_path.clone(), sha256_hex(text.as_bytes()))?;
            }
        }
        self.loaded_mtime.remove(&mcp_path);
        let consent_required = matches!(
            self.load_project_scope_gated(&mcp_path)?,
            LoadProjectScopeResult::ConsentRequired { .. }
        );

        let result = McpProjectSaveResult {
            enabled,
            connected: false,
            tool_count: 0,
            error: None,
            consent_required,
        };
        if !enabled {
            let override_id = format!("{}{}", project_id_prefix(root), name);
            self.drop_client(&override_id);
        }
        Ok(result)
    }

    /// `write_project_server`, then — when enabled — test the connection.
    /// The test runs off the manager lock, so it never stalls other MCP work.
    pub fn save_project_server(
        manager: &Arc<Mutex<Self>>,
        root: &Path,
        name: &str,
        entry: Option<&Value>,
        enabled: bool,
    ) -> Result<McpProjectSaveResult> {
        let (mut result, target) = {
            let mut mgr = Self::locked(manager);
            let result = mgr.write_project_server(root, name, entry, enabled)?;
            (result, mgr.effective_config_for_project(root, name))
        };
        if !enabled {
            return Ok(result);
        }
        let Some(cfg) = target else {
            result.error = Some(format!("Server `{}` is not active in this project", name));
            return Ok(result);
        };
        match Self::retest_server(manager, &cfg.id) {
            Ok(tools) => {
                result.connected = true;
                result.tool_count = tools.len();
            }
            Err(e) => result.error = Some(e.to_string()),
        }
        Ok(result)
    }

    /// Add `name` to the global pool, write it into each `(root, enabled)`
    /// project, and start connecting it once in the background. Returns the
    /// per-project write outcomes in the order given.
    pub fn add_pool_server_for_projects(
        manager: &Arc<Mutex<Self>>,
        name: &str,
        entry: &Value,
        projects: &[(PathBuf, bool)],
    ) -> Result<Vec<Result<McpProjectSaveResult>>> {
        let (outcomes, jobs) = {
            let mut mgr = Self::locked(manager);
            mgr.add_pool_server(name, entry)?;
            let outcomes: Vec<Result<McpProjectSaveResult>> = projects
                .iter()
                .map(|(root, enabled)| mgr.write_project_server(root, name, None, *enabled))
                .collect();
            let jobs = mgr.claim_connects(&[format!("user-{}", name)]);
            (outcomes, jobs)
        };
        Self::spawn_connects(manager, jobs);
        Ok(outcomes)
    }

    /// Ids worth connecting: every enabled server except project entries
    /// identical to their pool entry — those route to the pool server, so a
    /// second process for them would never be used.
    fn connectable_ids(&self) -> Vec<String> {
        self.configs
            .iter()
            .filter(|c| c.enabled)
            .filter(|c| {
                c.scope != McpScope::Project
                    || !self.configs.iter().any(|p| {
                        p.enabled
                            && p.scope == McpScope::User
                            && p.name == c.name
                            && same_transport(&p.transport, &c.transport)
                    })
            })
            .map(|c| c.id.clone())
            .collect()
    }

    /// Mark `cfg` as connecting and hand back the job to run off the lock.
    fn claim(&mut self, cfg: McpServerConfig) -> ConnectJob {
        self.connect_seq += 1;
        let token = self.connect_seq;
        self.connecting.insert(cfg.id.clone(), token);
        self.notify_status(&cfg.id);
        ConnectJob { cfg, token }
    }

    /// Claim every server in `ids` that is enabled, not connected and not
    /// already connecting. Run the jobs with `spawn_connects`.
    fn claim_connects(&mut self, ids: &[String]) -> Vec<ConnectJob> {
        let targets: Vec<McpServerConfig> = self
            .configs
            .iter()
            .filter(|c| {
                c.enabled
                    && ids.contains(&c.id)
                    && !self.clients.contains_key(&c.id)
                    && !self.connecting.contains_key(&c.id)
            })
            .cloned()
            .collect();
        targets.into_iter().map(|cfg| self.claim(cfg)).collect()
    }

    /// Store the result of a claimed connect. A superseded attempt (the server
    /// was re-tested, removed or reconfigured meanwhile) is discarded.
    fn apply_connect(
        &mut self,
        job: &ConnectJob,
        outcome: ConnectOutcome,
    ) -> std::result::Result<Vec<ToolDef>, String> {
        let id = &job.cfg.id;
        if self.connecting.get(id) != Some(&job.token) {
            if let Ok((client, _)) = outcome {
                release_client(Arc::new(Mutex::new(client)));
            }
            return Err("superseded by a newer connection attempt".into());
        }
        self.connecting.remove(id);
        let result = match outcome {
            Ok((client, tools)) => {
                self.status.insert(
                    id.clone(),
                    McpConnectionStatus::Connected {
                        tool_count: tools.len(),
                    },
                );
                self.read_only_cache
                    .insert(id.clone(), client.read_only_tools().clone());
                self.tool_cache.insert(id.clone(), tools.clone());
                self.tool_cache_at
                    .insert(id.clone(), std::time::Instant::now());
                if let Some(old) = self
                    .clients
                    .insert(id.clone(), Arc::new(Mutex::new(client)))
                {
                    release_client(old);
                }
                Ok(tools)
            }
            Err(error) => {
                self.status.insert(
                    id.clone(),
                    McpConnectionStatus::Failed {
                        error: error.clone(),
                    },
                );
                Err(error)
            }
        };
        self.notify_status(id);
        result
    }

    /// Run one claimed connect on the calling thread (off the manager lock),
    /// store its result and wake anyone waiting on it.
    fn run_connect(
        manager: &Arc<Mutex<Self>>,
        job: &ConnectJob,
    ) -> std::result::Result<Vec<ToolDef>, String> {
        let started = std::time::Instant::now();
        let outcome =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| connect_and_list(&job.cfg)))
                .unwrap_or_else(|_| Err("MCP connect panicked".into()));
        tracing::info!(
            target: "rustic::timing",
            server = %job.cfg.id,
            connected = outcome.is_ok(),
            elapsed_ms = started.elapsed().as_millis() as u64,
            "mcp: server connect"
        );
        Self::finish_connect(manager, job, outcome)
    }

    /// Apply a connect outcome under the lock, then wake waiters.
    fn finish_connect(
        manager: &Arc<Mutex<Self>>,
        job: &ConnectJob,
        outcome: ConnectOutcome,
    ) -> std::result::Result<Vec<ToolDef>, String> {
        let (result, signal) = {
            let mut mgr = Self::locked(manager);
            let result = mgr.apply_connect(job, outcome);
            (result, Arc::clone(&mgr.connect_signal))
        };
        signal.bump();
        result
    }

    /// Run each claimed connect on its own thread, so servers come up in
    /// parallel and none of them holds the manager lock while starting.
    fn spawn_connects(manager: &Arc<Mutex<Self>>, jobs: Vec<ConnectJob>) {
        for job in jobs {
            let shared = Arc::clone(manager);
            let fallback = job.clone();
            let spawned = std::thread::Builder::new()
                .name("mcp-connect".into())
                .spawn(move || {
                    let _ = Self::run_connect(&shared, &job);
                });
            if let Err(e) = spawned {
                let _ = Self::finish_connect(
                    manager,
                    &fallback,
                    Err(format!("could not start the connect thread: {}", e)),
                );
            }
        }
    }

    /// Block until none of `ids` has a connect in flight.
    fn wait_for_connects(manager: &Arc<Mutex<Self>>, ids: &[String]) {
        let signal = Arc::clone(&Self::locked(manager).connect_signal);
        loop {
            let seen = signal.generation();
            let pending = {
                let mgr = Self::locked(manager);
                ids.iter().any(|id| mgr.connecting.contains_key(id))
            };
            if !pending {
                return;
            }
            signal.wait_past(seen);
        }
    }

    /// Start connecting every server that isn't connected yet and re-list
    /// stale tool caches, all on background threads. Returns immediately;
    /// each server reports through the status listener as it settles.
    pub fn connect_all_in_background(manager: &Arc<Mutex<Self>>) {
        let (connects, refreshes) = {
            let mut mgr = Self::locked(manager);
            mgr.claim_background_work()
        };
        Self::spawn_connects(manager, connects);
        Self::spawn_tool_refreshes(manager, refreshes);
    }

    /// Claim what `connect_all_in_background` runs (call under the lock).
    fn claim_background_work(&mut self) -> (Vec<ConnectJob>, Vec<ToolRefreshJob>) {
        self.sweep_broken_clients();
        let ids = self.connectable_ids();
        let connects = self.claim_connects(&ids);
        (connects, self.claim_stale_tools(TOOL_CACHE_TTL))
    }

    /// What the MCP list UI shows: reload the given scope files, start
    /// connecting whatever isn't connected in the background, and return the
    /// rows at once — servers still coming up report `Connecting`.
    pub fn list_and_connect(
        manager: &Arc<Mutex<Self>>,
        user_path: Option<&Path>,
        project_path: Option<&Path>,
        pool_only: bool,
    ) -> Vec<McpServerWithStatus> {
        let (rows, connects, refreshes) = {
            let mut mgr = Self::locked(manager);
            if let Some(p) = user_path {
                let _ = mgr.load_scope(McpScope::User, p);
            }
            if let Some(p) = project_path {
                // F-10 gate: project servers only load once their content is approved.
                let _ = mgr.load_project_scope_gated(p);
            }
            let (connects, refreshes) = mgr.claim_background_work();
            let rows = if pool_only {
                mgr.list_pool_servers_with_status()
            } else {
                mgr.list_servers_with_status()
            };
            (rows, connects, refreshes)
        };
        Self::spawn_connects(manager, connects);
        Self::spawn_tool_refreshes(manager, refreshes);
        rows
    }

    /// Write raw JSON for `scope` at `path`, reload it, and start connecting
    /// the entries that changed in the background. Returns the scope's rows.
    pub fn save_scope_and_connect(
        manager: &Arc<Mutex<Self>>,
        scope: McpScope,
        path: &Path,
        content: &str,
    ) -> Result<Vec<McpServerWithStatus>> {
        let (rows, connects) = {
            let mut mgr = Self::locked(manager);
            match scope {
                McpScope::User => mgr.set_user_path(path.to_path_buf()),
                McpScope::Project => mgr.set_project_path(path.to_path_buf()),
            }
            mgr.save_scope_raw(scope, content)?;
            let connectable = mgr.connectable_ids();
            let ids: Vec<String> = mgr
                .configs
                .iter()
                .filter(|c| mgr.in_scope(c, scope) && connectable.contains(&c.id))
                .map(|c| c.id.clone())
                .collect();
            let connects = mgr.claim_connects(&ids);
            (mgr.list_scope_with_status(scope), connects)
        };
        Self::spawn_connects(manager, connects);
        Ok(rows)
    }

    /// Reconnect one server and list its tools without holding the manager
    /// lock. Supersedes a connect already in flight for the same server.
    pub fn retest_server(manager: &Arc<Mutex<Self>>, id: &str) -> Result<Vec<ToolDef>> {
        let job = {
            let mut mgr = Self::locked(manager);
            let cfg = mgr
                .configs
                .iter()
                .find(|c| c.id == id)
                .cloned()
                .ok_or_else(|| anyhow!("Server not found: {}", id))?;
            mgr.drop_client(id);
            mgr.connecting.remove(id);
            mgr.claim(cfg)
        };
        Self::run_connect(manager, &job).map_err(|e| anyhow!(e))
    }

    /// Re-list tools for connected servers whose cache is older than
    /// `max_age`, inline. Tests only — production re-lists on background threads.
    #[cfg(test)]
    fn refresh_stale_tools(&mut self, max_age: std::time::Duration) {
        for job in self.claim_stale_tools(max_age) {
            let outcome = list_for_refresh(&job.client);
            self.apply_tool_refresh(&job, outcome);
        }
    }

    /// Re-list each claimed server's tools on a thread that never holds the
    /// manager lock while waiting on the server.
    fn spawn_tool_refreshes(manager: &Arc<Mutex<Self>>, jobs: Vec<ToolRefreshJob>) {
        for job in jobs {
            let manager = Arc::clone(manager);
            let spawned = std::thread::Builder::new()
                .name("mcp-tool-refresh".into())
                .spawn(move || {
                    let outcome = list_for_refresh(&job.client);
                    Self::locked(&manager).apply_tool_refresh(&job, outcome);
                });
            if let Err(e) = spawned {
                tracing::warn!(error = %e, "MCP tool-cache refresh thread failed to start");
            }
        }
    }

    /// Claim connected servers whose tool cache is older than `max_age` for a
    /// re-list. Claiming re-stamps them, so concurrent turns don't re-list the
    /// same server and a failing one isn't re-probed every turn.
    fn claim_stale_tools(&mut self, max_age: std::time::Duration) -> Vec<ToolRefreshJob> {
        let now = std::time::Instant::now();
        let jobs: Vec<ToolRefreshJob> = self
            .clients
            .iter()
            .filter(|(id, _)| {
                self.tool_cache_at
                    .get(*id)
                    .map(|t| now.duration_since(*t) > max_age)
                    .unwrap_or(true)
            })
            .map(|(id, client)| ToolRefreshJob {
                id: id.clone(),
                client: std::sync::Arc::clone(client),
            })
            .collect();
        for job in &jobs {
            self.tool_cache_at.insert(job.id.clone(), now);
        }
        jobs
    }

    /// Store the result of a claimed re-list. Ignored when the client was
    /// dropped or reconnected meanwhile — that listing is from a dead session.
    fn apply_tool_refresh(&mut self, job: &ToolRefreshJob, outcome: ToolRefreshOutcome) {
        let still_current = self
            .clients
            .get(&job.id)
            .is_some_and(|c| std::sync::Arc::ptr_eq(c, &job.client));
        if !still_current {
            return;
        }
        match outcome {
            ToolRefreshOutcome::Listed(tools, read_only) => {
                self.status.insert(
                    job.id.clone(),
                    McpConnectionStatus::Connected {
                        tool_count: tools.len(),
                    },
                );
                self.read_only_cache.insert(job.id.clone(), read_only);
                self.tool_cache.insert(job.id.clone(), tools);
                self.notify_status(&job.id);
            }
            ToolRefreshOutcome::Failed(error) => {
                tracing::warn!(
                    server = %job.id,
                    %error,
                    "MCP tool-cache refresh failed; keeping cached tools"
                );
            }
            // Un-stamp so the next turn retries instead of waiting a full TTL.
            ToolRefreshOutcome::Busy => {
                self.tool_cache_at.remove(&job.id);
            }
        }
    }

    fn connect_one(&mut self, cfg: &McpServerConfig) -> (bool, usize, Option<String>) {
        match McpClient::connect(cfg.clone()) {
            Ok(mut client) => match client.list_tools() {
                Ok(tools) => {
                    let count = tools.len();
                    self.status.insert(
                        cfg.id.clone(),
                        McpConnectionStatus::Connected { tool_count: count },
                    );
                    self.read_only_cache
                        .insert(cfg.id.clone(), client.read_only_tools().clone());
                    self.tool_cache.insert(cfg.id.clone(), tools);
                    self.tool_cache_at
                        .insert(cfg.id.clone(), std::time::Instant::now());
                    self.clients.insert(
                        cfg.id.clone(),
                        std::sync::Arc::new(std::sync::Mutex::new(client)),
                    );
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

    /// Config ids a project actually uses: the global pool, overridden by the
    /// project's `.mcp.json` entries, minus names in its disabled list. An
    /// override identical to the pool entry reuses the pool id (one process).
    pub fn effective_ids_for_project(&self, root: &Path) -> Vec<String> {
        let prefix = project_id_prefix(root);
        let empty = BTreeSet::new();
        let disabled = self
            .project_disabled
            .get(&project_key(root))
            .unwrap_or(&empty);
        let mut by_name: std::collections::BTreeMap<&str, &McpServerConfig> = Default::default();
        for c in self
            .configs
            .iter()
            .filter(|c| c.enabled && c.scope == McpScope::User)
        {
            by_name.insert(c.name.as_str(), c);
        }
        for c in self
            .configs
            .iter()
            .filter(|c| c.enabled && c.scope == McpScope::Project && c.id.starts_with(&prefix))
        {
            let same_as_pool = by_name.get(c.name.as_str()).is_some_and(|p| {
                p.scope == McpScope::User
                    && transport_to_json(&p.transport) == transport_to_json(&c.transport)
            });
            if !same_as_pool {
                by_name.insert(c.name.as_str(), c);
            }
        }
        let mut ids: Vec<String> = by_name
            .into_iter()
            .filter(|(name, _)| !disabled.contains(*name))
            .map(|(_, c)| c.id.clone())
            .collect();
        ids.sort();
        ids
    }

    /// Effective config for `name` in project `root`, if enabled there.
    pub fn effective_config_for_project(&self, root: &Path, name: &str) -> Option<McpServerConfig> {
        let ids = self.effective_ids_for_project(root);
        self.configs
            .iter()
            .find(|c| c.name == name && ids.contains(&c.id))
            .cloned()
    }

    /// Ids to route tool calls through: the project's effective set when that
    /// project has been loaded, otherwise `None` (legacy global routing).
    fn routing_ids(&self, root: Option<&Path>) -> Option<Vec<String>> {
        let root = root?;
        if !self.project_disabled.contains_key(&project_key(root)) {
            return None;
        }
        Some(self.effective_ids_for_project(root))
    }

    /// Drop clients that flagged themselves broken so they get reconnected.
    fn sweep_broken_clients(&mut self) {
        let broken: Vec<String> = self
            .clients
            .iter()
            .filter(|(_, c)| c.try_lock().map(|g| g.is_broken()).unwrap_or(false))
            .map(|(id, _)| id.clone())
            .collect();
        for id in broken {
            tracing::warn!(server = %id, "dropping broken MCP client; reconnecting");
            self.clients.remove(&id);
        }
    }

    /// Connect the project's effective servers that aren't connected yet, in
    /// parallel and off the manager lock, and wait for them. A connect another
    /// caller already started (prewarm, the settings list) is joined, not repeated.
    fn connect_project_and_wait(manager: &Arc<Mutex<Self>>, root: &Path) {
        let (ids, jobs) = {
            let mut mgr = Self::locked(manager);
            mgr.sweep_broken_clients();
            let ids = mgr.effective_ids_for_project(root);
            let jobs = mgr.claim_connects(&ids);
            (ids, jobs)
        };
        Self::spawn_connects(manager, jobs);
        Self::wait_for_connects(manager, &ids);
    }

    /// Load both scopes (project scope stays consent-gated) and start
    /// connecting the project's servers ahead of its first turn. Returns
    /// without waiting; the turn joins whatever is still in flight.
    pub fn prewarm_project(manager: &Arc<Mutex<Self>>, user_path: Option<&Path>, root: &Path) {
        let jobs = {
            let mut mgr = Self::locked(manager);
            if let Some(p) = user_path {
                let _ = mgr.load_scope(McpScope::User, p);
            }
            let _ = mgr.load_project_scope_gated(&project_files::mcp_json_path(root));
            mgr.sweep_broken_clients();
            let ids = mgr.effective_ids_for_project(root);
            mgr.claim_connects(&ids)
        };
        Self::spawn_connects(manager, jobs);
    }

    /// Turn-start entry point: connect the project's missing servers and
    /// return its cached tools. Stale caches are re-listed on background
    /// threads that never hold the manager lock, so a refresh can't delay this
    /// turn or the next one; what it finds applies from the following turn.
    pub fn tools_for_turn(
        manager: &std::sync::Arc<std::sync::Mutex<Self>>,
        root: &Path,
    ) -> McpTurnTools {
        Self::tools_for_turn_with_ttl(manager, root, TOOL_CACHE_TTL)
    }

    fn tools_for_turn_with_ttl(
        manager: &std::sync::Arc<std::sync::Mutex<Self>>,
        root: &Path,
        max_age: std::time::Duration,
    ) -> McpTurnTools {
        Self::connect_project_and_wait(manager, root);
        let (turn, jobs) = {
            let mut mgr = Self::locked(manager);
            let turn = McpTurnTools {
                tools: mgr.project_tools(root),
                read_only: mgr.project_read_only_tools(root),
            };
            (turn, mgr.claim_stale_tools(max_age))
        };
        Self::spawn_tool_refreshes(manager, jobs);
        turn
    }

    /// Tools from the project's effective, connected servers (sorted by id).
    pub fn project_tools(&self, root: &Path) -> Vec<ToolDef> {
        let mut tools = Vec::new();
        for id in self.effective_ids_for_project(root) {
            if self.clients.contains_key(&id) {
                if let Some(t) = self.tool_cache.get(&id) {
                    tools.extend(t.iter().cloned());
                }
            }
        }
        tools
    }

    /// Project tools their server annotates `readOnlyHint`, mapped to the id of
    /// the server each call is routed to. The hint is server-supplied: use it
    /// for scheduling only, never for permission decisions.
    pub fn project_read_only_tools(&self, root: &Path) -> HashMap<String, String> {
        let mut out = HashMap::new();
        for tool in self.project_tools(root) {
            let Some(id) = self.server_id_for_tool(Some(root), &tool.name) else {
                continue;
            };
            let hinted = self
                .read_only_cache
                .get(&id)
                .is_some_and(|names| names.contains(&tool.name));
            if hinted {
                out.insert(tool.name, id);
            }
        }
        out
    }

    /// Disconnect and forget a single client (e.g. before a re-test).
    fn drop_client(&mut self, id: &str) {
        if let Some(client) = self.clients.remove(id) {
            release_client(client);
        }
        self.tool_cache.remove(id);
        self.read_only_cache.remove(id);
        self.tool_cache_at.remove(id);
    }

    /// Tool definitions from all connected servers, sorted by server id for
    /// prompt-cache stability. Served from the connect-time cache — zero
    /// IPC/network cost per turn.
    pub fn all_tools(&mut self) -> Vec<ToolDef> {
        let mut ids: Vec<String> = self.clients.keys().cloned().collect();
        ids.sort();
        let mut tools = Vec::new();
        for id in ids {
            if let Some(t) = self.tool_cache.get(&id) {
                tools.extend(t.iter().cloned());
            }
        }
        tools
    }

    /// Tools advertised by a single connected server, looked up by config id.
    /// Used by the settings UI to show what a server provides when the user
    /// expands its row. Queries the server live — off the manager lock, so a
    /// slow or busy server only delays this call — and refreshes the dispatch
    /// cache. Errors if the server isn't connected.
    pub fn list_server_tools(manager: &Arc<Mutex<Self>>, id: &str) -> Result<Vec<ToolDef>> {
        let client = Self::locked(manager)
            .clients
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow!("MCP server '{}' is not connected", id))?;
        let (tools, read_only) = {
            let mut guard = client
                .lock()
                .map_err(|_| anyhow!("MCP client mutex poisoned for '{}'", id))?;
            let tools = guard.list_tools()?;
            (tools, guard.read_only_tools().clone())
        };
        let mut mgr = Self::locked(manager);
        let still_current = mgr.clients.get(id).is_some_and(|c| Arc::ptr_eq(c, &client));
        if still_current {
            mgr.read_only_cache.insert(id.to_string(), read_only);
            mgr.tool_cache.insert(id.to_string(), tools.clone());
            mgr.tool_cache_at
                .insert(id.to_string(), std::time::Instant::now());
        }
        Ok(tools)
    }

    /// Resolve the server (config) id whose cached tool list advertises
    /// `name`. Sorted iteration keeps resolution deterministic when two
    /// servers advertise the same tool name. With a loaded project `root`,
    /// only that project's effective servers are considered.
    fn server_id_for_tool(&self, root: Option<&Path>, name: &str) -> Option<String> {
        let allowed = self.routing_ids(root);
        let mut ids: Vec<&String> = self
            .tool_cache
            .keys()
            .filter(|id| allowed.as_ref().is_none_or(|a| a.contains(id)))
            .collect();
        ids.sort();
        for id in ids {
            if self.tool_cache[id].iter().any(|t| t.name == name) {
                return Some(id.clone());
            }
        }
        None
    }

    /// Resolve the connected client that provides `name`, using the cached
    /// tool lists (no IPC). Returns a clone of the per-client handle so the
    /// caller can invoke the tool WITHOUT holding the manager lock — this is
    /// what lets calls to different servers run in parallel.
    pub fn client_providing(
        &self,
        root: Option<&Path>,
        name: &str,
    ) -> Option<std::sync::Arc<std::sync::Mutex<McpClient>>> {
        let id = self.server_id_for_tool(root, name)?;
        self.clients.get(&id).map(std::sync::Arc::clone)
    }

    /// 5.4/8.1: unconditionally drop the client that provides `name`.
    ///
    /// Called after an executor-side timeout abandoned a call that may STILL
    /// be blocked inside the client holding its per-client mutex (legacy SSE
    /// has no read timeout). The stream is desynced — the late reply would
    /// otherwise be paired with the NEXT request — so the handle is removed
    /// without touching its mutex. The abandoned call's Arc keeps the client
    /// alive until it returns, at which point Drop disconnects it; meanwhile
    /// `connect_all` (next turn) or `reconnect_for_tool` (same turn) spawns a
    /// fresh connection.
    pub fn drop_client_for_tool(&mut self, root: Option<&Path>, name: &str) {
        let Some(id) = self.server_id_for_tool(root, name) else {
            return;
        };
        if self.clients.remove(&id).is_some() {
            tracing::warn!(
                server = %id,
                tool = %name,
                "MCP call timed out — dropping the (possibly desynced) client; it will be reconnected"
            );
            self.status.insert(
                id.clone(),
                McpConnectionStatus::Failed {
                    error: "request timed out; connection dropped, will reconnect".into(),
                },
            );
            self.notify_status(&id);
        }
    }

    /// Drop the client that provides `name` iff it reports itself broken
    /// (in-client request timeout or stream death). try_lock so this never
    /// blocks on a mutex held by a still-running call.
    pub fn drop_client_if_broken(&mut self, root: Option<&Path>, name: &str) {
        let Some(id) = self.server_id_for_tool(root, name) else {
            return;
        };
        let broken = self
            .clients
            .get(&id)
            .and_then(|c| c.try_lock().ok().map(|g| g.is_broken()))
            .unwrap_or(false);
        if broken {
            self.clients.remove(&id);
            tracing::warn!(
                server = %id,
                tool = %name,
                "MCP client marked itself broken — dropped; it will be reconnected"
            );
            self.status.insert(
                id.clone(),
                McpConnectionStatus::Failed {
                    error: "connection broken (timeout or stream desync); will reconnect".into(),
                },
            );
            self.notify_status(&id);
        }
    }

    /// Same-turn lazy reconnect: if the server providing `name` has been
    /// dropped after a broken/timed-out call, spawn a fresh connection and
    /// return its client handle. No-op (returns the live handle) when the
    /// client is still connected.
    pub fn reconnect_for_tool(
        &mut self,
        root: Option<&Path>,
        name: &str,
    ) -> Option<std::sync::Arc<std::sync::Mutex<McpClient>>> {
        let id = self.server_id_for_tool(root, name)?;
        if let Some(c) = self.clients.get(&id) {
            return Some(std::sync::Arc::clone(c));
        }
        let cfg = self
            .configs
            .iter()
            .find(|c| c.id == id && c.enabled)?
            .clone();
        let (ok, _, _) = self.connect_one(&cfg);
        if ok {
            self.clients.get(&id).map(std::sync::Arc::clone)
        } else {
            None
        }
    }

    /// Call a tool on the appropriate server. Convenience wrapper used by
    /// hosts; the executor resolves via `client_providing` itself so the
    /// manager lock isn't held for the duration of the call.
    pub fn call_tool(&mut self, name: &str, arguments: Value) -> Result<Value> {
        let client = self
            .client_providing(None, name)
            .ok_or_else(|| anyhow!("No MCP server provides tool: {}", name))?;
        let mut guard = client
            .lock()
            .map_err(|_| anyhow!("MCP client mutex poisoned for tool '{}'", name))?;
        guard.call_tool(name, arguments)
    }

    pub fn disconnect_all(&mut self) {
        for (_, client) in self.clients.drain() {
            if let Ok(mut c) = client.lock() {
                c.disconnect();
            }
        }
        self.tool_cache.clear();
    }
}

fn scope_prefix(scope: McpScope) -> &'static str {
    match scope {
        McpScope::User => "user",
        McpScope::Project => "project",
    }
}

/// Normalised key for a project root (no trailing separator; case-insensitive on Windows).
pub fn project_key(root: &Path) -> String {
    let s = root.to_string_lossy().replace('\\', "/");
    let s = s.trim_end_matches('/');
    if cfg!(windows) {
        s.to_lowercase()
    } else {
        s.to_string()
    }
}

/// Config-id prefix for the project-scope servers of `root`, so two projects
/// with a same-named server never share (or clobber) each other's entry.
fn project_id_prefix(root: &Path) -> String {
    format!(
        "project-{}-",
        &sha256_hex(project_key(root).as_bytes())[..8]
    )
}

/// Parse `.mcp.json` content into a list of (name, transport) pairs.
///
/// The accepted shape matches Claude Code:
/// - `command` + `args` + optional `env` → stdio transport
/// - `url` + optional `headers` → sse transport
fn parse_mcp_json(text: &str) -> Result<Vec<(String, McpTransport)>> {
    let json: Value = serde_json::from_str(text).map_err(|e| anyhow!("Invalid JSON: {}", e))?;

    let servers_map = json
        .get("mcpServers")
        .and_then(|v| v.as_object())
        .ok_or_else(|| anyhow!("Missing or invalid \"mcpServers\" object"))?;

    let mut out = Vec::new();
    for (name, def) in servers_map {
        let transport = if let Some(url) = entry_remote_url(def) {
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
                .ok_or_else(|| {
                    anyhow!(
                        "Server \"{}\" is missing both \"command\" and \"url\"",
                        name
                    )
                })?
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

/// Remote URL of an entry, accepting the `serverUrl` / `httpUrl` spellings
/// used by Google (Stitch) and Gemini CLI docs alongside the standard `url`.
fn entry_remote_url(def: &Value) -> Option<&str> {
    ["url", "serverUrl", "httpUrl"]
        .iter()
        .find_map(|k| def.get(*k).and_then(|v| v.as_str()))
}

fn is_server_entry(v: &Value) -> bool {
    v.get("command").and_then(|c| c.as_str()).is_some() || entry_remote_url(v).is_some()
}

/// Normalize MCP config input into validated `(name, entry)` pairs.
///
/// Accepts the standard `{"mcpServers": {...}}` document, a bare
/// `{"<name>": {...}}` map, or a single entry (`command`/`url`) paired with
/// `name`. A JSON string holding any of these is decoded first, since tool
/// calls often deliver nested objects string-encoded.
pub fn normalize_server_input(input: &Value, name: Option<&str>) -> Result<Vec<(String, Value)>> {
    let decoded;
    let input = match input {
        Value::String(s) => {
            decoded = serde_json::from_str::<Value>(s.trim())
                .map_err(|e| anyhow!("Invalid JSON: {}", e))?;
            &decoded
        }
        v => v,
    };
    let obj = input
        .as_object()
        .ok_or_else(|| anyhow!("MCP config must be a JSON object"))?;

    let mut pairs: Vec<(String, Value)> = if let Some(servers) = obj.get("mcpServers") {
        servers
            .as_object()
            .ok_or_else(|| anyhow!("\"mcpServers\" must be an object"))?
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    } else if is_server_entry(input) {
        let n = name
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .ok_or_else(|| anyhow!("A server name is required for a single entry"))?;
        vec![(n.to_string(), input.clone())]
    } else if !obj.is_empty() && obj.values().all(is_server_entry) {
        obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    } else {
        return Err(anyhow!(
            "Expected {{\"mcpServers\": {{\"<name>\": {{\"command\": ..., \"args\": [...]}}}}}} \
             or an entry with \"command\" (stdio) or \"url\" (http)"
        ));
    };

    if pairs.is_empty() {
        return Err(anyhow!("\"mcpServers\" has no servers"));
    }
    if let Some(n) = name.map(str::trim).filter(|n| !n.is_empty()) {
        if pairs.len() > 1 {
            pairs.retain(|(k, _)| k == n);
            if pairs.is_empty() {
                return Err(anyhow!("No server named \"{}\" in \"mcpServers\"", n));
            }
        }
    }
    for (n, entry) in pairs.iter_mut() {
        if !entry.is_object() {
            return Err(anyhow!("Server \"{}\" must be a JSON object", n));
        }
        if let Some(obj) = entry.as_object_mut() {
            if !obj.contains_key("url") {
                for alias in ["serverUrl", "httpUrl"] {
                    if let Some(u) = obj.remove(alias) {
                        obj.insert("url".into(), u);
                        break;
                    }
                }
            }
        }
        parse_mcp_json(&json!({ "mcpServers": { n.as_str(): &*entry } }).to_string())?;
    }
    Ok(pairs)
}

/// Parse a single validated server entry into its transport.
pub fn entry_transport(name: &str, entry: &Value) -> Result<McpTransport> {
    parse_mcp_json(&json!({ "mcpServers": { name: entry } }).to_string())?
        .into_iter()
        .next()
        .map(|(_, t)| t)
        .ok_or_else(|| anyhow!("Server \"{}\" could not be parsed", name))
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

#[cfg(test)]
mod normalize_input_tests {
    use super::*;

    #[test]
    fn standard_document_without_type_or_name() {
        let v = json!({ "mcpServers": { "whatsapp": {
            "command": "C:\\uv.exe",
            "args": ["--directory", "D:\\srv", "run", "main.py"]
        }}});
        let pairs = normalize_server_input(&v, None).unwrap();
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].0, "whatsapp");
        match entry_transport(&pairs[0].0, &pairs[0].1).unwrap() {
            McpTransport::Stdio { command, args, .. } => {
                assert_eq!(command, "C:\\uv.exe");
                assert_eq!(args.len(), 4);
            }
            _ => panic!("expected stdio"),
        }
    }

    #[test]
    fn string_encoded_document_is_decoded() {
        let s = Value::String(r#"{"mcpServers":{"a":{"command":"a-bin"}}}"#.into());
        let pairs = normalize_server_input(&s, None).unwrap();
        assert_eq!(pairs[0].0, "a");
    }

    #[test]
    fn bare_entry_needs_name() {
        let v = json!({ "command": "npx" });
        assert!(normalize_server_input(&v, None).is_err());
        let pairs = normalize_server_input(&v, Some("x")).unwrap();
        assert_eq!(pairs[0].0, "x");
    }

    #[test]
    fn bare_name_map_and_url_entry() {
        let v = json!({ "remote": { "url": "https://x/mcp" } });
        let pairs = normalize_server_input(&v, None).unwrap();
        assert!(matches!(
            entry_transport(&pairs[0].0, &pairs[0].1).unwrap(),
            McpTransport::Sse { .. }
        ));
    }

    #[test]
    fn name_picks_one_of_many() {
        let v = json!({ "mcpServers": { "a": { "command": "a" }, "b": { "command": "b" } } });
        assert_eq!(normalize_server_input(&v, None).unwrap().len(), 2);
        let pairs = normalize_server_input(&v, Some("b")).unwrap();
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].0, "b");
        assert!(normalize_server_input(&v, Some("zz")).is_err());
    }

    #[test]
    fn entry_missing_command_and_url_fails() {
        let v = json!({ "mcpServers": { "a": { "args": [] } } });
        assert!(normalize_server_input(&v, None).is_err());
    }
}

#[cfg(test)]
mod project_scope_tests {
    use super::*;

    /// Scratch dir holding a pool file and one project root.
    fn setup(pool: &str) -> (PathBuf, PathBuf, McpManager) {
        let base = std::env::temp_dir().join(format!("rustic-mcp-eff-{}", uuid::Uuid::new_v4()));
        let root = base.join("proj");
        std::fs::create_dir_all(&root).unwrap();
        let pool_path = base.join("mcp.json");
        std::fs::write(&pool_path, pool).unwrap();
        let mut mgr = McpManager::new();
        mgr.load_scope(McpScope::User, &pool_path).unwrap();
        (base, root, mgr)
    }

    /// Approve and load the project's current `.mcp.json`.
    fn load_trusted(mgr: &mut McpManager, root: &Path) {
        let path = project_files::mcp_json_path(root);
        if let Ok(text) = std::fs::read_to_string(&path) {
            mgr.approve_project_consent(path.clone(), sha256_hex(text.as_bytes()))
                .unwrap();
        }
        mgr.load_project_scope_gated(&path).unwrap();
    }

    const POOL: &str = r#"{"mcpServers":{"a":{"command":"a-bin"},"b":{"command":"b-bin"}}}"#;

    #[test]
    fn identical_project_entry_reuses_pool_id_and_override_wins() {
        let (base, root, mut mgr) = setup(POOL);
        project_files::upsert_server(&root, "a", &json!({"command": "a-bin"})).unwrap();
        project_files::upsert_server(&root, "b", &json!({"command": "b-bin", "env": {"K": "p"}}))
            .unwrap();
        load_trusted(&mut mgr, &root);
        let ids = mgr.effective_ids_for_project(&root);
        let prefix = project_id_prefix(&root);
        assert_eq!(ids, vec![format!("{prefix}b"), "user-a".to_string()]);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn disabled_servers_are_excluded_and_other_projects_unaffected() {
        let (base, root, mut mgr) = setup(POOL);
        let other = base.join("other");
        std::fs::create_dir_all(&other).unwrap();
        project_files::set_disabled(&root, "a", true).unwrap();
        load_trusted(&mut mgr, &root);
        load_trusted(&mut mgr, &other);
        assert_eq!(
            mgr.effective_ids_for_project(&root),
            vec!["user-b".to_string()]
        );
        assert_eq!(
            mgr.effective_ids_for_project(&other),
            vec!["user-a".to_string(), "user-b".to_string()]
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn loading_one_project_keeps_another_projects_overrides() {
        let (base, root, mut mgr) = setup(POOL);
        let other = base.join("other");
        std::fs::create_dir_all(&other).unwrap();
        project_files::upsert_server(&root, "a", &json!({"command": "a-bin", "args": ["x"]}))
            .unwrap();
        load_trusted(&mut mgr, &root);
        load_trusted(&mut mgr, &other);
        let prefix = project_id_prefix(&root);
        assert!(mgr
            .effective_ids_for_project(&root)
            .contains(&format!("{prefix}a")));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn unapproved_project_file_does_not_load_overrides() {
        let (base, root, mut mgr) = setup(POOL);
        project_files::upsert_server(&root, "a", &json!({"command": "evil"})).unwrap();
        let res = mgr
            .load_project_scope_gated(&project_files::mcp_json_path(&root))
            .unwrap();
        assert!(matches!(
            res,
            LoadProjectScopeResult::ConsentRequired { .. }
        ));
        assert_eq!(
            mgr.effective_ids_for_project(&root),
            vec!["user-a".to_string(), "user-b".to_string()]
        );
        let _ = std::fs::remove_dir_all(&base);
    }
}

#[cfg(test)]
mod turn_tools_tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    /// Stdio MCP server advertising one tool whose name is the content of the
    /// file passed as its first argument.
    #[cfg(unix)]
    const FAKE_SERVER: &str = r#"while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"capabilities":{}}}\n' "$id" ;;
    *'"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"%s"}]}}\n' "$id" "$(cat "$1")" ;;
  esac
done
"#;

    /// Scratch project whose pool has the fake server; returns (base, root, tool-name file, manager).
    #[cfg(unix)]
    fn fake_setup(tool: &str) -> (PathBuf, PathBuf, PathBuf, McpManager) {
        let base = std::env::temp_dir().join(format!("rustic-mcp-turn-{}", uuid::Uuid::new_v4()));
        let root = base.join("proj");
        std::fs::create_dir_all(&root).unwrap();
        let script = base.join("server.sh");
        std::fs::write(&script, FAKE_SERVER).unwrap();
        let tool_file = base.join("tool");
        std::fs::write(&tool_file, tool).unwrap();
        let pool = json!({"mcpServers": {"fake": {"command": "sh", "args": [script, tool_file]}}});
        let pool_path = base.join("mcp.json");
        std::fs::write(&pool_path, pool.to_string()).unwrap();
        let mut mgr = McpManager::new();
        mgr.load_scope(McpScope::User, &pool_path).unwrap();
        (base, root, tool_file, mgr)
    }

    fn names(tools: &[ToolDef]) -> Vec<&str> {
        tools.iter().map(|t| t.name.as_str()).collect()
    }

    #[cfg(unix)]
    #[test]
    fn turn_start_serves_cache_and_refreshes_in_background() {
        let (base, root, tool_file, mgr) = fake_setup("alpha");
        let mgr = Arc::new(Mutex::new(mgr));
        let first = McpManager::tools_for_turn(&mgr, &root);
        assert_eq!(names(&first.tools), vec!["alpha"]);

        // The server changes its tools; a zero TTL makes the cache stale.
        std::fs::write(&tool_file, "beta").unwrap();
        std::thread::sleep(Duration::from_millis(5));
        let second = McpManager::tools_for_turn_with_ttl(&mgr, &root, Duration::ZERO);
        assert_eq!(
            names(&second.tools),
            vec!["alpha"],
            "turn start must not wait for the re-list"
        );

        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let tools = mgr.lock().unwrap().project_tools(&root);
            if names(&tools) == vec!["beta"] {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "background refresh never landed"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        mgr.lock().unwrap().disconnect_all();
        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    #[test]
    fn busy_client_keeps_cache_and_is_retried_next_turn() {
        let (base, root, tool_file, mgr) = fake_setup("alpha");
        let shared = Arc::new(Mutex::new(mgr));
        McpManager::connect_project_and_wait(&shared, &root);
        let mut mgr = shared.lock().unwrap();
        std::fs::write(&tool_file, "beta").unwrap();
        std::thread::sleep(Duration::from_millis(5));

        let client = Arc::clone(&mgr.clients["user-fake"]);
        let in_flight_call = client.lock().unwrap();
        mgr.refresh_stale_tools(Duration::ZERO);
        assert_eq!(names(&mgr.project_tools(&root)), vec!["alpha"]);
        assert!(
            !mgr.tool_cache_at.contains_key("user-fake"),
            "a skipped refresh must not count as done"
        );
        drop(in_flight_call);

        mgr.refresh_stale_tools(Duration::ZERO);
        assert_eq!(names(&mgr.project_tools(&root)), vec!["beta"]);
        mgr.disconnect_all();
        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    #[test]
    fn refresh_of_a_replaced_client_is_discarded() {
        let (base, root, _tool_file, mgr) = fake_setup("alpha");
        let shared = Arc::new(Mutex::new(mgr));
        McpManager::connect_project_and_wait(&shared, &root);
        let jobs = shared.lock().unwrap().claim_stale_tools(Duration::ZERO);
        assert_eq!(jobs.len(), 1);

        // Reconnect while the claimed re-list is still "in flight".
        shared.lock().unwrap().drop_client("user-fake");
        McpManager::connect_project_and_wait(&shared, &root);
        let mut mgr = shared.lock().unwrap();
        let stale_listing = ToolDef {
            name: "from-dead-session".into(),
            description: String::new(),
            parameters: json!({}),
        };
        mgr.apply_tool_refresh(
            &jobs[0],
            ToolRefreshOutcome::Listed(vec![stale_listing], Default::default()),
        );
        assert_eq!(names(&mgr.project_tools(&root)), vec!["alpha"]);
        mgr.disconnect_all();
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn prewarm_does_not_start_unapproved_project_servers() {
        let base = std::env::temp_dir().join(format!("rustic-mcp-warm-{}", uuid::Uuid::new_v4()));
        let root = base.join("proj");
        std::fs::create_dir_all(&root).unwrap();
        let pool_path = base.join("mcp.json");
        std::fs::write(&pool_path, r#"{"mcpServers":{}}"#).unwrap();
        let marker = base.join("spawned");
        project_files::upsert_server(
            &root,
            "evil",
            &json!({"command": "sh", "args": ["-c", format!("touch '{}'", marker.display())]}),
        )
        .unwrap();

        let shared = Arc::new(Mutex::new(McpManager::new()));
        McpManager::prewarm_project(&shared, Some(&pool_path), &root);
        let mgr = shared.lock().unwrap();
        assert!(mgr.effective_ids_for_project(&root).is_empty());
        assert!(mgr.clients.is_empty());
        assert!(
            mgr.connecting.is_empty(),
            "unapproved project server was claimed for a connect"
        );
        assert!(!marker.exists(), "unapproved project server was spawned");
        let _ = std::fs::remove_dir_all(&base);
    }
}

#[cfg(test)]
#[cfg(unix)]
mod background_connect_tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    /// Stdio MCP server that waits for the gate file ($1) to exist before
    /// answering `initialize`, then advertises one tool named $2.
    const GATED_SERVER: &str = r#"while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"initialize"'*)
      while [ ! -e "$1" ]; do sleep 0.02; done
      printf '{"jsonrpc":"2.0","id":%s,"result":{"capabilities":{}}}\n' "$id" ;;
    *'"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"%s"}]}}\n' "$id" "$2" ;;
  esac
done
"#;

    struct Fixture {
        base: PathBuf,
        pool_path: PathBuf,
        script: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let base = std::env::temp_dir().join(format!("rustic-mcp-bg-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&base).unwrap();
            let script = base.join("server.sh");
            std::fs::write(&script, GATED_SERVER).unwrap();
            Self {
                pool_path: base.join("mcp.json"),
                script,
                base,
            }
        }

        /// Gate file a server named `name` waits on before finishing its handshake.
        fn gate(&self, name: &str) -> PathBuf {
            self.base.join(format!("gate-{name}"))
        }

        /// Entry for a gated server advertising a tool called `tool`.
        fn entry(&self, name: &str, tool: &str) -> Value {
            json!({"command": "sh", "args": [self.script, self.gate(name), tool]})
        }

        /// Let server `name` finish its handshake.
        fn open(&self, name: &str) {
            std::fs::write(self.gate(name), "").unwrap();
        }

        /// Write the pool file with `servers` and return its text.
        fn write_pool(&self, servers: Value) -> String {
            let text = json!({ "mcpServers": servers }).to_string();
            std::fs::write(&self.pool_path, &text).unwrap();
            text
        }

        fn list(&self, mgr: &Arc<Mutex<McpManager>>) -> Vec<McpServerWithStatus> {
            McpManager::list_and_connect(mgr, Some(&self.pool_path), None, true)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.base);
        }
    }

    fn status_of(rows: &[McpServerWithStatus], id: &str) -> McpConnectionStatus {
        rows.iter()
            .find(|r| r.config.id == id)
            .map(|r| r.status.clone())
            .unwrap_or_else(|| panic!("no row for {id}"))
    }

    /// Poll until `cond` holds, failing the test after 10 s.
    fn wait_until(what: &str, mut cond: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !cond() {
            assert!(Instant::now() < deadline, "timed out waiting for: {what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn list_returns_rows_at_once_and_servers_settle_independently() {
        let fx = Fixture::new();
        fx.write_pool(json!({
            "slow": fx.entry("slow", "slow-tool"),
            "fast": fx.entry("fast", "fast-tool"),
            "broken": {"command": "/nonexistent/rustic-mcp-test-binary"},
        }));
        fx.open("fast");
        let mgr = Arc::new(Mutex::new(McpManager::new()));
        let events: Arc<Mutex<Vec<(String, McpConnectionStatus)>>> = Arc::default();
        let seqs: Arc<Mutex<Vec<u64>>> = Arc::default();
        let sink = Arc::clone(&events);
        let seq_sink = Arc::clone(&seqs);
        mgr.lock()
            .unwrap()
            .set_status_listener(Arc::new(move |id, status, seq| {
                sink.lock().unwrap().push((id.to_string(), status.clone()));
                seq_sink.lock().unwrap().push(seq);
            }));

        let started = Instant::now();
        let rows = fx.list(&mgr);
        // The snapshot is stamped no older than the pushes its own claims made.
        let claimed = seqs.lock().unwrap().iter().copied().take(3).max().unwrap();
        assert!(rows.iter().all(|r| r.status_seq >= claimed));
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "the list waited on a server that never answers"
        );
        assert_eq!(rows.len(), 3, "every configured server is listed up front");
        assert_eq!(
            status_of(&rows, "user-slow"),
            McpConnectionStatus::Connecting
        );

        // The fast and broken servers settle while `slow` is still held shut.
        wait_until("fast connects and broken fails", || {
            let rows = mgr.lock().unwrap().list_pool_servers_with_status();
            matches!(
                status_of(&rows, "user-fast"),
                McpConnectionStatus::Connected { tool_count: 1 }
            ) && matches!(
                status_of(&rows, "user-broken"),
                McpConnectionStatus::Failed { .. }
            )
        });
        let rows = mgr.lock().unwrap().list_pool_servers_with_status();
        assert_eq!(
            status_of(&rows, "user-slow"),
            McpConnectionStatus::Connecting
        );

        // A second list while `slow` is in flight must not start a second attempt.
        let token = mgr.lock().unwrap().connecting["user-slow"];
        fx.list(&mgr);
        assert_eq!(mgr.lock().unwrap().connecting["user-slow"], token);

        fx.open("slow");
        wait_until("slow connects", || {
            let rows = mgr.lock().unwrap().list_pool_servers_with_status();
            matches!(
                status_of(&rows, "user-slow"),
                McpConnectionStatus::Connected { .. }
            )
        });
        let seen = events.lock().unwrap().clone();
        for id in ["user-slow", "user-fast", "user-broken"] {
            assert!(
                seen.contains(&(id.to_string(), McpConnectionStatus::Connecting)),
                "no connecting event for {id}"
            );
        }
        assert!(seen.contains(&(
            "user-slow".to_string(),
            McpConnectionStatus::Connected { tool_count: 1 }
        )));
        let seqs = seqs.lock().unwrap().clone();
        assert!(
            seqs.windows(2).all(|w| w[0] < w[1]),
            "status pushes must carry strictly increasing sequence numbers"
        );
        let last_row_seq = mgr.lock().unwrap().list_pool_servers_with_status()[0].status_seq;
        assert_eq!(Some(&last_row_seq), seqs.last());
        mgr.lock().unwrap().disconnect_all();
    }

    #[test]
    fn saving_the_pool_only_reconnects_what_changed() {
        let fx = Fixture::new();
        fx.open("keep");
        fx.open("edit");
        fx.open("added");
        fx.write_pool(json!({
            "keep": fx.entry("keep", "keep-v1"),
            "edit": fx.entry("edit", "edit-v1"),
        }));
        let mgr = Arc::new(Mutex::new(McpManager::new()));
        fx.list(&mgr);
        wait_until("both servers connect", || {
            mgr.lock().unwrap().clients.len() == 2
        });
        let kept = Arc::clone(&mgr.lock().unwrap().clients["user-keep"]);

        let next = json!({ "mcpServers": {
            "keep": fx.entry("keep", "keep-v1"),
            "edit": fx.entry("edit", "edit-v2"),
            "added": fx.entry("added", "added-v1"),
        }})
        .to_string();
        let rows =
            McpManager::save_scope_and_connect(&mgr, McpScope::User, &fx.pool_path, &next).unwrap();
        assert!(matches!(
            status_of(&rows, "user-keep"),
            McpConnectionStatus::Connected { .. }
        ));
        assert_eq!(
            status_of(&rows, "user-edit"),
            McpConnectionStatus::Connecting
        );
        assert_eq!(
            status_of(&rows, "user-added"),
            McpConnectionStatus::Connecting
        );
        wait_until("changed servers reconnect", || {
            mgr.lock().unwrap().clients.len() == 3
        });
        let m = mgr.lock().unwrap();
        assert!(
            Arc::ptr_eq(&kept, &m.clients["user-keep"]),
            "an unchanged server was reconnected"
        );
        let tools: Vec<&str> = m.tool_cache["user-edit"]
            .iter()
            .map(|t| t.name.as_str())
            .collect();
        assert_eq!(tools, vec!["edit-v2"]);
        drop(m);
        mgr.lock().unwrap().disconnect_all();
    }

    #[test]
    fn adding_a_server_keeps_the_pool_connected_and_connects_once() {
        let fx = Fixture::new();
        fx.open("old");
        fx.write_pool(json!({ "old": fx.entry("old", "old-tool") }));
        let roots: Vec<(PathBuf, bool)> = (0..3)
            .map(|i| {
                let root = fx.base.join(format!("proj{i}"));
                std::fs::create_dir_all(&root).unwrap();
                (root, i != 2)
            })
            .collect();
        let mgr = Arc::new(Mutex::new(McpManager::new()));
        fx.list(&mgr);
        wait_until("old connects", || mgr.lock().unwrap().clients.len() == 1);
        let old = Arc::clone(&mgr.lock().unwrap().clients["user-old"]);

        let started = Instant::now();
        let outcomes = McpManager::add_pool_server_for_projects(
            &mgr,
            "new",
            &fx.entry("new", "new-tool"),
            &roots,
        )
        .unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "adding waited on the new server's handshake"
        );
        assert_eq!(outcomes.len(), 3);
        assert!(outcomes.iter().all(|o| o.is_ok()));
        assert!(project_files::read_disabled(&roots[2].0).contains("new"));
        assert!(project_files::read_project_entry(&roots[0].0, "new")
            .unwrap()
            .is_some());

        // The follow-up list must not spawn a copy per project.
        fx.list(&mgr);
        {
            let m = mgr.lock().unwrap();
            assert!(Arc::ptr_eq(&old, &m.clients["user-old"]));
            let in_flight: Vec<&String> = m.connecting.keys().collect();
            assert_eq!(in_flight, vec!["user-new"]);
        }
        fx.open("new");
        wait_until("new connects", || mgr.lock().unwrap().clients.len() == 2);
        assert!(mgr.lock().unwrap().connecting.is_empty());
        mgr.lock().unwrap().disconnect_all();
    }

    #[test]
    fn turn_start_joins_a_connect_already_in_flight() {
        let fx = Fixture::new();
        fx.write_pool(json!({ "srv": fx.entry("srv", "srv-tool") }));
        let root = fx.base.join("proj");
        std::fs::create_dir_all(&root).unwrap();
        let mgr = Arc::new(Mutex::new(McpManager::new()));
        McpManager::prewarm_project(&mgr, Some(&fx.pool_path), &root);
        let token = mgr.lock().unwrap().connecting["user-srv"];

        let turn_mgr = Arc::clone(&mgr);
        let turn_root = root.clone();
        let turn = std::thread::spawn(move || McpManager::tools_for_turn(&turn_mgr, &turn_root));
        std::thread::sleep(Duration::from_millis(150));
        assert!(!turn.is_finished(), "the turn did not wait for the connect");
        // The manager stays usable while the turn waits.
        assert_eq!(mgr.lock().unwrap().connecting["user-srv"], token);

        fx.open("srv");
        let tools = turn.join().unwrap().tools;
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["srv-tool"]);
        mgr.lock().unwrap().disconnect_all();
    }

    #[test]
    fn manager_work_does_not_wait_on_a_client_busy_with_a_call() {
        let fx = Fixture::new();
        fx.open("srv");
        let text = fx.write_pool(json!({ "srv": fx.entry("srv", "srv-tool") }));
        let mgr = Arc::new(Mutex::new(McpManager::new()));
        fx.list(&mgr);
        wait_until("srv connects", || mgr.lock().unwrap().clients.len() == 1);

        // Hold the per-client mutex the way an in-flight tool call does.
        let client = Arc::clone(&mgr.lock().unwrap().clients["user-srv"]);
        let in_flight_call = client.lock().unwrap();

        let worker_mgr = Arc::clone(&mgr);
        let pool_path = fx.pool_path.clone();
        let edited = text.replace("srv-tool", "srv-tool-2");
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let saved = McpManager::save_scope_and_connect(
                &worker_mgr,
                McpScope::User,
                &pool_path,
                &edited,
            );
            let removed = worker_mgr.lock().unwrap().remove_server("user-srv");
            let _ = tx.send(saved.is_ok() && removed.is_ok());
        });
        let finished = rx.recv_timeout(Duration::from_secs(5));
        drop(in_flight_call);
        assert_eq!(
            finished,
            Ok(true),
            "save/remove blocked behind a busy client"
        );
        mgr.lock().unwrap().disconnect_all();
    }
}

#[cfg(test)]
mod url_alias_tests {
    use super::*;

    #[test]
    fn server_url_and_http_url_canonicalize_to_url() {
        for key in ["serverUrl", "httpUrl"] {
            let v = json!({ "mcpServers": { "stitch": { key: "https://stitch.googleapis.com/mcp", "headers": { "X-Goog-Api-Key": "k" } } } });
            let pairs = normalize_server_input(&v, None).unwrap();
            assert_eq!(pairs[0].1["url"], "https://stitch.googleapis.com/mcp");
            assert!(pairs[0].1.get(key).is_none());
            match entry_transport("stitch", &pairs[0].1).unwrap() {
                McpTransport::Sse { headers, .. } => assert_eq!(headers["X-Goog-Api-Key"], "k"),
                _ => panic!("expected remote transport"),
            }
        }
    }
}

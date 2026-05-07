use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Per-model capability flags. Used to suppress request fields a particular
/// model rejects — e.g. some Compatible-provider hosts reject the
/// `temperature` field on Claude Opus 4.7 with a 400 error, so the user can
/// flip `supports_temperature: false` for that model id and we'll omit it.
///
/// Defaults via `serde(default)` mean every existing config (and every model
/// the user hasn't explicitly configured) keeps the prior behaviour: send
/// temperature, no behavioural change.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelCapabilities {
    /// True (default) → the request sends `temperature`. False → the field
    /// is omitted entirely so the provider applies its own default.
    #[serde(default = "default_true")]
    pub supports_temperature: bool,
}

impl Default for ModelCapabilities {
    fn default() -> Self {
        Self { supports_temperature: true }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ProviderType {
    Claude,
    OpenAi,
    Gemini,
    Compatible,
    /// Subscription-mode Claude Code CLI. Not a model API client — Rustic
    /// drives the user-installed `claude` binary as a child process and
    /// streams its NDJSON output. No API key required; the CLI inherits
    /// auth from `~/.claude/`. See `crate::harness` for the runtime.
    ClaudeCode,
    /// Subscription-mode Codex CLI. Same shape as `ClaudeCode` but drives
    /// `codex app-server` over JSON-RPC 2.0 instead of NDJSON. Backend
    /// scaffolding only as of plan §B.10 — `start_session` returns a
    /// "not yet implemented" error until the protocol port lands.
    Codex,
}

impl ProviderType {
    /// True when this provider is implemented as an external CLI process
    /// (the `harness` module), not as an HTTP-API client (`provider/*`).
    /// Callers branch on this to skip the model→tool-loop pipeline.
    pub fn is_harness(&self) -> bool {
        matches!(self, ProviderType::ClaudeCode | ProviderType::Codex)
    }

    /// String form used as the keychain account suffix and as the
    /// `provider_type` field stored on tasks. Mirrors the historical
    /// `format!("{:?}", pt)` output used elsewhere — keep in sync if you add
    /// a new variant.
    pub fn as_str(&self) -> &'static str {
        match self {
            ProviderType::Claude => "Claude",
            ProviderType::OpenAi => "OpenAi",
            ProviderType::Gemini => "Gemini",
            ProviderType::Compatible => "Compatible",
            ProviderType::ClaudeCode => "ClaudeCode",
            ProviderType::Codex => "Codex",
        }
    }
}

/// Returns true when the given `provider_type` string (as stored on a task)
/// names a harness-backed provider. Used by the dispatch branch in
/// `send_message` to route to the harness runtime.
pub fn is_harness_provider_key(key: &str) -> bool {
    key == "ClaudeCode" || key == "Codex"
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderEntry {
    pub provider_type: ProviderType,
    pub api_key: String,
    pub default_model: String,
    pub base_url: Option<String>,
    pub enabled: bool,
    /// Use 1M token context window (Claude & Gemini only). Default: false (200k).
    #[serde(default)]
    pub large_context: bool,
    /// User-specified max output tokens for Compatible provider models.
    /// When > 0, overrides the global max_tokens for models not in the registry.
    #[serde(default)]
    pub custom_max_output_tokens: u32,
    /// User-specified cost per 1M input tokens (USD) for Compatible provider.
    #[serde(default)]
    pub custom_input_cost: f64,
    /// User-specified cost per 1M output tokens (USD) for Compatible provider.
    #[serde(default)]
    pub custom_output_cost: f64,
    /// User-specified cost per 1M cached-input tokens (USD) for Compatible provider.
    #[serde(default)]
    pub custom_cached_input_cost: f64,
    /// User-specified cost per 1M cached-output tokens (USD) for Compatible provider.
    #[serde(default)]
    pub custom_cached_output_cost: f64,
    /// User-specified context window size (tokens). When > 0, overrides the
    /// family-based default. Applies to every model served by this provider.
    #[serde(default)]
    pub custom_context_window: u32,
    /// User-specified thinking budget (tokens). When > 0, overrides the
    /// per-provider default (10k for Claude, 0 elsewhere). Setting to 1 is
    /// treated as "disabled" for providers where 0 means "use default" is ambiguous —
    /// the command layer interprets a value of `u32::MAX` as "disable thinking".
    #[serde(default)]
    pub custom_thinking_budget: u32,
    /// Display name, used only by Compatible providers to disambiguate
    /// multiple instances (e.g. "Groq", "DeepSeek"). `None` for single-instance
    /// providers (Claude, OpenAi, Gemini).
    #[serde(default)]
    pub name: Option<String>,
}

/// Slugify a Compatible-provider display name to match the frontend's
/// `slugify` in `src/components/settings/ai-settings.js`. Both sides must
/// agree on the slug, otherwise `find_by_key` lookups miss and chats fail
/// with "provider … is no longer configured".
fn slugify_name(name: &str) -> String {
    let lowered = name.trim().to_lowercase();
    let mut out = String::with_capacity(lowered.len());
    let mut prev_dash = false;
    for ch in lowered.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

impl ProviderEntry {
    /// Unique string key identifying this provider instance, used as the
    /// `provider_type` stored on tasks and sent between frontend and backend.
    ///
    /// - `"Claude"`, `"OpenAi"`, `"Gemini"` — singletons.
    /// - `"Compatible:<slug>"` — Compatible providers with a user-supplied name.
    /// - `"Compatible"` — legacy unnamed Compatible (pre-multi-provider).
    pub fn provider_key(&self) -> String {
        match (&self.provider_type, &self.name) {
            (ProviderType::Compatible, Some(raw)) if !raw.trim().is_empty() => {
                let slug = slugify_name(raw);
                if slug.is_empty() {
                    format!("{:?}", self.provider_type)
                } else {
                    format!("Compatible:{}", slug)
                }
            }
            _ => format!("{:?}", self.provider_type),
        }
    }
}

impl AiConfig {
    /// Find a provider entry by its key. Falls back to the first Compatible
    /// entry when looking up legacy `"Compatible"` keys stored on old tasks.
    pub fn find_by_key(&self, key: &str) -> Option<&ProviderEntry> {
        if let Some(entry) = self.providers.iter().find(|p| p.provider_key() == key) {
            return Some(entry);
        }
        if key == "Compatible" {
            return self
                .providers
                .iter()
                .find(|p| matches!(p.provider_type, ProviderType::Compatible));
        }
        None
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AiConfig {
    pub providers: Vec<ProviderEntry>,
    pub default_provider: Option<ProviderType>,
    pub temperature: f32,
    pub max_tokens: u32,
    /// Per-model capability overrides keyed by model id (the same id sent in
    /// the API request, e.g. `"claude-opus-4-7"` or
    /// `"openai/gpt-oss-120b"`). Models not in this map use the default
    /// capabilities.
    #[serde(default)]
    pub model_capabilities: HashMap<String, ModelCapabilities>,
}

impl AiConfig {
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
            default_provider: None,
            temperature: 0.7,
            max_tokens: 16384,
            model_capabilities: HashMap::new(),
        }
    }

    /// Look up the capability flags for `model_id`, falling back to defaults
    /// when the model has no explicit entry.
    pub fn capabilities_for(&self, model_id: &str) -> ModelCapabilities {
        self.model_capabilities
            .get(model_id)
            .cloned()
            .unwrap_or_default()
    }
}

/// Supported client-side search backends. `Mcp` means "let the user's MCP
/// server handle it" — our code registers no `web_search` tool in that case
/// and the MCP-registered one takes over.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum WebSearchBackend {
    Tavily,
    Brave,
    Mcp,
}

impl Default for WebSearchBackend {
    fn default() -> Self {
        WebSearchBackend::Tavily
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WebSearchConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub backend: WebSearchBackend,
    /// API key for Tavily / Brave. Empty string = not configured.
    #[serde(default)]
    pub api_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebFetchConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl Default for WebFetchConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

fn default_true() -> bool {
    true
}

/// Agent-level tool configuration. Persisted in the DB under key "tool_config"
/// and loaded into `AgentState` at startup. Plumbed into `ToolContext` for
/// client-side tools; consumed by the provider adapters (claude/gemini) for
/// server-side tool injection.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ToolConfig {
    #[serde(default)]
    pub web_search: WebSearchConfig,
    #[serde(default)]
    pub web_fetch: WebFetchConfig,
}

impl ToolConfig {
    pub fn new() -> Self {
        Self::default()
    }
}

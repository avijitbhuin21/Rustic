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
    /// True (default) → when the user enables thinking/reasoning, the
    /// provider adapter sends the appropriate parameter (Claude `thinking`,
    /// Gemini `thinkingConfig`, OpenAI / OpenRouter `reasoning`). False →
    /// the field is omitted even if `thinking_budget > 0`, so providers /
    /// gateways that 400 on an unknown reasoning field don't trip on it.
    /// Defaulting to true keeps every existing model on the previous
    /// behaviour without requiring re-registration.
    #[serde(default = "default_true")]
    pub supports_reasoning_effort: bool,
}

impl Default for ModelCapabilities {
    fn default() -> Self {
        Self {
            supports_temperature: true,
            supports_reasoning_effort: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ProviderType {
    Claude,
    OpenAi,
    Gemini,
    Compatible,
    OpenRouter,
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
            ProviderType::OpenRouter => "OpenRouter",
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

/// Optional sub-agent override. When set, the main agent's `spawn_subagent`
/// schema gains a `model_tier` parameter — `"intelligent"` keeps the main
/// chat model (current behaviour), `"fast"` swaps in the cheaper model
/// configured below. When unset, sub-agents always inherit the main model
/// and the schema does not expose the choice at all.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct SubagentConfig {
    /// Provider key as produced by `ProviderEntry::provider_key()` —
    /// e.g. `"Claude"`, `"OpenAi"`, `"Compatible:groq"`. Looked up via
    /// `AiConfig::find_by_key` to obtain the API key, base URL, etc.
    pub provider_key: String,
    /// Concrete model id sent on the API request, e.g.
    /// `"claude-haiku-4-5"`, `"gpt-4.1-mini"`, `"gemini-3-flash"`.
    pub model: String,
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
    /// Optional cheaper-and-faster model used for sub-agents when the main
    /// agent picks `model_tier: "fast"`. `None` means the schema doesn't
    /// expose the tier choice and sub-agents always inherit the main model.
    #[serde(default)]
    pub subagent: Option<SubagentConfig>,
    /// P0.4: cross-task budgets (concurrent-stream cap + daily cost
    /// ceiling). Either field on this struct can be `None` to disable that
    /// gate; the default (no settings configured) leaves both off so
    /// existing installs are unaffected until the user opts in via
    /// Settings.
    #[serde(default)]
    pub budget: crate::budget::BudgetSettings,
}

impl AiConfig {
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
            default_provider: None,
            temperature: 0.7,
            max_tokens: 16384,
            model_capabilities: HashMap::new(),
            subagent: None,
            budget: crate::budget::BudgetSettings::default(),
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

/// One configured media-generation tool. The user picks an existing provider
/// (by its `ProviderEntry::provider_key()`), the concrete model id the
/// provider supports, and a per-call output count. The agent only sees the
/// corresponding tool (image_create / video_create / animate) when this entry
/// is filled in — `provider_key` empty disables registration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MediaModelEntry {
    /// Provider key produced by `ProviderEntry::provider_key()`. Examples:
    /// `"OpenAi"`, `"Gemini"`, `"OpenRouter"`. The provider's API key is
    /// looked up via `AiConfig::find_by_key` at tool-call time so users do
    /// not double-configure credentials.
    #[serde(default)]
    pub provider_key: String,
    /// Concrete model id sent to the provider. Examples:
    /// `"gpt-image-1"`, `"gemini-2.5-flash-image"`,
    /// `"google/gemini-2.5-flash-image-preview"` (OpenRouter),
    /// `"sora-2"`, `"veo-3.1-generate-preview"`.
    #[serde(default)]
    pub model: String,
    /// Max outputs the model is permitted to request in a single call.
    /// `0` (the serde default) is treated as `1` at the tool boundary.
    #[serde(default)]
    pub max_per_call: u32,
}

impl MediaModelEntry {
    pub fn is_configured(&self) -> bool {
        !self.provider_key.trim().is_empty() && !self.model.trim().is_empty()
    }

    pub fn effective_max(&self) -> u32 {
        if self.max_per_call == 0 {
            1
        } else {
            self.max_per_call.min(10)
        }
    }
}

/// User-configured media-generation tools. Each tool is independently
/// enabled by setting the corresponding entry's `provider_key` + `model`.
/// `animate` is image-to-video: the model takes an image (from a path
/// inside the project) plus a prompt and returns a short clip.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MediaConfig {
    #[serde(default)]
    pub image: MediaModelEntry,
    #[serde(default)]
    pub video: MediaModelEntry,
    #[serde(default)]
    pub animate: MediaModelEntry,
    /// When true, the `animate` tool reuses the `video` tool's configured
    /// provider+model+max instead of `animate`'s own entry. Surfaced as a
    /// toggle in Settings → Tools so users who run the same model for both
    /// don't have to enter it twice.
    #[serde(default)]
    pub link_animate_to_video: bool,
}

impl MediaConfig {
    /// Effective `animate` config — returns `video` when `link_animate_to_video`
    /// is set, otherwise the dedicated `animate` entry.
    pub fn effective_animate(&self) -> &MediaModelEntry {
        if self.link_animate_to_video {
            &self.video
        } else {
            &self.animate
        }
    }
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
    #[serde(default)]
    pub media: MediaConfig,
}

impl ToolConfig {
    pub fn new() -> Self {
        Self::default()
    }
}

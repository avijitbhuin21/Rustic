use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Per-model flags to suppress request fields that some providers reject with 400.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelCapabilities {
    /// When false, omits `temperature` from the request.
    #[serde(default = "default_true")]
    pub supports_temperature: bool,
    /// When false, omits thinking/reasoning params even if `thinking_budget > 0`.
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
}

impl ProviderType {
    /// Matches `format!("{:?}", pt)` — keep in sync when adding variants.
    pub fn as_str(&self) -> &'static str {
        match self {
            ProviderType::Claude => "Claude",
            ProviderType::OpenAi => "OpenAi",
            ProviderType::Gemini => "Gemini",
            ProviderType::Compatible => "Compatible",
            ProviderType::OpenRouter => "OpenRouter",
        }
    }
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
    /// Overrides the default thinking budget. `u32::MAX` disables thinking.
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

/// Cheaper model used for sub-agents when `model_tier: "fast"` is selected.
/// When unset, sub-agents inherit the main model.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct SubagentConfig {
    /// Provider key from `ProviderEntry::provider_key()`, e.g. `"Claude"`.
    pub provider_key: String,
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AiConfig {
    pub providers: Vec<ProviderEntry>,
    pub default_provider: Option<ProviderType>,
    pub temperature: f32,
    pub max_tokens: u32,
    /// Per-model capability overrides keyed by model id.
    #[serde(default)]
    pub model_capabilities: HashMap<String, ModelCapabilities>,
    #[serde(default)]
    pub subagent: Option<SubagentConfig>,
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

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ProviderType {
    Claude,
    OpenAi,
    Gemini,
    Compatible,
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

impl ProviderEntry {
    /// Unique string key identifying this provider instance, used as the
    /// `provider_type` stored on tasks and sent between frontend and backend.
    ///
    /// - `"Claude"`, `"OpenAi"`, `"Gemini"` — singletons.
    /// - `"Compatible:<slug>"` — Compatible providers with a user-supplied name.
    /// - `"Compatible"` — legacy unnamed Compatible (pre-multi-provider).
    pub fn provider_key(&self) -> String {
        match (&self.provider_type, &self.name) {
            (ProviderType::Compatible, Some(slug)) if !slug.is_empty() => {
                format!("Compatible:{}", slug)
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
}

impl AiConfig {
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
            default_provider: None,
            temperature: 0.7,
            max_tokens: 16384,
        }
    }
}

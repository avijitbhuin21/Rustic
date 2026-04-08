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

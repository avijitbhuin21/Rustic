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
            max_tokens: 4096,
        }
    }
}

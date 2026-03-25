use super::{AiProvider, AiResponse, Message, ModelInfo, ProviderConfig, ToolDef};
use super::openai::OpenAiProvider;
use anyhow::Result;
use async_trait::async_trait;

/// Generic OpenAI-compatible provider.
/// Works with OpenRouter, Grok, Gemini (via OpenAI-compatible endpoint), local models, etc.
/// Delegates to OpenAI provider with custom base_url.
pub struct CompatibleProvider {
    inner: OpenAiProvider,
    provider_name: String,
}

impl CompatibleProvider {
    pub fn new(name: String) -> Self {
        Self {
            inner: OpenAiProvider::new(),
            provider_name: name,
        }
    }
}

#[async_trait]
impl AiProvider for CompatibleProvider {
    async fn chat(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDef>,
        config: &ProviderConfig,
    ) -> Result<AiResponse> {
        // The base_url in config should already point to the compatible endpoint
        self.inner.chat(messages, tools, config).await
    }

    fn name(&self) -> &str {
        &self.provider_name
    }

    fn available_models(&self) -> Vec<ModelInfo> {
        // Compatible providers have user-configured models
        vec![]
    }
}

pub mod claude;
pub mod openai;
pub mod compatible;

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

// === Message types ===

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Role {
    #[serde(rename = "user")]
    User,
    #[serde(rename = "assistant")]
    Assistant,
    #[serde(rename = "system")]
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
    /// Extended thinking block — shown as collapsible in the UI.
    #[serde(rename = "thinking")]
    Thinking {
        thinking: String,
        /// Opaque signature returned by the API; must be echoed back in subsequent requests.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
        /// Duration in seconds the model spent thinking (stamped after turn completes).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_secs: Option<u64>,
    },
    /// UI-only marker injected when the user switches model mid-chat.
    /// Never serialized to the API — filtered out by the executor before every provider call.
    #[serde(rename = "model_switch")]
    ModelSwitch {
        from_model: String,
        to_model: String,
    },
}

/// Events emitted by a provider as it streams a response.
pub enum ProviderStreamEvent {
    TextDelta(String),
    ThinkingDelta(String),
}

/// Callback invoked for each streaming token from the provider.
pub type StreamCallback = Arc<dyn Fn(ProviderStreamEvent) + Send + Sync>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value, // JSON Schema
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_read_tokens: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    Error(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiResponse {
    pub content: Vec<ContentBlock>,
    pub usage: TokenUsage,
    pub stop_reason: StopReason,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub max_tokens: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub api_key: String,
    pub model: String,
    pub max_tokens: u32,
    pub temperature: f32,
    pub base_url: Option<String>,
    /// System prompt injected into every API call (not stored in messages history).
    pub system_prompt: Option<String>,
    /// Budget for extended thinking tokens. When > 0, the provider enables thinking.
    #[serde(default)]
    pub thinking_budget: u32,
    /// Context window size for this model in tokens. Used for context condensing.
    /// When 0, condensing is disabled.
    #[serde(default)]
    pub context_window: u32,
    /// Cancellation token — checked during streaming to abort early.
    #[serde(skip)]
    pub cancel_token: Option<Arc<std::sync::atomic::AtomicBool>>,
}

// === Provider trait ===

#[async_trait]
pub trait AiProvider: Send + Sync {
    /// Send a chat request. If `stream_cb` is Some, the provider should call it
    /// with each text/thinking delta as the response streams in.
    async fn chat(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDef>,
        config: &ProviderConfig,
        stream_cb: Option<StreamCallback>,
    ) -> Result<AiResponse>;

    fn name(&self) -> &str;
    fn available_models(&self) -> Vec<ModelInfo>;
}

pub mod claude;
pub mod compatible;
pub mod gemini;
pub mod openai;

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
        /// Opaque thought signature from Gemini — must be echoed back with the
        /// functionCall part in subsequent requests. Always None for other providers.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thought_signature: Option<String>,
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
    /// Base64-encoded image attached by the user.
    #[serde(rename = "image")]
    Image {
        media_type: String,
        data: String,
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
    /// A server-executed tool call (e.g. Anthropic `web_search`) just finished
    /// streaming its input. Emitted at content_block_stop so `input` is
    /// complete. The executor forwards this to the UI as `TaskEvent::ToolUse`.
    ServerToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    /// A server-executed tool's result block just arrived (content is final
    /// at content_block_start, not streamed). The executor forwards this to
    /// the UI as `TaskEvent::ToolResult`.
    ServerToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_read_tokens: u32,
    /// Tokens written to the prompt cache on this request (Anthropic: cache_creation_input_tokens).
    #[serde(default)]
    pub cache_write_tokens: u32,
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
    /// Cost per 1M input tokens (USD). 0.0 if unknown.
    #[serde(default)]
    pub input_cost_per_m: f64,
    /// Cost per 1M output tokens (USD). 0.0 if unknown.
    #[serde(default)]
    pub output_cost_per_m: f64,
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
    /// Whether the user has enabled web_search. Consumed by provider adapters
    /// that support server-side tools (Claude, Gemini) to inject the native
    /// tool declaration. Client-side providers ignore this flag — the tool
    /// executor itself gates registration on the frontend config.
    #[serde(default)]
    pub web_search_enabled: bool,
    /// Same for web_fetch.
    #[serde(default)]
    pub web_fetch_enabled: bool,
    /// True (default) → request includes `temperature`. False → it's omitted
    /// so the model can apply its own default. Pulled from the per-model
    /// capability overrides on `AiConfig`. Some Compatible-provider routers
    /// reject `temperature` on certain models (e.g. Claude Opus 4.7 on some
    /// hosts), so the user can flip this off without losing the model.
    #[serde(default = "default_true")]
    pub supports_temperature: bool,
    /// True → when `thinking_budget > 0`, the adapter sends the
    /// provider-specific reasoning parameter (Claude `thinking`, Gemini
    /// `thinkingConfig`, OpenAI / OpenRouter `reasoning`). False → the
    /// field is suppressed so models that 400 on an unknown reasoning
    /// field don't break. Mirrored from
    /// `ModelCapabilities::supports_reasoning_effort`.
    #[serde(default = "default_true")]
    pub supports_reasoning_effort: bool,
    /// Cancellation token — checked during streaming to abort early.
    #[serde(skip)]
    pub cancel_token: Option<Arc<std::sync::atomic::AtomicBool>>,
}

fn default_true() -> bool { true }

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

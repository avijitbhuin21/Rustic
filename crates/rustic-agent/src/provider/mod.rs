pub mod claude;
pub mod openai;
pub mod compatible;

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

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
}

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
}

// === Provider trait ===

#[async_trait]
pub trait AiProvider: Send + Sync {
    async fn chat(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDef>,
        config: &ProviderConfig,
    ) -> Result<AiResponse>;

    fn name(&self) -> &str;
    fn available_models(&self) -> Vec<ModelInfo>;
}

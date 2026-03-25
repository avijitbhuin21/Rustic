pub mod checkpoint;
pub mod config;
pub mod mcp;
pub mod provider;
pub mod task;
pub mod tools;

pub use checkpoint::{CheckpointInfo, FileChange};
pub use checkpoint::snapshot as checkpoint_ops;
pub use config::{AiConfig, ProviderEntry, ProviderType};
pub use provider::{
    AiProvider, AiResponse, ContentBlock, Message, ModelInfo, ProviderConfig, Role, StopReason,
    TokenUsage, ToolDef,
};
pub use task::{TaskInfo, TaskStatus};
pub use task::executor::{TaskEvent, TaskExecutor};
pub use task::permissions::PermissionLevel;
pub use tools::{BuiltinTools, ToolContext, ToolExecutor, ToolOutput};
pub use mcp::{McpManager, ServerConfig, McpTransport};

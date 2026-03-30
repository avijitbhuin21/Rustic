use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum PermissionLevel {
    /// Read-only. No file writes or command execution.
    Chat,
    /// File writes and commands require per-operation user approval. (Default)
    ManualEdit,
    /// File writes auto-allowed. Command execution requires user approval.
    AutoEdit,
    /// File writes and commands all auto-allowed. No approval prompts.
    FullAuto,
}

impl Default for PermissionLevel {
    fn default() -> Self {
        Self::ManualEdit
    }
}

#[derive(Debug, Clone)]
pub enum Action {
    Read,
    Write,
    Execute,
}

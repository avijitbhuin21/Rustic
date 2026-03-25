pub mod file_ops;
pub mod terminal;
pub mod search;

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;

use crate::provider::ToolDef;
use crate::task::permissions::{Action, PermissionLevel};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutput {
    pub content: String,
    pub is_error: bool,
}

/// Callback type for snapshotting a file before it is modified.
/// Takes the absolute file path, snapshots its current state.
pub type SnapshotFn = Arc<dyn Fn(&std::path::Path) + Send + Sync>;

/// Context available to tool execution.
pub struct ToolContext {
    pub project_root: PathBuf,
    pub permissions: PermissionLevel,
    /// Optional callback to snapshot a file before modification (for checkpoint system).
    pub snapshot_fn: Option<SnapshotFn>,
}

impl ToolContext {
    pub fn check_permission(&self, action: &Action) -> bool {
        match self.permissions {
            PermissionLevel::Admin => true,
            PermissionLevel::ReadWrite => match action {
                Action::Read => true,
                Action::Write | Action::Execute => true, // allowed but UI may confirm
            },
            PermissionLevel::ReadOnly => matches!(action, Action::Read),
        }
    }
}

#[async_trait]
pub trait ToolExecutor: Send + Sync {
    fn definitions(&self) -> Vec<ToolDef>;
    async fn execute(&self, name: &str, params: Value, context: &ToolContext) -> Result<ToolOutput>;
}

/// Built-in tool executor combining all built-in tools.
pub struct BuiltinTools;

impl BuiltinTools {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ToolExecutor for BuiltinTools {
    fn definitions(&self) -> Vec<ToolDef> {
        let mut defs = Vec::new();
        defs.extend(file_ops::definitions());
        defs.extend(terminal::definitions());
        defs.extend(search::definitions());
        defs
    }

    async fn execute(&self, name: &str, params: Value, context: &ToolContext) -> Result<ToolOutput> {
        match name {
            "read_file" | "write_file" | "create_file" | "list_directory" => {
                file_ops::execute(name, params, context).await
            }
            "run_command" => terminal::execute(name, params, context).await,
            "grep_search" => search::execute(name, params, context).await,
            _ => Ok(ToolOutput {
                content: format!("Unknown tool: {}", name),
                is_error: true,
            }),
        }
    }
}

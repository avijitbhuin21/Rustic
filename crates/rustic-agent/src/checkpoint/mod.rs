pub mod snapshot;

use serde::{Deserialize, Serialize};

/// Info about a checkpoint, returned to the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointInfo {
    pub id: String,
    pub task_id: String,
    pub message_index: i64,
    pub created_at: String,
    pub file_count: usize,
}

/// Describes a file change when previewing a checkpoint revert.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChange {
    pub file_path: String,
    /// "restore" = overwrite with snapshot content, "delete" = file was newly created and will be removed
    pub change_type: String,
}

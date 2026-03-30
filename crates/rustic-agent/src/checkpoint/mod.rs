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

/// Status of a file in a task diff.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum DiffStatus {
    Created,
    Modified,
    Deleted,
}

/// Diff for a single file changed during a task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileDiff {
    pub path: String,
    pub status: DiffStatus,
    pub insertions: usize,
    pub deletions: usize,
    /// Full unified diff text for the expand view.
    pub unified_diff: String,
}

/// Aggregated diff for all files changed during a task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskDiff {
    pub files: Vec<FileDiff>,
    pub total_insertions: usize,
    pub total_deletions: usize,
}

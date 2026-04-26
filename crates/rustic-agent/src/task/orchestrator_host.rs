//! Broker trait for the Global orchestrator's cross-project operations.
//!
//! The rustic-agent crate has no access to the host app's AppState, DB, or
//! workspace — so orchestrator tools (`spawn_subtask`,
//! `list_tasks_across_projects`, `read_task_history`) route through this
//! trait. The host implementation lives in src-tauri.

use serde::Serialize;

/// Summary of a project visible to the orchestrator.
#[derive(Debug, Clone, Serialize)]
pub struct OrchestratorProject {
    pub id: String,
    pub name: String,
    pub root_path: String,
    /// Human-readable file-tree listing for the project (gitignore-aware,
    /// capped at a few hundred entries). Lets the orchestrator understand
    /// project layout without a follow-up read call. Empty on walk errors.
    #[serde(default)]
    pub file_tree: String,
}

/// Summary of a task visible to the orchestrator. Kept deliberately narrow
/// so this trait doesn't have to mirror every `TaskInfo` field.
#[derive(Debug, Clone, Serialize)]
pub struct OrchestratorTaskSummary {
    pub task_id: String,
    pub project_id: String,
    pub project_name: String,
    pub title: String,
    pub status: String,
    pub model: String,
    pub created_at: String,
    pub updated_at: String,
}

/// Filters for the cross-project task listing.
#[derive(Debug, Clone, Default)]
pub struct OrchestratorTaskFilter {
    pub project_id: Option<String>,
    pub status: Option<String>,
    pub limit: Option<usize>,
}

/// Serialized message from a task history. Content is the raw JSON of the
/// `ContentBlock` list — the orchestrator is expected to read and summarize,
/// not to round-trip through a typed model.
#[derive(Debug, Clone, Serialize)]
pub struct OrchestratorMessage {
    pub role: String,
    pub content_json: String,
}

/// Host-implemented surface for orchestrator tools.
///
/// Implementations live in src-tauri where AppState + DB are available.
pub trait OrchestratorHost: Send + Sync {
    /// List all projects the orchestrator can delegate to. The Global
    /// pseudo-project is filtered out of results so it never becomes a
    /// spawn target.
    fn list_projects(&self) -> Result<Vec<OrchestratorProject>, String>;

    /// List tasks across every project (or a subset per filter). Falls
    /// back to the DB so tasks from previous sessions are included.
    fn list_tasks(&self, filter: OrchestratorTaskFilter) -> Result<Vec<OrchestratorTaskSummary>, String>;

    /// Return the message history for a specific task as serialized
    /// content blocks. Used by `read_task_history`.
    fn read_task_history(&self, task_id: &str) -> Result<Vec<OrchestratorMessage>, String>;

    /// Create a new top-level task in the named project and dispatch the
    /// first user prompt. Returns the new task id; execution is
    /// fire-and-forget — the orchestrator does not block on the sub-task.
    ///
    /// `parent_task_id` lets the host inherit the orchestrator's current
    /// provider and model so spawned subtasks don't fall back to a stale
    /// default when the user has switched the global agent mid-session.
    fn spawn_subtask(
        &self,
        project_id: &str,
        title: Option<String>,
        prompt: String,
        parent_task_id: &str,
    ) -> Result<String, String>;
}

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectRow {
    pub id: String,
    pub name: String,
    pub root_path: String,
    pub created_at: String,
    pub settings_json: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRow {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub status: String,
    pub provider_type: String,
    pub model: String,
    pub created_at: String,
    pub updated_at: String,
    // Cost tracking
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_cache_read_tokens: i64,
    pub estimated_cost_usd: f64,
    pub turn_count: i64,
    /// Session id reported by a harness CLI (Claude Code, Codex, ...) on its
    /// first `system:init` envelope. We pass this back via `--resume` when
    /// the user reopens the task so the CLI restores its conversation
    /// history. NULL for native API-key tasks and for harness tasks that
    /// haven't sent a first message yet.
    #[serde(default)]
    pub harness_session_id: Option<String>,
    /// Full `TaskCost` snapshot as JSON (migration 018), including the
    /// per-model breakdown. The flat token/cost columns are rollups; this is
    /// the authoritative source when present.
    #[serde(default)]
    pub cost_json: Option<String>,
    /// Reasoning-effort tier the user last used with this task ('off' | 'low'
    /// | 'medium' | 'high' | 'max'). NULL for tasks predating migration 022.
    #[serde(default)]
    pub thinking_tier: Option<String>,
    /// Sticky-note pin: pinned tasks sort to the top of the task tree
    /// (migration 023). Defaults to false.
    #[serde(default)]
    pub pinned: bool,
    /// Active /goal completion condition (migration 024). NULL when no goal
    /// is set; cleared automatically once the evaluator confirms the goal.
    #[serde(default)]
    pub goal: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchivedMessageRow {
    pub task_id: String,
    pub generation: i64,
    pub slot: i64,
    pub role: String,
    pub content_json: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageRow {
    pub id: String,
    pub task_id: String,
    pub role: String,
    pub content_json: String,
    pub created_at: String,
    pub sort_order: i64,
    pub turn_usage_json: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettingRow {
    pub key: String,
    pub value_json: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentRecord {
    pub task_id: String,
    pub agent_id: String,
    pub model: String,
    pub prompt: String,
    /// Human-readable name the orchestrator gave this child at spawn time.
    pub name: String,
    pub summary: String,
    pub status: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cost_usd: f64,
    pub error: String,
    pub created_at: String,
    pub updated_at: String,
    /// Streamed assistant text accumulator. Updated transactionally on each
    /// text-delta from the sub-agent so the activity panel can replay the
    /// run after a restart.
    #[serde(default)]
    pub output_text: String,
    /// JSON-encoded array of tool_use + tool_result pairs the sub-agent has
    /// produced so far. Updated on every tool_use / tool_result event.
    #[serde(default = "default_tool_calls_json")]
    pub tool_calls_json: String,
}

fn default_tool_calls_json() -> String {
    "[]".to_string()
}

/// One tracked GitHub issue (auto-issue-resolve, migration 017). See
/// `github_repo.rs` for the status lifecycle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GithubIssueRow {
    pub id: i64,
    pub project_id: String,
    pub repo_full_name: String,
    pub issue_number: i64,
    pub title: String,
    pub issue_url: String,
    pub task_id: Option<String>,
    pub status: String,
    pub pending_tool_use_id: Option<String>,
    pub pending_questions_json: Option<String>,
    pub cost_cap_usd: Option<f64>,
    pub error: String,
    pub created_at: String,
    pub updated_at: String,
}

/// One queued webhook delivery awaiting the issue worker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GithubEventRow {
    pub id: i64,
    pub issue_id: i64,
    pub kind: String,
    pub payload_json: String,
    pub created_at: String,
}

/// One snapshot row, anchored to a user message UUID. `tree_oid` is the
/// libgit2 hex hash of the shadow tree captured at `open_snapshot` time
/// (R.1 design — see `docs/file_tracking_decision.md`). It's `None` only
/// for rows that pre-date the shadow migration; those snapshots are
/// unrevertable and will be removed by retention on their task's next turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileHistorySnapshotRow {
    pub message_id: String,
    pub task_id: String,
    pub sequence: i64,
    pub tree_oid: Option<String>,
    pub created_at: String,
}

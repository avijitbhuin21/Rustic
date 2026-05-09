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

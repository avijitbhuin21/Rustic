pub mod condense;
pub mod cost;
pub mod executor;
pub mod file_lock;
pub mod orchestrator_host;
pub mod permission_broker;
pub mod permissions;
pub mod subagent;
pub mod terminal_broker;
pub mod user_question_broker;

use crate::checkpoint::TaskDiff;
use crate::provider::Message;
use crate::task::cost::TaskCost;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedSender;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TaskStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskInfo {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub status: TaskStatus,
    pub provider_type: String,
    pub model: String,
    /// ISO-8601 UTC timestamp. Hydrated from the DB row; empty when the
    /// task is still an in-memory scratch (rare — usually populated at
    /// create time via `chrono::Utc::now()`).
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

/// Describes which operation is requesting user approval.
#[derive(Debug, Clone)]
pub enum PermissionOp {
    WriteFile(String),  // path
    CreateFile(String), // path
    RunCommand(String), // command string
    SensitiveFile {
        path: String,
        tier: u8,
        reason: String,
    },
}

impl PermissionOp {
    /// Returns (operation_type, human-readable description, short preview).
    pub fn describe(&self) -> (String, String, Option<String>) {
        match self {
            PermissionOp::WriteFile(path) => (
                "write_file".into(),
                format!("Write: {}", path),
                None,
            ),
            PermissionOp::CreateFile(path) => (
                "create_file".into(),
                format!("Create: {}", path),
                None,
            ),
            PermissionOp::RunCommand(cmd) => {
                // SECURITY: Show the full command, untruncated, in the approval
                // preview. A previous version cut to 80 chars which let prompt
                // injection hide malicious tails after a benign prefix.
                ("run_command".into(), "Run command".into(), Some(cmd.clone()))
            }
            PermissionOp::SensitiveFile { path, tier, reason } => {
                let op_type = format!("sensitive_file_tier{}", tier);
                let description = format!("Sensitive file access: {}", path);
                let preview = Some(reason.clone());
                (op_type, description, preview)
            }
        }
    }
}

/// Events emitted during task execution for real-time UI updates.
#[derive(Debug, Clone)]
pub enum TaskEvent {
    TextDelta { task_id: String, text: String },
    ThinkingDelta { task_id: String, text: String },
    ToolUse { task_id: String, tool_use_id: String, tool_name: String, tool_input: serde_json::Value },
    ToolResult { task_id: String, tool_use_id: String, output: String, is_error: bool },
    StatusChange { task_id: String, status: TaskStatus },
    MessageComplete { task_id: String, message: Message },
    TaskComplete {
        task_id: String,
        diff: TaskDiff,
        /// Final user-facing summary written by the model via `complete_task`.
        /// `None` when the loop ended without an explicit complete_task call
        /// (turn limit, cancellation, model just stopped).
        summary: Option<String>,
    },
    /// Emitted when a tool needs user approval (ManualEdit mode).
    PermissionRequest {
        task_id: String,
        request_id: String,
        operation: String,       // e.g. "write_file"
        description: String,     // e.g. "Write: src/auth.rs"
        preview: Option<String>, // e.g. first line of command
    },
    /// Emitted after every provider call with accumulated cost for this task.
    CostUpdate {
        task_id: String,
        cost: TaskCost,
    },
    /// Emitted when the agent writes to .rustic/memory.md.
    MemoryUpdated { task_id: String },
    /// Emitted when the main model spawns a sub-agent.
    SubagentSpawned { task_id: String, agent_id: String, model: String, prompt: String },
    /// Emitted when a sub-agent completes successfully.
    SubagentCompleted { task_id: String, agent_id: String, summary: String },
    /// Emitted when a sub-agent fails.
    SubagentFailed { task_id: String, agent_id: String, error: String },
    /// Text streaming from a sub-agent (agent_id identifies which one).
    SubagentTextDelta { task_id: String, agent_id: String, text: String },
    /// Cost update from a sub-agent (forwarded from child executor).
    SubagentCostUpdate { task_id: String, agent_id: String, cost: TaskCost },
    /// Emitted when the agent calls chat_message (type: question) to request clarification.
    UserQuestionRequest { task_id: String, request_id: String, question: String },
    /// Emitted when the agent updates its todo list.
    TodoUpdated { task_id: String, todos: Vec<TodoItem> },
    /// Emitted during tool execution to report intermediate progress.
    ToolProgress { task_id: String, tool_use_id: String, progress_text: String },
    /// Emitted when context condensing starts (an API call to summarize the conversation).
    ContextCondenseStarted { task_id: String },
    /// Emitted when context condensing completes.
    ContextCondenseCompleted { task_id: String, original_messages: u32, condensed_to: u32 },
    /// Emitted after every provider call with the raw token counts for THIS request.
    /// Separate from CostUpdate (which is cumulative). Used for per-request visibility
    /// and for accumulating per-user-turn cost in the UI.
    RequestUsage {
        task_id: String,
        input_tokens: u32,
        output_tokens: u32,
        cache_read_tokens: u32,
        cache_write_tokens: u32,
        /// USD cost of this single request (computed with the current model's prices).
        cost_usd: f64,
    },
}

/// A single item in the agent's todo list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    pub content: String,
    pub status: String, // "pending", "in_progress", "completed"
}

pub type EventTx = UnboundedSender<TaskEvent>;

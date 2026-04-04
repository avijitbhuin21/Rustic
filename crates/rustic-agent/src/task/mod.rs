pub mod condense;
pub mod cost;
pub mod executor;
pub mod file_lock;
pub mod permission_broker;
pub mod permissions;
pub mod subagent;
pub mod user_question_broker;

use crate::checkpoint::TaskDiff;
use crate::provider::Message;
use crate::task::cost::TaskCost;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc::UnboundedSender;

/// Per-task turn budget. Shared between the executor loop and the extend_turn_budget command.
#[derive(Debug, Clone)]
pub struct TurnBudget {
    pub used: u32,
    pub max: u32,
}

impl TurnBudget {
    pub fn new(max: u32) -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self { used: 0, max }))
    }

    pub fn remaining(lock: &Arc<Mutex<Self>>) -> u32 {
        let b = lock.lock().unwrap();
        b.max.saturating_sub(b.used)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TaskStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
    TurnLimitReached,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskInfo {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub status: TaskStatus,
    pub provider_type: String,
    pub model: String,
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
                let preview = if cmd.len() > 80 {
                    Some(format!("{}…", &cmd[..80]))
                } else {
                    Some(cmd.clone())
                };
                ("run_command".into(), "Run command".into(), preview)
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
        summary: String,
        notes: Option<String>,
        diff: TaskDiff,
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
    /// Emitted when the turn count reaches (max - 5) to warn the model.
    TurnBudgetWarning {
        task_id: String,
        turns_remaining: u32,
    },
    /// Emitted when the agent writes to .rustic/memory.md.
    MemoryUpdated { task_id: String },
    /// Emitted when the main model spawns a sub-agent.
    SubagentSpawned { task_id: String, agent_id: String, model: String },
    /// Emitted when a sub-agent completes successfully.
    SubagentCompleted { task_id: String, agent_id: String, summary: String },
    /// Emitted when a sub-agent fails.
    SubagentFailed { task_id: String, agent_id: String, error: String },
    /// Text streaming from a sub-agent (agent_id identifies which one).
    SubagentTextDelta { task_id: String, agent_id: String, text: String },
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
}

/// A single item in the agent's todo list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    pub content: String,
    pub status: String, // "pending", "in_progress", "completed"
}

pub type EventTx = UnboundedSender<TaskEvent>;

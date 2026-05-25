pub mod condense;
pub mod cost;
pub mod executor;
pub mod file_lock;
pub mod ask_user_broker;
pub mod ceiling_broker;
pub mod permission_broker;
pub mod permissions;
pub mod subagent;
pub mod terminal_broker;
pub mod tool_search;

use crate::provider::Message;
use crate::task::cost::TaskCost;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::Sender;

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
    /// Name + id known, args pending. Frontend shows spinner; followed by
    /// `ToolUseInputDelta*`, `ToolUseStop`, then `ToolUse` before execution.
    ToolUseStart { task_id: String, tool_use_id: String, tool_name: String },
    /// A fragment of the tool's input JSON. Concatenate raw on the frontend.
    ToolUseInputDelta { task_id: String, tool_use_id: String, partial_json: String },
    /// Input finalized; execution starts shortly after via `ToolUse`.
    ToolUseStop { task_id: String, tool_use_id: String },
    ToolUse { task_id: String, tool_use_id: String, tool_name: String, tool_input: serde_json::Value },
    ToolResult { task_id: String, tool_use_id: String, output: String, is_error: bool },
    StatusChange { task_id: String, status: TaskStatus },
    MessageComplete { task_id: String, message: Message },
    TaskComplete {
        task_id: String,
        /// Always `None`; kept for persisted-history compat with old `complete_task` summaries.
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
    /// Emitted when a sub-agent completes successfully. `model` carries the
    /// resolved model the sub-agent actually ran on (intelligent / fast tier),
    /// so the completion card stays correctly labelled even if the original
    /// `SubagentSpawned` event was missed (race with first paint, task
    /// reload, etc.).
    SubagentCompleted { task_id: String, agent_id: String, model: String, summary: String },
    /// Emitted when a sub-agent fails.
    SubagentFailed { task_id: String, agent_id: String, error: String },
    /// Text streaming from a sub-agent (agent_id identifies which one).
    SubagentTextDelta { task_id: String, agent_id: String, text: String },
    /// Extended-thinking streaming from a sub-agent. Carried on a separate
    /// channel from `SubagentTextDelta` because the coalescer concatenates
    /// per-event strings — interleaving `[thinking]`-prefixed deltas with
    /// plain text deltas would lose the boundary markers entirely.
    SubagentThinkingDelta { task_id: String, agent_id: String, text: String },
    /// Cost update from a sub-agent (forwarded from child executor).
    SubagentCostUpdate { task_id: String, agent_id: String, cost: TaskCost },
    /// Tool use emitted by a sub-agent (forwarded so the frontend can render a tool card).
    SubagentToolUse { task_id: String, agent_id: String, tool_name: String, tool_use_id: String, input: serde_json::Value },
    /// Tool result emitted by a sub-agent (forwarded so the frontend can show the result card).
    SubagentToolResult { task_id: String, agent_id: String, tool_use_id: String, content: String, is_error: bool },
    /// Emitted when the agent updates its todo list.
    TodoUpdated { task_id: String, todos: Vec<TodoItem> },
    /// Emitted during tool execution to report intermediate progress.
    ToolProgress { task_id: String, tool_use_id: String, progress_text: String },
    /// Emitted when context condensing starts (an API call to summarize the conversation).
    ContextCondenseStarted { task_id: String },
    /// Emitted when context condensing completes.
    ContextCondenseCompleted { task_id: String, original_messages: u32, condensed_to: u32 },
    /// Emitted by the changed-files tracker when one or more paths are
    /// recorded into the snapshot for the current user message. `paths` are
    /// project-relative (forward slashes). `kind` distinguishes the tool that
    /// produced the change so the UI can icon it differently.
    FileTracked {
        task_id: String,
        message_id: String,
        kind: FileTrackedKind,
        paths: Vec<String>,
    },
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
    /// P0.1: the provider call failed with a transient/stream error and we
    /// are about to retry. Fires before the backoff sleep so the UI can show
    /// "retrying in <waiting_ms>" instead of leaving the user staring at a
    /// frozen spinner. `attempt` is 1-indexed (so attempt=2 means "first
    /// retry"); after `attempt = max_attempts` fails, the executor surfaces
    /// the underlying error instead of firing another StreamRetry.
    ///
    /// `error` carries a short human-readable cause so the UI can render
    /// "Rate limit exceeded, retrying in 60s" instead of just a spinner.
    /// `None` is reserved for non-error retries (e.g. stream stalled with
    /// no upstream HTTP status).
    StreamRetry {
        task_id: String,
        attempt: u32,
        max_attempts: u32,
        waiting_ms: u32,
        error: Option<String>,
    },
    /// P0.2: the `ask_user` tool fired and is waiting on the user's
    /// answers. `questions` is the JSON array the agent passed to the
    /// tool (each entry has `id`, `text`, `kind`, optional `options`).
    /// The frontend renders a tabbed dialog and replies via the
    /// `respond_to_ask_user` Tauri command, which routes back to the
    /// `AskUserBroker` to unblock the awaiting tool.
    AskUserRequest {
        task_id: String,
        request_id: String,
        questions: serde_json::Value,
    },
    /// P0.4 fix #4: the daily-cost ceiling has been hit at the top of a new
    /// turn. The task is parked on the [`CeilingBroker`]; the frontend
    /// renders a modal with "Raise ceiling to …" + "Stop task" and replies
    /// via `respond_to_ceiling_breach`. Without this, the executor's only
    /// option was to hard-fail the task — the user had to dig into Settings
    /// and re-send the message to retry.
    CeilingBreached {
        task_id: String,
        request_id: String,
        ceiling_cents: u64,
        spent_cents: u64,
    },
    /// P1.9: emitted when the executor has been parked waiting on running
    /// sub-agents past the 30-minute soft timeout. The task is not
    /// cancelled — the executor keeps waiting in 30-min chunks — but the
    /// user gets a visible notice so they can decide whether to wait
    /// longer or stop the task. Re-emitted each time another 30 minutes
    /// elapses with no completion.
    SubagentParkTimeout {
        task_id: String,
        running_agents: Vec<String>,
        parked_minutes: u32,
    },
}

/// Source of a `FileTracked` event — drives the UI icon for the changed-files panel.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FileTrackedKind {
    /// File captured by an edit tool BEFORE its mutation (create / edit / patch).
    EditTool,
    /// File detected post-bash by the sweep worker.
    BashSweep,
}

/// A single item in the agent's todo list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    pub content: String,
    pub status: String, // "pending", "in_progress", "completed"
}

/// Bounded sender for TaskEvent. Used to be `UnboundedSender`, but that let
/// the queue grow without limit when the consumer (Tauri event emitter →
/// frontend) was slower than the producer (streaming token deltas, parallel
/// tool results). A 16K capacity cap is generous enough that overflow only
/// occurs when the frontend is genuinely stalled; sites use `try_send` and
/// drop the event in that case (better than OOM).
///
/// Recommended capacity when constructing the channel: `EVENT_CHANNEL_CAP`.
pub type EventTx = Sender<TaskEvent>;

pub const EVENT_CHANNEL_CAP: usize = 16384;

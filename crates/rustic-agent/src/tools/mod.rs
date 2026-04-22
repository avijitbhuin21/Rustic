pub mod ask_user;
pub mod complete_task;
pub mod file_ops;
pub mod skill_tools;
pub mod subagent_tools;
pub mod terminal;
pub mod search;
pub mod todo_tools;
pub mod workflow_tools;

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;

use crate::checkpoint::TaskDiff;
use crate::mcp::McpManager;
use crate::provider::{ProviderConfig, ToolDef};
use crate::task::file_lock::FileLockRegistry;
use crate::task::permission_broker::PermissionBroker;
use crate::task::permissions::{Action, PermissionLevel, SharedPermissions};
use crate::task::terminal_broker::AgentTerminals;
use crate::task::user_question_broker::UserQuestionBroker;
use crate::task::{EventTx, TurnBudget};
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutput {
    pub content: String,
    pub is_error: bool,
}

/// Callback type for snapshotting a file before it is modified.
/// Takes the absolute file path, snapshots its current state.
pub type SnapshotFn = Arc<dyn Fn(&std::path::Path) + Send + Sync>;

/// Callback type for computing the task diff at completion time.
/// Captures the task_id and DB reference.
pub type ComputeDiffFn = Arc<dyn Fn() -> TaskDiff + Send + Sync>;

/// Context available to tool execution.
pub struct ToolContext {
    pub project_root: PathBuf,
    /// Shared permissions — updated in real-time when the user changes permission mode mid-conversation.
    pub shared_permissions: SharedPermissions,
    /// Optional callback to snapshot a file before modification (for checkpoint system).
    pub snapshot_fn: Option<SnapshotFn>,
    /// Optional callback to compute the task diff at completion time.
    pub compute_diff_fn: Option<ComputeDiffFn>,
    /// Cancellation flag — set to true by abort_task to stop the executor loop.
    pub cancel_token: Option<Arc<AtomicBool>>,
    /// Broker for per-operation approval in ManualEdit / AutoEdit modes.
    pub permission_broker: Arc<PermissionBroker>,
    /// Channel for emitting task events (TextDelta, ToolUse, PermissionRequest, etc.)
    pub event_tx: EventTx,
    /// The task ID this context belongs to.
    pub task_id: String,
    /// Shared turn budget for this task — incremented by the executor each loop.
    pub turn_budget: Arc<Mutex<TurnBudget>>,
    /// Per-file lock registry — all write tools acquire a per-path lock before modifying files.
    pub file_lock: Arc<FileLockRegistry>,
    /// Shared MCP manager — used by the executor to route non-builtin tool calls.
    pub mcp_manager: Option<Arc<Mutex<McpManager>>>,
    /// Pre-fetched MCP tool definitions — combined with builtin defs before each provider call.
    pub mcp_tool_defs: Vec<ToolDef>,
    /// Sub-agent registry — allows tools to register/complete sub-agents.
    pub subagent_registry: Arc<crate::task::subagent::SubagentRegistry>,
    /// 0 = main agent, 1 = sub-agent. Sub-agents cannot spawn further sub-agents.
    pub agent_depth: u8,
    /// Provider configurations for sub-agent model resolution.
    pub ai_config: Arc<crate::config::AiConfig>,
    /// Paths pre-approved by the user in `.rustic/allowed-files.txt`. These skip tier-2/3 confirmation.
    pub allowed_paths: Vec<String>,
    /// Broker for ask_user tool — pauses execution and waits for user text input.
    pub question_broker: Arc<UserQuestionBroker>,
    /// The parent agent's provider config — sub-agents inherit model, API key, max_tokens, thinking_budget.
    pub parent_provider_config: Option<Arc<ProviderConfig>>,
    /// Set by the `complete_task` tool when the model explicitly ends the task.
    /// The executor loop checks this after every tool batch and, when populated,
    /// breaks the loop using this string as the task's final summary.
    /// For sub-agents, this string becomes the summary returned to the parent.
    pub completion_summary: Arc<Mutex<Option<String>>>,
    /// Paths this agent is allowed to write to (repo-relative, directory-prefix
    /// semantics matching `paths_overlap`).
    /// `None` = unrestricted (main agent). `Some(paths)` = scoped sub-agent;
    /// writes outside these paths are rejected with WRITE_SCOPE_VIOLATION.
    /// `Some(vec![])` = read-only sub-agent (every write is rejected).
    pub write_scope: Option<Vec<String>>,
    /// Writes the sub-agent declined to make because they were out of scope.
    /// Populated by the `report_blocked_write` tool; drained into
    /// `SubagentResult.blocked_on` when the sub-agent finishes. Always
    /// present so the tool can run regardless of agent type — the main
    /// agent simply never calls the tool.
    pub blocked_writes: Arc<Mutex<Vec<crate::task::subagent::BlockedWrite>>>,
    /// Broker for agent-owned pty terminals. Host app (src-tauri) supplies an
    /// implementation wired to the shared TerminalManager. `None` in unit tests
    /// or embedded contexts — tools must handle this gracefully.
    pub agent_terminals: Option<Arc<dyn AgentTerminals>>,
}

impl ToolContext {
    /// Emit a tool progress event (non-blocking). Used by long-running tools
    /// to report intermediate status to the UI (e.g., "5 matches found so far").
    pub fn emit_progress(&self, tool_use_id: &str, progress_text: &str) {
        let _ = self.event_tx.send(crate::task::TaskEvent::ToolProgress {
            task_id: self.task_id.clone(),
            tool_use_id: tool_use_id.to_string(),
            progress_text: progress_text.to_string(),
        });
    }

    /// Read the current permission level (may change mid-conversation).
    pub fn permissions(&self) -> PermissionLevel {
        self.shared_permissions.level()
    }

    /// Read whether sensitive file access is allowed (may change mid-conversation).
    pub fn sensitive_files_allowed(&self) -> bool {
        self.shared_permissions.sensitive_files_allowed()
    }

    /// Returns true if the action is unconditionally allowed without broker involvement.
    /// ManualEdit writes and ManualEdit/AutoEdit commands return false — broker required.
    pub fn check_permission(&self, action: &Action) -> bool {
        let perm = self.permissions();
        match (&perm, action) {
            // Chat: read-only, hard deny everything else
            (PermissionLevel::Chat, Action::Read) => true,
            (PermissionLevel::Chat, _) => false,
            // ManualEdit: reads free; writes + exec need broker
            (PermissionLevel::ManualEdit, Action::Read) => true,
            (PermissionLevel::ManualEdit, _) => false,
            // AutoEdit: reads + writes free; exec needs broker
            (PermissionLevel::AutoEdit, Action::Execute) => false,
            (PermissionLevel::AutoEdit, _) => true,
            // FullAuto: everything free
            (PermissionLevel::FullAuto, _) => true,
        }
    }

    /// Returns true if this mode routes writes through the broker (i.e. ManualEdit).
    pub fn needs_write_approval(&self) -> bool {
        self.permissions() == PermissionLevel::ManualEdit
    }

    /// Returns true if this mode routes commands through the broker (ManualEdit or AutoEdit).
    pub fn needs_exec_approval(&self) -> bool {
        matches!(
            self.permissions(),
            PermissionLevel::ManualEdit | PermissionLevel::AutoEdit
        )
    }
}

#[async_trait]
pub trait ToolExecutor: Send + Sync {
    fn definitions(&self) -> Vec<ToolDef>;
    async fn execute(&self, name: &str, tool_use_id: &str, params: Value, context: &ToolContext) -> Result<ToolOutput>;
}

/// Built-in tool executor combining all built-in tools.
pub struct BuiltinTools;

impl BuiltinTools {
    pub fn new() -> Self {
        Self
    }

    /// Returns true if `name` is a built-in tool handled by this executor.
    /// Non-builtin tool calls are routed to the MCP manager.
    pub fn is_builtin(name: &str) -> bool {
        matches!(
            name,
            "read_file"
                | "create_file"
                | "edit_file"
                | "apply_patch"
                | "list_directory"
                | "run_command"
                | "read_terminal_output"
                | "kill_terminal"
                | "grep_search"
                | "read_skill"
                | "chat_message"
                | "todo_write"
                | "spawn_subagent"
                | "list_active_agents"
                | "wait_for_subagents"
                | "report_blocked_write"
                | "complete_task"
        )
    }

    /// Returns true if the tool is read-only and safe to run concurrently with other read-only tools.
    /// Write/execute tools return false and will be executed sequentially after all read-only tools finish.
    pub fn is_read_only(name: &str) -> bool {
        matches!(
            name,
            "read_file"
                | "list_directory"
                | "grep_search"
                | "read_skill"
                | "list_active_agents"
                | "report_blocked_write"
                | "todo_write"
                | "read_terminal_output"
        )
    }
}

#[async_trait]
impl ToolExecutor for BuiltinTools {
    fn definitions(&self) -> Vec<ToolDef> {
        let mut defs = Vec::new();
        defs.extend(file_ops::definitions());
        defs.extend(terminal::definitions());
        defs.extend(search::definitions());
        defs.extend(skill_tools::definitions());
        defs.extend(workflow_tools::definitions());
        defs.extend(ask_user::definitions());
        defs.extend(todo_tools::definitions());
        defs.extend(subagent_tools::definitions());
        defs.extend(complete_task::definitions());
        defs
    }

    async fn execute(&self, name: &str, tool_use_id: &str, params: Value, context: &ToolContext) -> Result<ToolOutput> {
        match name {
            "read_file" | "create_file" | "edit_file" | "apply_patch"
            | "list_directory" => {
                file_ops::execute(name, params, context).await
            }
            "run_command" | "read_terminal_output" | "kill_terminal" => {
                terminal::execute(name, tool_use_id, params, context).await
            }
            "grep_search" => search::execute(name, tool_use_id, params, context).await,
            "read_skill" => skill_tools::execute(name, params, context).await,
            "read_workflow" => workflow_tools::execute(name, params, context).await,
            "chat_message" => ask_user::execute(name, params, context).await,
            "todo_write" => todo_tools::execute(name, params, context).await,
            "spawn_subagent" | "list_active_agents" | "wait_for_subagents"
            | "report_blocked_write" => {
                subagent_tools::execute(name, params, context).await
            }
            "complete_task" => complete_task::execute(name, params, context).await,
            _ => Ok(ToolOutput {
                content: format!("Unknown tool: {}", name),
                is_error: true,
            }),
        }
    }
}


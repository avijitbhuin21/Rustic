pub mod ask_user;
pub mod complete_task;
pub mod file_ops;
pub mod orchestrator_tools;
pub mod skill_tools;
pub mod subagent_tools;
pub mod terminal;
pub mod search;
pub mod todo_tools;
pub mod web_tools;
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
use crate::task::orchestrator_host::OrchestratorHost;
use crate::task::permission_broker::PermissionBroker;
use crate::task::permissions::{Action, PermissionLevel, SharedPermissions};
use crate::task::terminal_broker::AgentTerminals;
use crate::task::user_question_broker::UserQuestionBroker;
use crate::task::EventTx;
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutput {
    pub content: String,
    pub is_error: bool,
}

/// Per-task record of files that `read_file` has already returned to the model.
/// Used to short-circuit a repeat read of an unchanged file with a FILE_UNCHANGED
/// stub so the model cites the earlier `read_file` tool_result instead of paying
/// to re-fetch the same bytes.
#[derive(Default)]
pub struct FileReadRegistry {
    /// Map from canonical absolute path → the cumulative range the model has
    /// already seen for that file, plus the mtime it was seen at. If the current
    /// mtime still matches and the new request fits inside that range, we stub.
    entries: Mutex<std::collections::HashMap<PathBuf, FileReadEntry>>,
}

#[derive(Clone)]
struct FileReadEntry {
    mtime: std::time::SystemTime,
    /// Non-overlapping, sorted list of (start, end) ranges (1-indexed, inclusive)
    /// that the model has been shown across prior reads. Tracked as explicit
    /// intervals — not a min/max pair — so two disjoint reads (e.g. lines 1-100
    /// and 200-300) don't falsely claim coverage of the gap in between.
    intervals: Vec<(usize, usize)>,
}

/// Merge `(start, end)` into `intervals`, keeping the list sorted and
/// non-overlapping. Ranges that touch or overlap coalesce; disjoint ranges
/// stay separate.
fn merge_interval(intervals: &mut Vec<(usize, usize)>, start: usize, end: usize) {
    intervals.push((start, end));
    intervals.sort_by_key(|(s, _)| *s);
    let mut merged: Vec<(usize, usize)> = Vec::with_capacity(intervals.len());
    for (s, e) in intervals.drain(..) {
        if let Some(last) = merged.last_mut() {
            // +1 so adjacent ranges (e.g. 1-100 and 101-200) combine cleanly.
            if s <= last.1.saturating_add(1) {
                last.1 = last.1.max(e);
                continue;
            }
        }
        merged.push((s, e));
    }
    *intervals = merged;
}

fn covered_by(intervals: &[(usize, usize)], start: usize, end: usize) -> bool {
    intervals.iter().any(|(s, e)| *s <= start && end <= *e)
}

impl FileReadRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Return true if `(path, start, end)` with the given current mtime is
    /// already fully covered by a prior read — in which case the caller should
    /// return a FILE_UNCHANGED stub instead of re-reading.
    pub fn already_covered(
        &self,
        path: &std::path::Path,
        start: usize,
        end: usize,
        current_mtime: std::time::SystemTime,
    ) -> bool {
        let Ok(entries) = self.entries.lock() else { return false };
        let Some(entry) = entries.get(path) else { return false };
        entry.mtime == current_mtime && covered_by(&entry.intervals, start, end)
    }

    /// Record a completed read so future identical (or narrower) reads of the
    /// same file, at the same mtime, can short-circuit.
    pub fn record(
        &self,
        path: PathBuf,
        mtime: std::time::SystemTime,
        start: usize,
        end: usize,
    ) {
        let Ok(mut entries) = self.entries.lock() else { return };
        entries
            .entry(path)
            .and_modify(|e| {
                if e.mtime != mtime {
                    // File changed on disk — discard the old coverage, start fresh.
                    e.mtime = mtime;
                    e.intervals.clear();
                }
                merge_interval(&mut e.intervals, start, end);
            })
            .or_insert_with(|| {
                let mut intervals = Vec::new();
                merge_interval(&mut intervals, start, end);
                FileReadEntry { mtime, intervals }
            });
    }

    /// Drop any cached record for `path` — call this whenever a write tool
    /// modifies the file so the next read doesn't falsely stub on stale mtime.
    pub fn invalidate(&self, path: &std::path::Path) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.remove(path);
        }
    }
}

/// Callback type for computing the task diff at completion time.
/// Captures the task_id and DB reference.
pub type ComputeDiffFn = Arc<dyn Fn() -> TaskDiff + Send + Sync>;

/// Context available to tool execution.
pub struct ToolContext {
    pub project_root: PathBuf,
    /// Shared permissions — updated in real-time when the user changes permission mode mid-conversation.
    pub shared_permissions: SharedPermissions,
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
    /// Per-file lock registry — all write tools acquire a per-path lock before modifying files.
    pub file_lock: Arc<FileLockRegistry>,
    /// Per-task registry of files already read in this conversation. Lets
    /// `read_file` short-circuit unchanged re-reads with a FILE_UNCHANGED stub.
    /// Sub-agents get a fresh registry because their conversation context is
    /// independent of the parent's; stubbing against the parent's prior reads
    /// would point at a tool_result the sub-agent never saw.
    pub file_read_registry: Arc<FileReadRegistry>,
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
    /// Agent-level tool configuration (web_search backend + API keys,
    /// web_fetch toggle). Consumed by client-side web tools.
    pub tool_config: Arc<crate::config::ToolConfig>,
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
    /// True when this task runs in the Global orchestrator scope
    /// (project_id == GLOBAL_PROJECT_ID). Gates cross-project tools and
    /// forces read-only file access.
    pub is_global: bool,
    /// Host-supplied surface for orchestrator operations (spawn_subtask,
    /// list_tasks_across_projects, read_task_history). `None` in unit tests
    /// and outside the Global scope.
    pub orchestrator_host: Option<Arc<dyn OrchestratorHost>>,
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
                | "glob"
                | "read_skill"
                | "chat_message"
                | "todo_write"
                | "spawn_subagent"
                | "list_active_agents"
                | "wait_for_subagents"
                | "report_blocked_write"
                | "complete_task"
                | "list_projects"
                | "list_tasks_across_projects"
                | "read_task_history"
                | "spawn_subtask"
                | "web_search"
                | "web_fetch"
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
                | "glob"
                | "read_skill"
                | "list_active_agents"
                | "report_blocked_write"
                | "todo_write"
                | "read_terminal_output"
                | "list_projects"
                | "list_tasks_across_projects"
                | "read_task_history"
                | "web_search"
                | "web_fetch"
        )
    }

    /// Like `definitions()` but constrains the `run_command` tool's `shell`
    /// parameter to the set of shells the host has confirmed are installed.
    /// Prefer this from the executor — it prevents the model from asking for
    /// a shell that would fail to spawn.
    pub fn definitions_for_host(&self, available_shells: &[String]) -> Vec<ToolDef> {
        let mut defs = Vec::new();
        defs.extend(file_ops::definitions());
        defs.extend(terminal::definitions(available_shells));
        defs.extend(search::definitions());
        defs.extend(skill_tools::definitions());
        defs.extend(workflow_tools::definitions());
        defs.extend(ask_user::definitions());
        defs.extend(todo_tools::definitions());
        defs.extend(subagent_tools::definitions());
        defs.extend(complete_task::definitions());
        defs.extend(orchestrator_tools::definitions());
        defs
    }
}

#[async_trait]
impl ToolExecutor for BuiltinTools {
    fn definitions(&self) -> Vec<ToolDef> {
        // Trait impl has no host context — use the empty-shell variant which
        // drops the `shell` parameter. Real callers should prefer
        // `definitions_for_host()` so the model sees the host's actual shells.
        self.definitions_for_host(&[])
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
            "grep_search" | "glob" => search::execute(name, tool_use_id, params, context).await,
            "read_skill" => skill_tools::execute(name, params, context).await,
            "read_workflow" => workflow_tools::execute(name, params, context).await,
            "chat_message" => ask_user::execute(name, params, context).await,
            "todo_write" => todo_tools::execute(name, params, context).await,
            "spawn_subagent" | "list_active_agents" | "wait_for_subagents"
            | "report_blocked_write" => {
                subagent_tools::execute(name, params, context).await
            }
            "complete_task" => complete_task::execute(name, params, context).await,
            "list_projects" | "list_tasks_across_projects" | "read_task_history"
            | "spawn_subtask" => {
                orchestrator_tools::execute(name, params, context).await
            }
            "web_search" | "web_fetch" => {
                web_tools::execute(name, tool_use_id, params, context).await
            }
            _ => Ok(ToolOutput {
                content: format!("Unknown tool: {}", name),
                is_error: true,
            }),
        }
    }
}


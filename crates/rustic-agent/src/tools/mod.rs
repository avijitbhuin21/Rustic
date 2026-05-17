pub mod ask_user;
pub mod code_intel;
pub mod file_ops;
pub mod media_tools;
pub mod orchestrator_tools;
pub mod skill_tools;
pub mod subagent_tools;
pub mod terminal;
pub mod search;
pub mod todo_tools;
pub mod web_tools;
pub mod workflow_tools;
pub mod worktree;

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;

use crate::mcp::McpManager;
use crate::provider::{ProviderConfig, ToolDef};
use crate::task::file_lock::FileLockRegistry;
use crate::task::orchestrator_host::OrchestratorHost;
use crate::task::permission_broker::PermissionBroker;
use crate::task::permissions::{Action, PermissionLevel, SharedPermissions};
use crate::task::terminal_broker::AgentTerminals;
use crate::task::EventTx;
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

/// Tool execution output. `attachments` carries binary payloads (PDF, image)
/// alongside the text `content`; empty for text-only tools. Provider layer
/// encodes each attachment as the API's native block shape.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolOutput {
    pub content: String,
    pub is_error: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<ToolAttachment>,
}

impl ToolOutput {
    pub fn text(content: impl Into<String>, is_error: bool) -> Self {
        Self {
            content: content.into(),
            is_error,
            attachments: Vec::new(),
        }
    }
}

/// Discriminated-union of binary payloads a tool can return alongside its text
/// output. Adding a new variant is forward-compatible: providers that don't
/// recognise it fall back to a short text placeholder in tool_result content.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolAttachment {
    /// Raw image bytes (PNG / JPEG / WebP / GIF). `media_type` is the MIME string.
    Image { media_type: String, data: Vec<u8> },
    /// PDF bytes. Anthropic encodes as a native `document` block; other providers fall
    /// back to text-only. `page_count` is informational only.
    Pdf { data: Vec<u8>, page_count: usize },
}

/// Unit of range tracked per file. Keyed by `(PathBuf, ReadUnit)` so a line read
/// and a cell read of the same .ipynb don't falsely stub each other.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum ReadUnit {
    Lines,
    Cells,
}

/// Short-circuits a repeat `read_file` of an unchanged file with a FILE_UNCHANGED stub.
#[derive(Default)]
pub struct FileReadRegistry {
    entries: Mutex<std::collections::HashMap<(PathBuf, ReadUnit), FileReadEntry>>,
}

#[derive(Clone)]
struct FileReadEntry {
    mtime: std::time::SystemTime,
    /// Non-overlapping sorted (start, end) intervals already shown to the model.
    /// Explicit intervals (not a min/max pair) so disjoint reads don't falsely
    /// cover the gap between them.
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

    pub fn already_covered(
        &self,
        path: &std::path::Path,
        unit: ReadUnit,
        start: usize,
        end: usize,
        current_mtime: std::time::SystemTime,
    ) -> bool {
        let Ok(entries) = self.entries.lock() else { return false };
        let key = (path.to_path_buf(), unit);
        let Some(entry) = entries.get(&key) else { return false };
        entry.mtime == current_mtime && covered_by(&entry.intervals, start, end)
    }

    pub fn record(
        &self,
        path: PathBuf,
        unit: ReadUnit,
        mtime: std::time::SystemTime,
        start: usize,
        end: usize,
    ) {
        let Ok(mut entries) = self.entries.lock() else { return };
        entries
            .entry((path, unit))
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

    /// Drop all cached records for `path` across every unit — called after any write.
    pub fn invalidate(&self, path: &std::path::Path) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.retain(|(p, _), _| p != path);
        }
    }
}

/// Incremental message-history persistence callback. Called at every stable
/// point in `run_turn` so process termination doesn't lose mid-turn progress.
pub type PersistMessagesFn = Arc<dyn Fn(&[crate::provider::Message]) + Send + Sync>;

/// Context available to tool execution.
pub struct ToolContext {
    pub project_root: PathBuf,
    pub shared_permissions: SharedPermissions,
    /// `None` for sub-agents / tests; `Some(fn)` for the main task (incremental DB persist).
    pub persist_messages_fn: Option<PersistMessagesFn>,
    pub cancel_token: Option<Arc<AtomicBool>>,
    pub permission_broker: Arc<PermissionBroker>,
    pub event_tx: EventTx,
    pub task_id: String,
    pub file_lock: Arc<FileLockRegistry>,
    /// Sub-agents get a fresh registry — their context is independent of the parent's.
    pub file_read_registry: Arc<FileReadRegistry>,
    pub mcp_manager: Option<Arc<Mutex<McpManager>>>,
    pub mcp_tool_defs: Vec<ToolDef>,
    pub subagent_registry: Arc<crate::task::subagent::SubagentRegistry>,
    /// 0 = main agent, 1 = sub-agent. Sub-agents cannot spawn further sub-agents.
    pub agent_depth: u8,
    pub ai_config: Arc<crate::config::AiConfig>,
    pub tool_config: Arc<crate::config::ToolConfig>,
    /// Paths pre-approved in `.rustic/allowed-files.txt`; skip tier-2/3 confirmation.
    pub allowed_paths: Vec<String>,
    pub parent_provider_config: Option<Arc<ProviderConfig>>,
    /// Cheaper/faster provider for `model_tier: "fast"` spawns; `None` when unconfigured.
    pub subagent_provider_config: Option<Arc<ProviderConfig>>,
    /// `None` = unrestricted (main agent). `Some([])` = read-only sub-agent.
    pub write_scope: Option<Vec<String>>,
    /// Writes blocked by WRITE_SCOPE_VIOLATION; drained into SubagentResult on finish.
    pub blocked_writes: Arc<Mutex<Vec<crate::task::subagent::BlockedWrite>>>,
    /// `None` in unit tests / embedded contexts — tools must handle gracefully.
    pub agent_terminals: Option<Arc<dyn AgentTerminals>>,
    /// True in Global orchestrator scope: gates cross-project tools, forces read-only FS.
    pub is_global: bool,
    /// When true, write/execute tools are rejected; agent can still read and propose.
    pub is_plan_mode: bool,
    /// Concurrent-stream cap + daily cost ceiling; sub-agents share the parent's handle.
    pub budget: crate::budget::Budget,
    /// Sub-agents share the parent's broker so child ask_user surfaces in the same dialog.
    pub ask_user_broker: std::sync::Arc<crate::task::ask_user_broker::AskUserBroker>,
    /// Parks executor when daily ceiling is hit; sub-agents share the parent's handle.
    pub ceiling_broker: std::sync::Arc<crate::task::ceiling_broker::CeilingBroker>,
    /// `None` in unit tests and outside Global scope.
    pub orchestrator_host: Option<Arc<dyn OrchestratorHost>>,
    /// `None` disables tracking. Edit tools call `capture`; bash enqueues `SweepJob`.
    pub file_history: Option<Arc<crate::file_history::FileHistory>>,
    pub sweep_worker: Option<Arc<crate::file_history::SweepWorker>>,
    /// Snapshot anchor: captures/sweeps in this turn land under this message id.
    pub current_user_message_id: Option<String>,
    /// Accumulates non-token media-tool costs; drained into TaskCost after each batch.
    pub tool_cost_sink: Arc<Mutex<f64>>,
    /// Per-project shared services (parsers, symbol index). Deduped by WorkspaceRegistry.
    pub workspace_services: Arc<crate::workspace::WorkspaceServices>,
    /// Shared registry so worktree-override spawns look up the right WorkspaceServices.
    pub workspace_registry: Arc<crate::workspace::WorkspaceRegistry>,
    /// `Some((parent_task_id, agent_id))` for sub-agents; `None` for the main agent.
    pub subagent_self: Option<(String, String)>,
    /// Deferred tools already loaded via `tool_search`; shared with sub-agents.
    pub loaded_deferred_tools: Arc<Mutex<std::collections::HashSet<String>>>,
}

impl ToolContext {
    pub fn emit_progress(&self, tool_use_id: &str, progress_text: &str) {
        let _ = self.event_tx.try_send(crate::task::TaskEvent::ToolProgress {
            task_id: self.task_id.clone(),
            tool_use_id: tool_use_id.to_string(),
            progress_text: progress_text.to_string(),
        });
    }

    pub fn permissions(&self) -> PermissionLevel {
        self.shared_permissions.level()
    }

    pub fn sensitive_files_allowed(&self) -> bool {
        self.shared_permissions.sensitive_files_allowed()
    }

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

    pub fn needs_write_approval(&self) -> bool {
        self.permissions() == PermissionLevel::ManualEdit
    }

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
                | "todo_write"
                | "spawn_subagent"
                | "list_subagents"
                | "report_blocked_write"
                | "send_message"
                | "nudge_subagent"
                | "stop_subagent"
                | "list_projects"
                | "list_tasks_across_projects"
                | "read_task_history"
                | "spawn_subtask"
                | "web_search"
                | "web_fetch"
                | "image_create"
                | "video_create"
                | "animate"
                | "find_symbol"
                | "goto_definition"
                | "find_references"
                | "outline"
                | "call_sites"
                | "enter_worktree"
                | "exit_worktree"
                | "list_worktrees"
                | "tool_search"
                | "goal_complete"
        )
    }

    pub fn is_read_only(name: &str) -> bool {
        matches!(
            name,
            "read_file"
                | "list_directory"
                | "grep_search"
                | "glob"
                | "read_skill"
                | "list_subagents"
                | "report_blocked_write"
                | "todo_write"
                | "read_terminal_output"
                | "list_projects"
                | "list_tasks_across_projects"
                | "read_task_history"
                | "web_search"
                | "web_fetch"
                | "ask_user"
                | "find_symbol"
                | "goto_definition"
                | "find_references"
                | "outline"
                | "call_sites"
                | "tool_search"
                | "goal_complete"
                | "list_worktrees"
        )
    }

    /// Like `definitions()` but constrains `run_command`'s `shell` to installed shells
    /// and conditionally exposes `model_tier` on `spawn_subagent` when `fast_subagent_model` is set.
    pub fn definitions_for_host(
        &self,
        available_shells: &[String],
        fast_subagent_model: Option<&str>,
    ) -> Vec<ToolDef> {
        let mut defs = Vec::new();
        defs.extend(file_ops::definitions());
        defs.extend(terminal::definitions(available_shells));
        defs.extend(search::definitions());
        defs.extend(skill_tools::definitions());
        defs.extend(workflow_tools::definitions());
        defs.extend(todo_tools::definitions());
        defs.extend(subagent_tools::definitions(fast_subagent_model));
        defs.extend(orchestrator_tools::definitions());
        defs.extend(ask_user::definitions());
        defs.extend(code_intel::definitions());
        defs.extend(worktree::definitions());
        defs.extend(goal_loop_def());
        defs.push(crate::task::tool_search::tool_search_def());
        defs
    }
}

fn goal_loop_def() -> Vec<ToolDef> {
    vec![ToolDef {
        name: "goal_complete".into(),
        description: "Signal that the current /goal objective has been achieved. Call this \
                      ONLY when the user's stated goal is fully done — not when you're \
                      ending a single turn. Optional `summary` (short prose describing \
                      what was achieved) is shown to the user as the goal-loop's final \
                      result. Outside of /goal mode this tool is a no-op informational \
                      stub."
            .into(),
        parameters: json!({
            "type": "object",
            "properties": {
                "summary": {
                    "type": "string",
                    "description": "Optional short summary of what was achieved. Shown to \
                                    the user when the goal-loop terminates."
                }
            }
        }),
    }]
}

#[async_trait]
impl ToolExecutor for BuiltinTools {
    fn definitions(&self) -> Vec<ToolDef> {
        // Prefer `definitions_for_host()` in production — this variant omits shells.
        self.definitions_for_host(&[], None)
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
            "todo_write" => todo_tools::execute(name, params, context).await,
            "spawn_subagent" | "list_subagents" | "report_blocked_write"
            | "send_message" | "nudge_subagent" | "stop_subagent"
            | "wait_for_subagents" => { // legacy name — handler returns a clear removal message
                subagent_tools::execute(name, params, context).await
            }
            "list_projects" | "list_tasks_across_projects" | "read_task_history"
            | "spawn_subtask" => {
                orchestrator_tools::execute(name, params, context).await
            }
            "web_search" | "web_fetch" => {
                web_tools::execute(name, tool_use_id, params, context).await
            }
            "image_create" | "video_create" | "animate" => {
                media_tools::execute(name, tool_use_id, params, context).await
            }
            "ask_user" => ask_user::execute(params, context).await,
            "find_symbol" | "goto_definition" | "find_references" | "outline"
            | "call_sites" => code_intel::execute(name, params, context).await,
            "enter_worktree" | "exit_worktree" | "list_worktrees" => worktree::execute(name, params, context).await,
            "tool_search" => crate::task::tool_search::execute(params, context).await,
            "goal_complete" => {
                let summary = params
                    .get("summary")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim();
                let body = if summary.is_empty() {
                    "goal_complete recorded. (Outside /goal mode this is a no-op; the \
                     task simply continues. End your turn with a plain-text summary if \
                     you're done.)"
                        .to_string()
                } else {
                    format!(
                        "goal_complete recorded with summary: {}. (Outside /goal mode this \
                         is a no-op; the task simply continues.)",
                        summary
                    )
                };
                Ok(ToolOutput { content: body, is_error: false , attachments: Vec::new() })
            }
            _ => Ok(ToolOutput {
                content: format!("Unknown tool: {}", name),
                is_error: true,
                attachments: Vec::new(),
            }),
        }
    }
}


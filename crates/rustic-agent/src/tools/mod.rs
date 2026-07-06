pub mod ask_user;
pub mod code_intel;
pub mod extension_tools;
pub mod file_ops;
pub mod history;
pub mod media_tools;
pub mod notebook;
pub mod patch;
pub mod search;
pub mod skill_tools;
pub mod subagent_tools;
pub mod terminal;
pub mod todo_tools;
pub mod web_tools;
pub mod workflow_tools;

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;

use crate::mcp::McpManager;
use crate::provider::{ProviderConfig, ToolDef};
use crate::task::file_lock::FileLockRegistry;
use crate::task::permission_broker::PermissionBroker;
use crate::task::permissions::{Action, PermissionLevel, SharedPermissions};
use crate::task::terminal_broker::AgentTerminals;
use crate::task::EventTx;
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

/// Accept a JSON value as a boolean across the many shapes models actually
/// produce — real `true`/`false`, the strings `"true"`/`"false"`/`"1"`/`"0"`/
/// `"yes"`/`"no"` (case-insensitive), and numbers (non-zero → true). Returns
/// false for anything else (including null/missing). Use this for any optional
/// boolean tool argument; relying on `as_bool()` alone silently mis-parses
/// stringified bools.
pub(crate) fn coerce_bool(v: &Value) -> bool {
    if let Some(b) = v.as_bool() {
        return b;
    }
    if let Some(s) = v.as_str() {
        let trimmed = s.trim().to_ascii_lowercase();
        return matches!(trimmed.as_str(), "true" | "1" | "yes" | "y" | "on");
    }
    if let Some(n) = v.as_i64() {
        return n != 0;
    }
    if let Some(n) = v.as_f64() {
        return n != 0.0;
    }
    false
}

/// Accept a batch field as either a real JSON array OR a JSON-stringified
/// array. Some tool-calling models (notably Claude Haiku and GPT-4-class
/// models under certain prompts) serialize nested arrays as escaped strings
/// rather than honoring the schema's `"type": "array"`. Silently coercing at
/// the boundary is cheaper than asking the model to retry the call.
pub(crate) fn coerce_batch_array(v: Option<&Value>) -> Option<Vec<Value>> {
    let v = v?;
    if let Some(arr) = v.as_array() {
        return Some(arr.clone());
    }
    if let Some(s) = v.as_str() {
        if let Ok(parsed) = serde_json::from_str::<Value>(s) {
            if let Some(arr) = parsed.as_array() {
                return Some(arr.clone());
            }
        }
    }
    None
}

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

/// Per-category bucket for non-token tool costs (currently media generation:
/// image / video). Drained at the end of each turn into `TaskCost` so the chat
/// header reports image vs video spend separately rather than rolling both
/// into one opaque "tool cost" number.
#[derive(Debug, Clone, Default)]
pub struct ToolCostBucket {
    pub image_usd: f64,
    pub video_usd: f64,
}

impl ToolCostBucket {
    pub fn total(&self) -> f64 {
        self.image_usd + self.video_usd
    }

    /// Reset to zero and return the previous contents.
    pub fn take(&mut self) -> ToolCostBucket {
        std::mem::take(self)
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
        let Ok(entries) = self.entries.lock() else {
            return false;
        };
        let key = (path.to_path_buf(), unit);
        let Some(entry) = entries.get(&key) else {
            return false;
        };
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
        let Ok(mut entries) = self.entries.lock() else {
            return;
        };
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

    /// Drop ALL coverage. Called after context condensation — most previous
    /// read results no longer exist in the condensed history, so coverage-based
    /// FILE_UNCHANGED stubs would refuse reads of content the model can no
    /// longer see.
    pub fn clear(&self) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.clear();
        }
    }

    /// Check if a file has been read at all (any range, any unit) in this task.
    pub fn has_been_read(&self, path: &std::path::Path) -> bool {
        if let Ok(entries) = self.entries.lock() {
            entries.keys().any(|(p, _)| p == path)
        } else {
            false
        }
    }
}

/// Incremental message-history persistence callback. Called at every stable
/// point in `run_turn` so process termination doesn't lose mid-turn progress.
pub type PersistMessagesFn = Arc<dyn Fn(&[crate::provider::Message]) + Send + Sync>;

/// One message dropped from live context by a condense pass, as stored in the
/// host's archive. `generation` counts condense passes (1-based); `slot` is
/// the message's position within its generation.
#[derive(Debug, Clone)]
pub struct ArchivedMessage {
    pub generation: i64,
    pub slot: i64,
    pub role: String,
    pub content_json: String,
}

/// Host-backed archive of messages dropped by context condensing. `append`
/// stores one condense generation; `fetch_all` returns every archived message
/// for the task ordered by (generation, slot). `None` on ToolContext (tests,
/// sub-agents) disables archiving.
pub trait ConversationArchive: Send + Sync {
    fn append(&self, task_id: &str, rows: &[(String, String)]);
    fn fetch_all(&self, task_id: &str) -> Vec<ArchivedMessage>;
}

/// Context available to tool execution.
pub struct ToolContext {
    pub project_root: PathBuf,
    pub shared_permissions: SharedPermissions,
    /// `None` for sub-agents / tests; `Some(fn)` for the main task (incremental DB persist).
    pub persist_messages_fn: Option<PersistMessagesFn>,
    /// Archive for messages dropped by condense; `None` for sub-agents / tests.
    pub conversation_archive: Option<Arc<dyn ConversationArchive>>,
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
    /// Provider-type key for the parent's model (e.g. "FreeBuff", "Claude",
    /// "Compatible:slug", "OpenRouter"). Sub-agents build their provider from
    /// THIS, not from the model id — `vendor/model` ids (FreeBuff, OpenRouter,
    /// Compatible) are indistinguishable by name, so name-guessing mis-routed
    /// FreeBuff sub-agents into the Compatible/OpenAI path (→ 401, no key).
    pub parent_provider_type: Option<String>,
    /// Provider-type key for the `model_tier:"fast"` sub-agent provider.
    pub subagent_provider_type: Option<String>,
    /// `None` = unrestricted (main agent). `Some([])` = read-only sub-agent.
    pub write_scope: Option<Vec<String>>,
    /// Writes blocked by WRITE_SCOPE_VIOLATION; drained into SubagentResult on finish.
    pub blocked_writes: Arc<Mutex<Vec<crate::task::subagent::BlockedWrite>>>,
    /// `None` in unit tests / embedded contexts — tools must handle gracefully.
    pub agent_terminals: Option<Arc<dyn AgentTerminals>>,
    /// When true, write/execute tools are rejected; agent can still read and propose.
    pub is_plan_mode: bool,
    /// Concurrent-stream cap + daily cost ceiling; sub-agents share the parent's handle.
    pub budget: crate::budget::Budget,
    /// Sub-agents share the parent's broker so child ask_user surfaces in the same dialog.
    pub ask_user_broker: std::sync::Arc<crate::task::ask_user_broker::AskUserBroker>,
    /// `Some(slot)` switches `ask_user` from "block on the broker" to "suspend
    /// the turn": the executor captures the call into the slot WITHOUT
    /// executing it (no tool_result is appended), persists the dangling
    /// tool_use, and ends the turn early. The host inspects the slot after
    /// `run_turn` returns and is responsible for delivering the questions
    /// (e.g. as a GitHub issue comment) and later resuming the task with a
    /// tool_result for the captured id carrying the user's answer. `None`
    /// (the default everywhere except GitHub-issue tasks) keeps the
    /// interactive blocking behavior.
    pub ask_user_suspend: Option<Arc<Mutex<Option<crate::task::SuspendedAskUser>>>>,
    /// Parks executor when daily ceiling is hit; sub-agents share the parent's handle.
    pub ceiling_broker: std::sync::Arc<crate::task::ceiling_broker::CeilingBroker>,
    /// `None` disables tracking. Edit tools call `capture`; bash enqueues `SweepJob`.
    pub file_history: Option<Arc<crate::file_history::FileHistory>>,
    pub sweep_worker: Option<Arc<crate::file_history::SweepWorker>>,
    /// Readiness signal for the deferred (background) baseline snapshot. The
    /// executor awaits this before the first file-mutating tool so the baseline
    /// row exists before any `capture`/sweep. `None` ⇒ no deferral (treat as
    /// ready) — used by legacy/embedded callers that build the context directly.
    pub baseline_gate: Option<crate::file_history::BaselineGate>,
    /// Snapshot anchor: captures/sweeps in this turn land under this message id.
    pub current_user_message_id: Option<String>,
    /// Accumulates non-token media-tool costs; drained into TaskCost after each batch.
    /// Categorised so the chat header can show image vs video generation cost
    /// separately rather than rolling everything into one "tool cost" number.
    pub tool_cost_sink: Arc<Mutex<ToolCostBucket>>,
    /// Per-project shared services (parsers, symbol index). Deduped by WorkspaceRegistry.
    pub workspace_services: Arc<crate::workspace::WorkspaceServices>,
    /// Shared registry so worktree-override spawns look up the right WorkspaceServices.
    pub workspace_registry: Arc<crate::workspace::WorkspaceRegistry>,
    /// `Some((parent_task_id, agent_id))` for sub-agents; `None` for the main agent.
    pub subagent_self: Option<(String, String)>,
    /// Live copy of the agent's todo list, written through by `todo_write`.
    /// The executor reads it to (a) reinject the list as a periodic anchor
    /// message on long sessions and (b) preserve it verbatim through context
    /// condensing. Sub-agents get a fresh slot — their list is their own.
    pub current_todos: Arc<Mutex<Vec<crate::task::TodoItem>>>,
    /// Deferred tools already loaded via `tool_search`; shared with sub-agents.
    pub loaded_deferred_tools: Arc<Mutex<std::collections::HashSet<String>>>,
    /// Live /goal slot: `Some(slot)` on main-agent contexts; the executor
    /// re-arms the turn loop while the slot holds an active goal. `None` for
    /// sub-agents and tests (goal loops never nest).
    pub goal_state: Option<crate::task::goal::GoalSlot>,
    /// Per-task deferred tool table (P1.7). Written by the executor's tool
    /// partition step each turn; read by `tool_search`. Lives on the context
    /// so concurrent tasks with different tool pools (e.g. different MCP
    /// servers) can't overwrite each other — was a process-global static.
    pub deferred_tools: Arc<Mutex<Vec<crate::provider::ToolDef>>>,
    /// Snapshot of the parent agent's message history at the start of the
    /// current turn. Populated by `TaskExecutor::run_turn` right before tool
    /// dispatch so `spawn_subagent` can inherit it into the child's initial
    /// context (lets the child skip re-reading files the parent already
    /// loaded). Empty Vec for sub-agents — they don't pass context further
    /// because sub-agents can't spawn sub-agents anyway. Read-only after the
    /// executor updates it.
    pub parent_message_snapshot: Arc<Mutex<Vec<crate::provider::Message>>>,
}

impl ToolContext {
    pub fn emit_progress(&self, tool_use_id: &str, progress_text: &str) {
        let _ = self
            .event_tx
            .try_send(crate::task::TaskEvent::ToolProgress {
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

    /// Test-only factory: fully-populated permissive context (FullAuto, no
    /// MCP, no file history) rooted at `project_root`. Returns the context
    /// plus the event receiver so tests can assert on emitted TaskEvents.
    #[cfg(test)]
    pub fn new_test(
        project_root: PathBuf,
    ) -> (Self, tokio::sync::mpsc::Receiver<crate::task::TaskEvent>) {
        let (event_tx, event_rx) = tokio::sync::mpsc::channel(crate::task::EVENT_CHANNEL_CAP);
        let workspace_registry = Arc::new(crate::workspace::WorkspaceRegistry::new());
        let workspace_services = workspace_registry.get_or_create(&project_root);
        let ctx = Self {
            project_root,
            shared_permissions: SharedPermissions::new(PermissionLevel::FullAuto, true),
            persist_messages_fn: None,
            conversation_archive: None,
            cancel_token: None,
            permission_broker: Arc::new(PermissionBroker::new()),
            event_tx,
            task_id: "test-task".to_string(),
            file_lock: FileLockRegistry::new(),
            file_read_registry: Arc::new(FileReadRegistry::new()),
            mcp_manager: None,
            mcp_tool_defs: Vec::new(),
            subagent_registry: crate::task::subagent::SubagentRegistry::new(),
            agent_depth: 0,
            ai_config: Arc::new(crate::config::AiConfig::default()),
            tool_config: Arc::new(crate::config::ToolConfig::default()),
            allowed_paths: Vec::new(),
            parent_provider_config: None,
            subagent_provider_config: None,
            parent_provider_type: None,
            subagent_provider_type: None,
            write_scope: None,
            blocked_writes: Arc::new(Mutex::new(Vec::new())),
            agent_terminals: None,
            is_plan_mode: false,
            budget: crate::budget::Budget::new(&crate::budget::BudgetSettings::default()),
            ask_user_broker: Arc::new(crate::task::ask_user_broker::AskUserBroker::new()),
            ask_user_suspend: None,
            ceiling_broker: Arc::new(crate::task::ceiling_broker::CeilingBroker::new()),
            file_history: None,
            sweep_worker: None,
            baseline_gate: None,
            current_user_message_id: None,
            tool_cost_sink: Arc::new(Mutex::new(ToolCostBucket::default())),
            workspace_services,
            workspace_registry,
            subagent_self: None,
            current_todos: Arc::new(Mutex::new(Vec::new())),
            goal_state: None,
            loaded_deferred_tools: Arc::new(Mutex::new(std::collections::HashSet::new())),
            deferred_tools: Arc::new(Mutex::new(Vec::new())),
            parent_message_snapshot: Arc::new(Mutex::new(Vec::new())),
        };
        (ctx, event_rx)
    }
}

#[async_trait]
pub trait ToolExecutor: Send + Sync {
    fn definitions(&self) -> Vec<ToolDef>;
    async fn execute(
        &self,
        name: &str,
        tool_use_id: &str,
        params: Value,
        context: &ToolContext,
    ) -> Result<ToolOutput>;
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
                | "move_file"
                | "apply_patch"
                | "edit_notebook"
                | "list_directory"
                | "run_command"
                | "read_terminal_output"
                | "kill_terminal"
                | "list_all_terminals"
                | "grep_search"
                | "glob"
                | "read_skill"
                | "read_workflow"
                | "install_extension"
                | "add_mcp_server"
                | "uninstall_extension"
                | "todo_write"
                | "spawn_subagent"
                | "list_subagents"
                | "check_subagent"
                | "report_blocked_write"
                | "escalate_question"
                | "send_message"
                | "nudge_subagent"
                | "stop_subagent"
                | "web_search"
                | "web_fetch"
                | "image_create"
                | "video_create"
                | "animate"
                | "ask_user"
                | "find_symbol"
                | "goto_definition"
                | "find_references"
                | "outline"
                | "call_sites"
                | "tool_search"
                | "search_history"
                | "read_history"
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
                | "search_history"
                | "read_history"
                | "list_subagents"
                | "check_subagent"
                | "report_blocked_write"
                | "escalate_question"
                | "todo_write"
                | "read_terminal_output"
                | "list_all_terminals"
                | "web_search"
                | "web_fetch"
                | "ask_user"
                | "find_symbol"
                | "goto_definition"
                | "find_references"
                | "outline"
                | "call_sites"
                | "tool_search"
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
        defs.extend(patch::definitions());
        defs.extend(notebook::definitions());
        defs.extend(terminal::definitions(available_shells));
        defs.extend(search::definitions());
        defs.extend(skill_tools::definitions());
        defs.extend(workflow_tools::definitions());
        defs.extend(extension_tools::definitions());
        defs.extend(todo_tools::definitions());
        defs.extend(subagent_tools::definitions(fast_subagent_model));
        defs.extend(ask_user::definitions());
        defs.extend(code_intel::definitions());
        defs.extend(history::definitions());
        defs.push(crate::task::tool_search::tool_search_def());
        defs
    }
}

#[async_trait]
impl ToolExecutor for BuiltinTools {
    fn definitions(&self) -> Vec<ToolDef> {
        // Prefer `definitions_for_host()` in production — this variant omits shells.
        self.definitions_for_host(&[], None)
    }

    async fn execute(
        &self,
        name: &str,
        tool_use_id: &str,
        params: Value,
        context: &ToolContext,
    ) -> Result<ToolOutput> {
        match name {
            "read_file" | "create_file" | "edit_file" | "move_file" | "list_directory" => {
                file_ops::execute(name, params, context).await
            }
            "apply_patch" => patch::execute(params, context).await,
            "edit_notebook" => notebook::execute(params, context).await,
            "run_command" | "read_terminal_output" | "kill_terminal" | "list_all_terminals" => {
                terminal::execute(name, tool_use_id, params, context).await
            }
            "grep_search" | "glob" => search::execute(name, tool_use_id, params, context).await,
            "read_skill" => skill_tools::execute(name, params, context).await,
            "read_workflow" => workflow_tools::execute(name, params, context).await,
            "install_extension" | "add_mcp_server" | "uninstall_extension" => {
                extension_tools::execute(name, params, context).await
            }
            "todo_write" => todo_tools::execute(name, params, context).await,
            "spawn_subagent"
            | "list_subagents"
            | "check_subagent"
            | "report_blocked_write"
            | "send_message"
            | "nudge_subagent"
            | "stop_subagent"
            | "escalate_question"
            | "wait_for_subagents" => {
                // legacy name — handler returns a clear removal message
                subagent_tools::execute(name, params, context).await
            }
            "web_search" | "web_fetch" => {
                web_tools::execute(name, tool_use_id, params, context).await
            }
            "image_create" | "video_create" | "animate" => {
                media_tools::execute(name, tool_use_id, params, context).await
            }
            "ask_user" => ask_user::execute(params, context).await,
            "find_symbol" | "goto_definition" | "find_references" | "outline" | "call_sites" => {
                code_intel::execute(name, params, context).await
            }
            "search_history" | "read_history" => history::execute(name, params, context).await,
            "tool_search" => crate::task::tool_search::execute(params, context).await,
            _ => Ok(ToolOutput {
                content: format!("Unknown tool: {}", name),
                is_error: true,
                attachments: Vec::new(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ask_user_is_builtin() {
        assert!(
            BuiltinTools::is_builtin("ask_user"),
            "ask_user should be recognized as a built-in tool"
        );
    }

    #[test]
    fn test_all_core_builtins() {
        let tools = vec![
            "read_file",
            "create_file",
            "edit_file",
            "list_directory",
            "grep_search",
            "glob",
            "run_command",
            "read_terminal_output",
            "list_all_terminals",
            "kill_terminal",
            "ask_user",
            "find_symbol",
            "goto_definition",
            "find_references",
            "outline",
            "call_sites",
        ];
        for tool in tools {
            assert!(
                BuiltinTools::is_builtin(tool),
                "{} should be built-in",
                tool
            );
        }
    }
}

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

/// Tool execution output.
///
/// **Shape.** Every existing tool returns plain text via `content` + an
/// error flag — that's what providers' `tool_result` blocks have
/// historically carried. C9.3 added an `attachments` field so tools
/// that work with binary formats (PDFs, images, future formats) can
/// surface the raw bytes alongside the text summary without
/// stringifying through base64-in-`content`. When `attachments` is
/// empty the output behaves exactly as before — that's the default
/// for every text tool. When non-empty, the provider layer is
/// responsible for encoding each attachment as the API's native block
/// shape (Anthropic `document` / `image`, OpenAI image part, etc.).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolOutput {
    pub content: String,
    pub is_error: bool,
    /// C9.3: optional binary payloads to surface alongside `content`.
    /// Empty for text-only tools (the common case). PDF / image / future
    /// binary-format tools populate this; the provider layer encodes
    /// each variant as the API's native attachment block. The text in
    /// `content` is always the primary signal — `attachments` is the
    /// richer-than-text companion, NOT a replacement.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<ToolAttachment>,
}

impl ToolOutput {
    /// C9.3 convenience constructor for the text-only case (the default
    /// for every existing tool). Leaves `attachments` empty so existing
    /// call sites that pre-date the field can migrate to this with
    /// search-and-replace if desired.
    pub fn text(content: impl Into<String>, is_error: bool) -> Self {
        Self {
            content: content.into(),
            is_error,
            attachments: Vec::new(),
        }
    }
}

/// C9.3: discriminated-union of binary payloads a tool can return
/// alongside its text output. Each variant maps to a native attachment
/// block on the provider side — see the per-variant comments for the
/// target encoding.
///
/// Adding a new variant is forward-compatible: tools that don't emit
/// it stay text-only; providers that don't recognise it fall back to a
/// short text placeholder in the tool_result content. The model never
/// crashes on an unknown attachment.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolAttachment {
    /// Raw image bytes (PNG / JPEG / WebP / GIF). Anthropic and OpenAI
    /// both accept this as a native `image` block in tool_result content
    /// — the model "sees" the pixels. `media_type` is the standard MIME
    /// (e.g. `image/png`).
    Image { media_type: String, data: Vec<u8> },
    /// PDF document bytes. Anthropic supports `document` blocks with
    /// base64 source for PDFs up to ~32 MB — the model reads them
    /// natively. Other providers fall back to text-only (we only emit
    /// this when the text extraction already populated `content`, so
    /// the fallback isn't catastrophic). `page_count` is informational
    /// — surfaced in logs / UI; the API doesn't need it.
    Pdf { data: Vec<u8>, page_count: usize },
}

/// Per-task record of files that `read_file` has already returned to the model.
/// What unit of range we're tracking. Text reads count in 1-indexed line
/// numbers; notebook reads count in 1-indexed cell numbers. They share
/// the same registry path-keyed by `(PathBuf, ReadUnit)` so a line read
/// of foo.ipynb (rare but legal — e.g. raw JSON inspection) and a cell
/// read of foo.ipynb don't collide.
///
/// PDF / DOCX / XLSX get their own variants once those formats land.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum ReadUnit {
    /// Text-file 1-indexed line number range.
    Lines,
    /// .ipynb cell index range (1-indexed, inclusive).
    Cells,
}

/// Used to short-circuit a repeat read of an unchanged file with a FILE_UNCHANGED
/// stub so the model cites the earlier `read_file` tool_result instead of paying
/// to re-fetch the same bytes.
#[derive(Default)]
pub struct FileReadRegistry {
    /// Map from `(canonical absolute path, ReadUnit)` → the cumulative
    /// range the model has already seen for that file/unit pair, plus
    /// the mtime it was seen at. If the current mtime still matches and
    /// the new request fits inside that range, we stub.
    ///
    /// Keyed by `(PathBuf, ReadUnit)` rather than `PathBuf` alone so a
    /// text-line read of an .ipynb and a cell read of the same file
    /// don't false-stub each other.
    entries: Mutex<std::collections::HashMap<(PathBuf, ReadUnit), FileReadEntry>>,
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

    /// Return true if `(path, unit, start, end)` with the given current
    /// mtime is already fully covered by a prior read of the same unit —
    /// in which case the caller should return a FILE_UNCHANGED stub
    /// instead of re-reading.
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

    /// Record a completed read so future identical (or narrower) reads of the
    /// same file/unit, at the same mtime, can short-circuit.
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

    /// Drop any cached record for `path` across ALL units — call this
    /// whenever a write tool modifies the file so the next read doesn't
    /// falsely stub on stale mtime. We drop every unit because a write
    /// changes the bytes regardless of how prior reads partitioned them.
    pub fn invalidate(&self, path: &std::path::Path) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.retain(|(p, _), _| p != path);
        }
    }
}

/// Callback type for incrementally persisting the in-flight message history
/// to durable storage. Called from inside `run_turn` at every stable point
/// (top of each iteration, after tool batches, before return) so a sudden
/// process termination — most commonly the user clicking Stop and then
/// closing the app — doesn't lose the model's progress mid-turn. The host
/// (src-tauri) supplies a closure that captures the DB handle and writes
/// `messages` transactionally via `replace_messages_for_task`.
pub type PersistMessagesFn = Arc<dyn Fn(&[crate::provider::Message]) + Send + Sync>;

/// Context available to tool execution.
pub struct ToolContext {
    pub project_root: PathBuf,
    /// Shared permissions — updated in real-time when the user changes permission mode mid-conversation.
    pub shared_permissions: SharedPermissions,
    /// Optional callback to persist messages incrementally during run_turn.
    /// `None` for sub-agents and unit tests; `Some(fn)` for the main task so
    /// the host can save progress transactionally as the agent works.
    pub persist_messages_fn: Option<PersistMessagesFn>,
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
    /// The parent agent's provider config — sub-agents inherit model, API key, max_tokens, thinking_budget.
    pub parent_provider_config: Option<Arc<ProviderConfig>>,
    /// Optional cheaper/faster provider config used when the main agent calls
    /// `spawn_subagent` with `model_tier: "fast"`. `None` when the user has
    /// not configured a sub-agent model in settings — in that case the
    /// `model_tier` parameter is also omitted from the tool schema.
    pub subagent_provider_config: Option<Arc<ProviderConfig>>,
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
    /// P0.3 plan mode: when true, all write- / execute-class tools are
    /// rejected before they reach the underlying handler. The task agent
    /// can still investigate (read_file, grep_search, list_directory,
    /// glob, todo_write, web_search/web_fetch) and propose, but cannot
    /// modify the filesystem or run shell commands until the user
    /// flips this off from the task panel. This is a per-task toggle —
    /// distinct from `is_global` (which is read-only by *scope* policy,
    /// not by user intent). Defaults to false so existing call sites
    /// behave identically to before.
    pub is_plan_mode: bool,
    /// P0.4: cross-task budget enforcer (concurrent-stream cap + daily cost
    /// ceiling). Wired through `ToolContext` so the executor can acquire
    /// permits and check the ceiling on every turn. Sub-agents inherit the
    /// parent's Budget handle so their streams + costs count against the
    /// same global pool. `unrestricted()` is the safe fallback for callers
    /// that don't have a configured budget (unit tests, embedded use).
    pub budget: crate::budget::Budget,
    /// P0.2: shared broker for the `ask_user` tool. The tool emits a
    /// `TaskEvent::AskUserRequest` via this broker, parks on a oneshot,
    /// and unblocks when the user submits answers via the
    /// `respond_to_ask_user` Tauri command. Sub-agents share the parent's
    /// broker handle so a child's `ask_user` call surfaces in the same
    /// dialog flow.
    pub ask_user_broker: std::sync::Arc<crate::task::ask_user_broker::AskUserBroker>,
    /// P0.4 fix #4: shared broker for the daily-cost-ceiling pause flow.
    /// When the executor detects the ceiling has been hit at the top of a
    /// new turn, it parks on this broker (instead of hard-failing the
    /// task) until the user picks "Raise ceiling to …" or "Stop task" in
    /// the frontend modal. Sub-agents share the parent's broker so their
    /// own runs respect the same global cap.
    pub ceiling_broker: std::sync::Arc<crate::task::ceiling_broker::CeilingBroker>,
    /// Host-supplied surface for orchestrator operations (spawn_subtask,
    /// list_tasks_across_projects, read_task_history). `None` in unit tests
    /// and outside the Global scope.
    pub orchestrator_host: Option<Arc<dyn OrchestratorHost>>,
    /// Changed-files tracker. `None` disables tracking entirely (e.g. in unit
    /// tests, or when the host hasn't initialized the history registry yet).
    /// Edit tools call `capture` synchronously before mutating a file; the
    /// bash tool enqueues a `SweepJob` after each foreground invocation.
    pub file_history: Option<Arc<crate::file_history::FileHistory>>,
    /// Background sweep worker bound to `file_history`. `None` when
    /// `file_history` is `None` — both must be wired together.
    pub sweep_worker: Option<Arc<crate::file_history::SweepWorker>>,
    /// UUID of the user message this turn is responding to. The tracker uses
    /// this as the snapshot anchor — every capture / sweep recorded during
    /// the turn lands in the snapshot for this id, so `/rewind` to this
    /// message restores the worktree to its pre-turn state.
    pub current_user_message_id: Option<String>,
    /// Shared accumulator for non-token tool costs. Media-generation tools
    /// (`image_create`, `video_create`, `animate`) hit per-image / per-second
    /// priced endpoints whose spend never shows up in chat-model
    /// `TokenUsage`. They deposit their estimated cost here; the executor
    /// drains it after every tool batch and folds it into
    /// `TaskCost.estimated_cost_usd` so the cost shown in the chat header
    /// reflects the real total.
    pub tool_cost_sink: Arc<Mutex<f64>>,
    /// P1.3: per-project shared services (tree-sitter parsers, symbol index,
    /// file watcher). Multiple concurrent tasks in the same project hold
    /// `Arc` clones of the same `WorkspaceServices` so this state exists once
    /// per project, not once per task. Sub-agents inherit the parent's handle.
    /// Constructed by the host via `WorkspaceRegistry::get_or_create`.
    pub workspace_services: Arc<crate::workspace::WorkspaceServices>,
    /// C3.4: host-side `WorkspaceRegistry` so sub-agents spawned with a
    /// `project_root` override (P1.4 worktree handoff) can look up their
    /// OWN `WorkspaceServices` for that worktree path instead of inheriting
    /// the parent's. Sharing the registry across all contexts keeps the
    /// per-project dedupe contract — two children both spawned into the
    /// same worktree share a single services bundle.
    pub workspace_registry: Arc<crate::workspace::WorkspaceRegistry>,
    /// P1.6: identifies this context as belonging to a sub-agent. `Some((
    /// parent_task_id, agent_id))` when this is a sub-agent's `ToolContext`;
    /// `None` for the main agent. The executor uses this to drain the
    /// sub-agent's inbox at turn boundaries and to record per-turn
    /// `record_turn` calls into the registry.
    pub subagent_self: Option<(String, String)>,
    /// P1.7: per-task set of deferred tool names the model has loaded via
    /// `tool_search` so far. Starts empty; the executor reads this at the
    /// top of every turn to decide which deferred tools (in addition to
    /// `tool_search::ALWAYS_ON`) get their full schemas back in the
    /// request. Sub-agents share the Arc with the parent so a tool
    /// loaded by the orchestrator is immediately available to its children.
    pub loaded_deferred_tools: Arc<Mutex<std::collections::HashSet<String>>>,
}

impl ToolContext {
    /// Emit a tool progress event (non-blocking). Used by long-running tools
    /// to report intermediate status to the UI (e.g., "5 matches found so far").
    pub fn emit_progress(&self, tool_use_id: &str, progress_text: &str) {
        let _ = self.event_tx.try_send(crate::task::TaskEvent::ToolProgress {
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
                // P1.4: worktree management.
                | "enter_worktree"
                | "exit_worktree"
                // C3.7: read-only worktree enumeration.
                | "list_worktrees"
                // P1.7: deferred-schema lookup. Always advertised when the
                // tool-search feature is enabled by the host.
                | "tool_search"
                // P1.8: end-of-goal marker for /goal mode.
                | "goal_complete"
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
                | "list_subagents"
                | "report_blocked_write"
                | "todo_write"
                | "read_terminal_output"
                | "list_projects"
                | "list_tasks_across_projects"
                | "read_task_history"
                | "web_search"
                | "web_fetch"
                // P0.2 + P0.3: ask_user is an interactive prompt — it
                // doesn't touch the filesystem or run anything, so it's
                // safe in plan mode and parallelizable with other reads.
                | "ask_user"
                // P1.2: all 5 code-intel tools are read-only; they query
                // the symbol index and parse files but never write.
                | "find_symbol"
                | "goto_definition"
                | "find_references"
                | "outline"
                | "call_sites"
                // P1.7: schema lookup is pure metadata fetch — no IO beyond
                // reading a static table — safe to run in the parallel batch.
                | "tool_search"
                // P1.8: marking the goal complete is metadata only.
                | "goal_complete"
                // C3.7: enumerating worktrees only reads `.git` — no writes.
                | "list_worktrees"
        )
    }

    /// Like `definitions()` but constrains the `run_command` tool's `shell`
    /// parameter to the set of shells the host has confirmed are installed.
    /// Prefer this from the executor — it prevents the model from asking for
    /// a shell that would fail to spawn.
    ///
    /// `fast_subagent_model` is the user-configured cheaper sub-agent model id
    /// or `None` when no sub-agent model is configured. When `Some`, the
    /// `spawn_subagent` schema gains a required `model_tier` parameter so the
    /// orchestrator picks per-spawn between the main and the configured
    /// cheaper model. When `None`, the choice is hidden from the model.
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
        // P0.2: ask_user is now advertised to the model — the AskUserBroker
        // + tabbed dialog are wired end-to-end, so the agent has a real way
        // to request structured input from the user mid-turn.
        defs.extend(ask_user::definitions());
        // P1.2: code-intel tools (find_symbol, goto_definition,
        // find_references, outline, call_sites).
        defs.extend(code_intel::definitions());
        // P1.4: worktree management — `enter_worktree` / `exit_worktree`.
        defs.extend(worktree::definitions());
        // P1.8: end-of-goal marker. Always present even outside goal-loop
        // mode so a model running on a stale prompt has a useful no-op
        // response instead of "unknown tool". The handler is a no-op
        // outside goal mode.
        defs.extend(goal_loop_def());
        // P1.7: tool_search is the meta-tool the model uses to fetch
        // deferred tools' full schemas. Always part of the always-on pool
        // (see `tool_search::ALWAYS_ON`); listing it here means the model
        // sees the schema in the prompt as long as the executor doesn't
        // strip it.
        defs.push(crate::task::tool_search::tool_search_def());
        defs
    }
}

/// Definition for the `goal_complete` tool. Lives at module scope so the
/// goal-loop wrapper can both inject it into the prompt and look it up by
/// name when the model invokes it.
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
        // Trait impl has no host context — use the empty-shell variant which
        // drops the `shell` parameter. Real callers should prefer
        // `definitions_for_host()` so the model sees the host's actual shells.
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
            // P1.9: handler still dispatches the legacy name to a clear
            // "tool removed" message so a model running on a stale prompt
            // gets a useful explanation instead of "unknown tool".
            | "wait_for_subagents" => {
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
            // P0.2: dispatch in place but tool def is NOT advertised to the
            // model via definitions_for_host yet (UI dialog isn't wired). If
            // the model somehow calls it the placeholder error makes the
            // gap obvious.
            "ask_user" => ask_user::execute(params, context).await,
            // P1.2: workspace symbol index + tree-sitter walks.
            "find_symbol" | "goto_definition" | "find_references" | "outline"
            | "call_sites" => code_intel::execute(name, params, context).await,
            // P1.4: worktree creation / removal.
            "enter_worktree" | "exit_worktree" | "list_worktrees" => worktree::execute(name, params, context).await,
            // P1.7: tool-search dispatches into its own module which reads
            // the configured deferred-tool table. When the feature is off
            // the schema is hidden from the model, but the dispatch stays
            // available in case the call slips through.
            "tool_search" => crate::task::tool_search::execute(params, context).await,
            // P1.8: `goal_complete` is recognised by the executor's goal-
            // loop wrapper; outside that wrapper it's an informational
            // no-op so a model calling it doesn't fail the turn.
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


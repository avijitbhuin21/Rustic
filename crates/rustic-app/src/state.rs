use crate::watcher::FileWatcherManager;
use rustic_agent::{
    AgentTerminalExit, AiConfig, FileHistory, FileLockRegistry, FileWatcher, McpManager, Message,
    PermissionBroker, PermissionLevel, SharedPermissions, SubagentRegistry, SweepWorker, TaskCost,
    TaskInfo, ToolConfig, WorkspaceRegistry,
};
use rustic_core::buffer::{Buffer, BufferId};
use rustic_core::syntax::SyntaxHighlighter;
use rustic_core::workspace::Workspace;
use rustic_db::Database;
use rustic_terminal::TerminalManager;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, Mutex};

/// Thread-safe map of task_id → latest TaskCost, updated by the executor thread.
pub type TaskCostMap = Arc<Mutex<HashMap<String, TaskCost>>>;

pub struct AgentTask {
    pub info: TaskInfo,
    pub messages: Vec<Message>,
    pub permissions: PermissionLevel,
    /// Whether the agent is allowed to read sensitive files without prompting (FullAuto only).
    pub sensitive_files_allowed: bool,
    /// P0.3: per-task plan-mode toggle. When true, the executor rejects every
    /// write- / execute-class tool call. Default false. Flipped by the user
    /// via the `set_task_plan_mode` Tauri command and read at the top of
    /// each `send_message` to populate `ToolContext.is_plan_mode`.
    pub is_plan_mode: bool,
    /// Shared permissions — the executor reads from this Arc in real-time.
    /// When the user changes permissions mid-conversation, we update this and the executor sees it.
    pub shared_permissions: Option<SharedPermissions>,
    /// Accumulated token cost for this task (updated after each turn).
    #[allow(dead_code)]
    pub cost: TaskCost,
    /// Cached file tree string to avoid expensive filesystem walks on every message.
    /// Regenerated when the cache is empty or when files have been modified.
    pub cached_file_tree: Option<String>,
    /// Timestamp (unix millis) of when cached_file_tree was last generated.
    /// Used to detect stale caches.
    pub file_tree_cache_time: u64,
    /// Live /goal slot shared with the executor (ToolContext.goal_state).
    /// `Some(GoalState)` while a goal is active; hosts write it on
    /// set/clear commands and hydrate it from tasks.goal on load.
    pub goal: rustic_agent::task::goal::GoalSlot,
}

pub struct AgentState {
    pub tasks: HashMap<String, AgentTask>,
    pub ai_config: AiConfig,
    /// Agent-level tool configuration (web_search/web_fetch toggles + backend keys).
    pub tool_config: ToolConfig,
    pub project_permissions: HashMap<String, PermissionLevel>,
    pub mcp_manager: Arc<Mutex<McpManager>>,
    /// Per-task cancellation tokens. Set to true to abort a running task.
    pub cancellation_tokens: HashMap<String, Arc<AtomicBool>>,
    /// Shared permission broker for ManualEdit / AutoEdit approval flow.
    pub permission_broker: Arc<PermissionBroker>,
    /// P0.2: shared broker for the `ask_user` tool. One per AgentState
    /// (shared across all tasks). Tasks construct their own ToolContext
    /// with an Arc-clone of this so sub-agents can call `ask_user` and
    /// land in the same dialog flow as their parent.
    pub ask_user_broker: Arc<rustic_agent::task::ask_user_broker::AskUserBroker>,
    /// P0.4 fix #4: shared broker for the daily-cost-ceiling pause flow.
    /// Sized like `ask_user_broker` — one per AgentState, cloned into
    /// every ToolContext so the executor can park a task on a "raise or
    /// stop" dialog instead of hard-failing on the first breach.
    pub ceiling_broker: Arc<rustic_agent::task::ceiling_broker::CeilingBroker>,
}

impl AgentState {
    pub fn new() -> Self {
        Self {
            tasks: HashMap::new(),
            ai_config: AiConfig::new(),
            tool_config: ToolConfig::new(),
            project_permissions: HashMap::new(),
            mcp_manager: Arc::new(Mutex::new(McpManager::new())),
            cancellation_tokens: HashMap::new(),
            permission_broker: Arc::new(PermissionBroker::new()),
            ask_user_broker: Arc::new(rustic_agent::task::ask_user_broker::AskUserBroker::new()),
            ceiling_broker: Arc::new(rustic_agent::task::ceiling_broker::CeilingBroker::new()),
        }
    }
}

pub struct AppState {
    pub workspace: Mutex<Workspace>,
    pub buffers: Mutex<HashMap<BufferId, Buffer>>,
    pub highlighters: Mutex<HashMap<BufferId, SyntaxHighlighter>>,
    pub terminal_manager: Mutex<TerminalManager>,
    pub agent: Arc<Mutex<AgentState>>,
    /// Locking discipline: these are `std::sync::Mutex`es (deliberately — many
    /// lock sites live in sync-only contexts: spawn_blocking workers, FS-watcher
    /// callbacks, `lock_safe`). Guards must NEVER be held across an `.await`
    /// point; take the lock in a narrow scope, clone/copy the data out, drop the
    /// guard, then await. Enforced by `clippy::await_holding_lock = "deny"`
    /// (workspace lint in the root Cargo.toml).
    pub db: Arc<Mutex<Database>>,
    pub git_token: Mutex<Option<String>>,
    /// Latest TaskCost per task_id. Updated by the executor thread via Arc clone.
    pub task_costs: TaskCostMap,
    /// Per-file async mutex registry — shared across all tasks to prevent concurrent file writes.
    pub file_lock: Arc<FileLockRegistry>,
    /// Sub-agent registry — shared across all tasks so sub-agents can report back.
    pub subagent_registry: Arc<SubagentRegistry>,
    /// File system watcher — watches project directories for external changes.
    pub file_watcher: Mutex<FileWatcherManager>,
    /// Per-task queue of pty-exit notifications for agent-owned background
    /// terminals. The pty reader thread pushes on EOF; the executor loop
    /// drains at the top of each iteration so exits become user-visible
    /// messages the model sees on the next turn.
    pub agent_terminal_exits: Arc<Mutex<HashMap<String, Vec<AgentTerminalExit>>>>,
    /// Changed-files tracker handles, keyed by canonicalized project root.
    /// Lazily populated in `send_message` the first time a project takes a
    /// turn; subsequent turns reuse the same `FileHistory` + `SweepWorker`
    /// so the sweep worker's tokio task isn't churned every message.
    pub file_history_registry: Arc<Mutex<HashMap<String, FileHistoryHandle>>>,
    /// Monotonically-increasing search id. `start_search` bumps and stores the
    /// new value; the background walker task only emits events while the
    /// stored id matches its own — incrementing the counter therefore cancels
    /// every in-flight search. Shared across the search command and the
    /// blocking worker via `Arc`.
    pub active_search_id: Arc<AtomicU64>,
    /// P1.3: per-project shared services (tree-sitter parsers, symbol index,
    /// file watcher). The registry hands out one `Arc<WorkspaceServices>` per
    /// canonical project root so concurrent tasks in the same project share
    /// one instance instead of holding 4× copies of the parser state.
    pub workspace_services: Arc<WorkspaceRegistry>,
}

/// Per-project pair: the synchronous tracker API + its background sweep worker.
/// Both are `Arc`-cloned into the `ToolContext` for each turn.
///
/// `_watcher` is held here purely to keep the OS-level FS watch alive for
/// the lifetime of the project — dropping it stops the watcher and cuts
/// off events to the accumulator. The field is intentionally unread; the
/// accumulator owns the connection to the file history. `None` when watcher
/// registration failed (e.g. Linux inotify limit exceeded) — the sweep
/// transparently falls back to full walks in that case.
#[derive(Clone)]
pub struct FileHistoryHandle {
    pub history: Arc<FileHistory>,
    pub sweep: Arc<SweepWorker>,
    pub _watcher: Option<Arc<FileWatcher>>,
}

impl AppState {
    pub fn new(db: Database) -> Self {
        let agent = Arc::new(Mutex::new(AgentState::new()));
        Self {
            workspace: Mutex::new(Workspace::new()),
            buffers: Mutex::new(HashMap::new()),
            highlighters: Mutex::new(HashMap::new()),
            terminal_manager: Mutex::new(TerminalManager::new()),
            agent,
            db: Arc::new(Mutex::new(db)),
            git_token: Mutex::new(None),
            task_costs: Arc::new(Mutex::new(HashMap::new())),
            file_lock: FileLockRegistry::new(),
            subagent_registry: SubagentRegistry::new(),
            file_watcher: Mutex::new(FileWatcherManager::new()),
            agent_terminal_exits: Arc::new(Mutex::new(HashMap::new())),
            file_history_registry: Arc::new(Mutex::new(HashMap::new())),
            active_search_id: Arc::new(AtomicU64::new(0)),
            workspace_services: Arc::new(WorkspaceRegistry::new()),
        }
    }
}

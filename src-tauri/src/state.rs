use crate::watcher::FileWatcherManager;
use rustic_core::buffer::{Buffer, BufferId};
use rustic_core::lsp::LspManager;
use rustic_core::syntax::SyntaxHighlighter;
use rustic_core::workspace::Workspace;
use rustic_terminal::TerminalManager;
use rustic_agent::{
    AgentTerminalExit, AiConfig, FileLockRegistry, HarnessRegistry, McpManager, Message,
    PermissionBroker, PermissionLevel, SharedPermissions, SubagentRegistry, TaskCost, TaskInfo,
    ToolConfig, UserQuestionBroker,
};
use rustic_db::Database;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

/// Thread-safe map of task_id → latest TaskCost, updated by the executor thread.
pub type TaskCostMap = Arc<Mutex<HashMap<String, TaskCost>>>;

pub struct AgentTask {
    pub info: TaskInfo,
    pub messages: Vec<Message>,
    pub permissions: PermissionLevel,
    /// Whether the agent is allowed to read sensitive files without prompting (FullAuto only).
    pub sensitive_files_allowed: bool,
    /// Shared permissions — the executor reads from this Arc in real-time.
    /// When the user changes permissions mid-conversation, we update this and the executor sees it.
    pub shared_permissions: Option<SharedPermissions>,
    /// Accumulated token cost for this task (updated after each turn).
    #[allow(dead_code)]
    pub cost: TaskCost,
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
    /// Shared question broker for ask_user tool — pauses agent and waits for user input.
    pub question_broker: Arc<UserQuestionBroker>,
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
            question_broker: Arc::new(UserQuestionBroker::new()),
        }
    }
}

pub struct AppState {
    pub workspace: Mutex<Workspace>,
    pub buffers: Mutex<HashMap<BufferId, Buffer>>,
    pub highlighters: Mutex<HashMap<BufferId, SyntaxHighlighter>>,
    pub terminal_manager: Mutex<TerminalManager>,
    pub agent: Arc<Mutex<AgentState>>,
    pub db: Arc<Mutex<Database>>,
    pub lsp_manager: Arc<Mutex<LspManager>>,
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
    /// Root on disk where per-checkpoint project snapshots are stored, laid
    /// out as `<snapshot_root>/<task_id>/<checkpoint_id>/`. Populated from the
    /// app data dir at startup.
    pub snapshot_root: PathBuf,
    /// Live external-agent CLI sessions (Claude Code, Codex). One entry per
    /// task currently dispatched to a harness provider. The Tauri close hook
    /// calls `shutdown_all` so no `claude`/`codex` child outlives the app.
    pub harness_registry: Arc<HarnessRegistry>,
}

impl AppState {
    pub fn new(db: Database, snapshot_root: PathBuf) -> Self {
        Self {
            workspace: Mutex::new(Workspace::new()),
            buffers: Mutex::new(HashMap::new()),
            highlighters: Mutex::new(HashMap::new()),
            terminal_manager: Mutex::new(TerminalManager::new()),
            agent: Arc::new(Mutex::new(AgentState::new())),
            db: Arc::new(Mutex::new(db)),
            lsp_manager: Arc::new(Mutex::new(LspManager::new())),
            git_token: Mutex::new(None),
            task_costs: Arc::new(Mutex::new(HashMap::new())),
            file_lock: FileLockRegistry::new(),
            subagent_registry: SubagentRegistry::new(),
            file_watcher: Mutex::new(FileWatcherManager::new()),
            agent_terminal_exits: Arc::new(Mutex::new(HashMap::new())),
            snapshot_root,
            harness_registry: Arc::new(HarnessRegistry::new()),
        }
    }
}

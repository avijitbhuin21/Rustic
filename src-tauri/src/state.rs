use crate::watcher::FileWatcherManager;
use rustic_core::buffer::{Buffer, BufferId};
use rustic_core::lsp::LspManager;
use rustic_core::syntax::SyntaxHighlighter;
use rustic_core::workspace::Workspace;
use rustic_terminal::TerminalManager;
use rustic_agent::{
    AiConfig, FileLockRegistry, McpManager, Message, PermissionBroker, PermissionLevel,
    SharedPermissions, TaskCost, TaskInfo, TurnBudget, SubagentRegistry, UserQuestionBroker,
};
use rustic_db::Database;
use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

/// Thread-safe map of task_id → latest TaskCost, updated by the executor thread.
pub type TaskCostMap = Arc<Mutex<HashMap<String, TaskCost>>>;

/// Thread-safe map of task_id → live TurnBudget Arc (shared with executor).
pub type TurnBudgetMap = Arc<Mutex<HashMap<String, Arc<Mutex<TurnBudget>>>>>;

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
    pub project_permissions: HashMap<String, PermissionLevel>,
    pub mcp_manager: Arc<Mutex<McpManager>>,
    /// Per-task cancellation tokens. Set to true to abort a running task.
    pub cancellation_tokens: HashMap<String, Arc<AtomicBool>>,
    /// Shared permission broker for ManualEdit / AutoEdit approval flow.
    pub permission_broker: Arc<PermissionBroker>,
    /// Shared question broker for ask_user tool — pauses agent and waits for user input.
    pub question_broker: Arc<UserQuestionBroker>,
    /// Default max turns per task (project-level override). Defaults to 50.
    pub default_turn_budget: u32,
}

impl AgentState {
    pub fn new() -> Self {
        Self {
            tasks: HashMap::new(),
            ai_config: AiConfig::new(),
            project_permissions: HashMap::new(),
            mcp_manager: Arc::new(Mutex::new(McpManager::new())),
            cancellation_tokens: HashMap::new(),
            permission_broker: Arc::new(PermissionBroker::new()),
            question_broker: Arc::new(UserQuestionBroker::new()),
            default_turn_budget: 50,
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
    pub lsp_manager: Mutex<LspManager>,
    pub git_token: Mutex<Option<String>>,
    /// Latest TaskCost per task_id. Updated by the executor thread via Arc clone.
    pub task_costs: TaskCostMap,
    /// Live TurnBudget Arc per task_id. Shared with executor; updated by extend_turn_budget command.
    pub turn_budgets: TurnBudgetMap,
    /// Per-file async mutex registry — shared across all tasks to prevent concurrent file writes.
    pub file_lock: Arc<FileLockRegistry>,
    /// Sub-agent registry — shared across all tasks so sub-agents can report back.
    pub subagent_registry: Arc<SubagentRegistry>,
    /// File system watcher — watches project directories for external changes.
    pub file_watcher: Mutex<FileWatcherManager>,
}

impl AppState {
    pub fn new(db: Database) -> Self {
        Self {
            workspace: Mutex::new(Workspace::new()),
            buffers: Mutex::new(HashMap::new()),
            highlighters: Mutex::new(HashMap::new()),
            terminal_manager: Mutex::new(TerminalManager::new()),
            agent: Arc::new(Mutex::new(AgentState::new())),
            db: Arc::new(Mutex::new(db)),
            lsp_manager: Mutex::new(LspManager::new()),
            git_token: Mutex::new(None),
            task_costs: Arc::new(Mutex::new(HashMap::new())),
            turn_budgets: Arc::new(Mutex::new(HashMap::new())),
            file_lock: FileLockRegistry::new(),
            subagent_registry: SubagentRegistry::new(),
            file_watcher: Mutex::new(FileWatcherManager::new()),
        }
    }
}

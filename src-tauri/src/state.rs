use rustic_core::buffer::{Buffer, BufferId};
use rustic_core::lsp::LspManager;
use rustic_core::syntax::SyntaxHighlighter;
use rustic_core::workspace::Workspace;
use rustic_terminal::TerminalManager;
use rustic_agent::{
    AiConfig, McpManager, Message, PermissionLevel, TaskInfo,
};
use rustic_db::Database;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub struct AgentTask {
    pub info: TaskInfo,
    pub messages: Vec<Message>,
    pub permissions: PermissionLevel,
}

pub struct AgentState {
    pub tasks: HashMap<String, AgentTask>,
    pub ai_config: AiConfig,
    pub project_permissions: HashMap<String, PermissionLevel>,
    pub mcp_manager: McpManager,
}

impl AgentState {
    pub fn new() -> Self {
        Self {
            tasks: HashMap::new(),
            ai_config: AiConfig::new(),
            project_permissions: HashMap::new(),
            mcp_manager: McpManager::new(),
        }
    }
}

pub struct AppState {
    pub workspace: Mutex<Workspace>,
    pub buffers: Mutex<HashMap<BufferId, Buffer>>,
    pub highlighters: Mutex<HashMap<BufferId, SyntaxHighlighter>>,
    pub terminal_manager: Mutex<TerminalManager>,
    pub agent: Mutex<AgentState>,
    pub db: Arc<Mutex<Database>>,
    pub lsp_manager: Mutex<LspManager>,
    pub git_token: Mutex<Option<String>>,
}

impl AppState {
    pub fn new(db: Database) -> Self {
        Self {
            workspace: Mutex::new(Workspace::new()),
            buffers: Mutex::new(HashMap::new()),
            highlighters: Mutex::new(HashMap::new()),
            terminal_manager: Mutex::new(TerminalManager::new()),
            agent: Mutex::new(AgentState::new()),
            db: Arc::new(Mutex::new(db)),
            lsp_manager: Mutex::new(LspManager::new()),
            git_token: Mutex::new(None),
        }
    }
}

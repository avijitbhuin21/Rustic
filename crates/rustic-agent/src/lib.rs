pub mod budget;
pub mod config;
pub mod file_history;
pub mod file_tree;
pub mod index;
pub mod io_util;
pub mod mcp;
pub mod model_registry;
pub mod provider;
pub mod rules;
pub mod skills;
pub mod system_prompt;
pub mod task;
pub mod tools;
pub mod workflows;
pub mod workspace;

pub use config::{
    AiConfig, ModelCapabilities, ProviderEntry, ProviderType, SubagentConfig, ToolConfig,
    WebFetchConfig, WebSearchBackend, WebSearchConfig,
};
pub use provider::{
    AiProvider, AiResponse, ContentBlock, Message, ModelInfo, ProviderConfig, Role, StopReason,
    TokenUsage, ToolDef,
};
pub use task::{EventTx, EVENT_CHANNEL_CAP, FileTrackedKind, PermissionOp, TaskEvent, TaskInfo, TaskStatus};
pub use file_history::{
    CaptureOutcome, ChangeCallback, DirtyPathAccumulator, DirtySet, FileChangeStats, FileDiff,
    FileHistory, FileHistoryError, FileWatcher, FileWatcherError, RestoreOutcome, RevertPlanEntry,
    ShadowSnapshot, SweepJob, SweepWorker, TaskNetChange,
};
pub use task::subagent::{SubagentRegistry, SubagentResult, SubagentCompletionEvent};
pub use task::file_lock::FileLockRegistry;
pub use task::cost::{calculate_cost, calculate_cost_breakdown, CostBreakdown, TaskCost};
pub use task::executor::TaskExecutor;
pub use task::permission_broker::{NativePermissionDecision, PermissionBroker};
pub use task::terminal_broker::{AgentTerminalExit, AgentTerminalInfo, AgentTerminals};
pub use task::TodoItem;
pub use task::permissions::{PermissionLevel, SharedPermissions};
pub use tools::{BuiltinTools, FileReadRegistry, PersistMessagesFn, ToolContext, ToolExecutor, ToolOutput};
pub use mcp::{
    sha256_hex as mcp_sha256_hex, LoadProjectScopeResult, McpConnectResult, McpConnectionStatus,
    McpManager, McpScope, McpServerWithStatus, McpTransport, ServerConfig,
};
pub use skills::{SkillDef, SkillScope, discover_skills, discover_global_skills, global_skills_dir, build_skills_system_section, skill_body};
pub use system_prompt::{build_system_prompt, build_project_structure_section, build_subagent_prompt, plan_mode_addendum, shell_env, models_from_providers};
pub use file_tree::{generate_file_tree, generate_file_tree_with_limits};
pub use workflows::{WorkflowDef, discover_workflows, discover_global_workflows, global_workflows_dir, seed_default_workflows, workflow_body, build_workflows_system_section};
pub use rules::{
    RuleDef, RuleState, RulesState, build_user_rules_system_section, discover_global_rules,
    forget_rule, global_rules_dir, load_rules_state, parse_rule_frontmatter, rule_active_projects,
    rule_body, rule_state, set_rule_projects, set_rule_state,
};
pub use index::{IndexStatus, SymbolEntry, SymbolIndex, SymbolKind};
pub use workspace::{WorkspaceRegistry, WorkspaceServices};
pub use task::tool_search::{
    build_deferred_tools_directory, is_always_on as is_always_on_tool,
};

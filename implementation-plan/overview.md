# Rustic Agent — Architecture Overview

---

## Current State

```
crates/rustic-agent/src/
├── task/
│   ├── executor.rs       — agentic loop (sequential tool execution, no sub-agents)
│   ├── permissions.rs    — PermissionLevel: Admin | ReadWrite | ReadOnly (3 levels)
│   └── mod.rs
├── tools/
│   ├── file_ops.rs       — read_file, write_file, create_file, list_directory
│   ├── search.rs         — grep_search
│   └── terminal.rs       — run_command
├── provider/             — Claude, OpenAI, Compatible providers
├── mcp/                  — MCP manager (stdio + SSE)
├── checkpoint/           — SQLite-backed file snapshots
└── config.rs             — AiConfig, ProviderEntry

src/components/agent/
├── agent-panel.js        — task list sidebar
├── chat-view.js          — message rendering, checkpoint UI
└── mcp-config.js         — MCP server management UI
```

---

## Target Architecture

```
crates/rustic-agent/src/
├── task/
│   ├── executor.rs           — agentic loop + reactive sub-agent injection
│   ├── permissions.rs        — PermissionLevel: Chat|ManualEdit|AutoEdit|FullAuto
│   ├── permission_broker.rs  — oneshot channel approval flow for ManualEdit
│   ├── file_lock.rs          — per-file tokio::Mutex registry
│   ├── subagent.rs           — SubagentRegistry + broadcast channel completion
│   ├── cost.rs               — TaskCost accumulation + pricing table
│   └── mod.rs
├── tools/
│   ├── file_ops.rs           — create_file, edit_file, apply_patch,
│   │                           insert_lines, delete_lines (all locked RMW)
│   ├── search.rs             — grep_search (kept, lightweight)
│   ├── terminal.rs           — run_command (hard-capped output, OS-aware)
│   └── subagent_tools.rs     — spawn_subagent, wait_for_all_agents,
│                               list_active_agents, cancel_agent
├── context/
│   ├── mcp_loader.rs         — two-level deferred MCP loading + BM25 index
│   ├── skill_loader.rs       — SKILL.md discovery + lazy loading
│   ├── workflow_loader.rs    — workflow discovery
│   └── memory.rs             — memory.md load at task start
├── provider/                 — Claude, OpenAI, Gemini, Compatible
├── mcp/                      — MCP manager (stdio + HTTP + SSE)
├── checkpoint/               — existing (unchanged)
└── config.rs                 — AiConfig + SubagentModel + pricing

src/components/agent/
├── agent-panel.js            — task list + memory indicator
├── chat-view.js              — messages + model switch separators +
│                               approval widget + cost display + sub-agent panel
├── mcp-config.js             — MCP server management
├── skills-panel.js           — skills browser + install UI
└── workflows-panel.js        — workflow list + create UI
```

---

## Data Flow

### Normal Turn
```
User message
    │
    ▼
send_message (Tauri command)
    │  loads memory.md if first turn
    │  builds ToolContext (permissions, locks, broker, registry)
    │  detects OS/shell once
    ▼
TaskExecutor::run_turn()
    │
    ├─► provider.chat(messages, tools, config)
    │       └─ tools = built-ins + MCP (deferred) + sub-agent tools (if depth=0)
    │
    ├─► for each tool_use in response:
    │       ├─ run_command      → execute in shell, cap at 16KB
    │       ├─ edit_file        → lock → re-read → apply → write → unlock
    │       ├─ create_file      → reject if file has content
    │       ├─ apply_patch      → lock → apply all hunks atomically
    │       ├─ insert/delete    → lock → line operation → write
    │       ├─ spawn_subagent   → tokio::spawn new executor (depth=1) → return immediately
    │       ├─ wait_for_all     → SubagentRegistry::wait_for_all()
    │       └─ MCP tool         → route to MCP server (deferred schema load)
    │
    ├─► if no tool calls AND active sub-agents exist:
    │       └─ SubagentRegistry::wait_for_any()  [zero CPU, broadcast channel]
    │           └─ on completion: inject "[Sub-agent X completed]\n<result>" → loop
    │
    └─► if no tool calls AND no active sub-agents:
            └─ turn complete
```

### ManualEdit Approval Flow
```
edit_file / run_command called with ManualEdit mode
    │
    ▼
PermissionBroker::request()
    │  emits PermissionRequest event to Tauri frontend
    │  awaits oneshot channel (60s timeout → auto-deny)
    ▼
UI shows approval widget in chat view
User clicks Allow / Deny
    │
    ▼
respond_to_permission (Tauri command)
    │  resolves oneshot channel
    ▼
Tool executes (or returns PERMISSION_DENIED)
```

### Reactive Sub-Agent Completion
```
Main model ends turn with no tool calls
    │  5 sub-agents still running
    ▼
executor: SubagentRegistry::wait_for_any()
    │  tokio suspends — zero CPU
    │
    │  ... Agent B finishes at t=8s ...
    │
    ▼
broadcast fires → executor wakes
    │
    ▼
If result > 2K tokens: summarize via cheap model first
    │
    ▼
Inject: "[Sub-agent 'refactor-auth' completed — model: haiku]\n<result>\n[3 still running: ...]"
    │
    ▼
Loop back → main model processes B's result
    │  may spawn more agents, do file edits, or just acknowledge
    ▼
Continues until: no tool calls AND no active sub-agents
```

---

## Key Structures

### ToolContext (passed to every tool execution)
```rust
pub struct ToolContext {
    pub project_root: PathBuf,
    pub permissions: PermissionLevel,
    pub snapshot_fn: Option<SnapshotFn>,      // checkpoint before write
    pub permission_broker: Arc<PermissionBroker>,
    pub event_tx: UnboundedSender<TaskEvent>,
    pub task_id: String,
    pub file_lock_registry: Arc<FileLockRegistry>,
    pub subagent_registry: Arc<SubagentRegistry>,
    pub agent_depth: u8,                      // 0=main, 1=subagent
    pub shell_env: ShellEnv,                  // detected once at task start
    pub turn_budget: TurnBudget,              // remaining turns tracker
}
```

### TaskEvent (all events emitted to frontend)
```rust
pub enum TaskEvent {
    TextDelta { task_id, text },
    ToolUse { task_id, tool_name, tool_input },
    ToolResult { task_id, tool_use_id, output, is_error },
    StatusChange { task_id, status },
    MessageComplete { task_id, message },
    CostUpdate { task_id, cost: TaskCost },
    PermissionRequest { task_id, request_id, operation, description, preview },
    SubagentSpawned { task_id, agent_id, model },
    SubagentCompleted { task_id, agent_id, result },
    SubagentFailed { task_id, agent_id, error },
    SubagentTextDelta { task_id, agent_id, text },
    TurnBudgetWarning { task_id, turns_remaining },
}
```

### PermissionLevel
```rust
pub enum PermissionLevel {
    Chat,        // read-only; commands ask
    ManualEdit,  // writes ask; commands ask  [default]
    AutoEdit,    // writes auto; commands ask
    FullAuto,    // nothing asks
}
```

---

## Context Budget (200K window example)

```
System prompt:          ~8,000 tokens  (fixed)
Tool definitions:       ~3,000 tokens  (5 write tools + sub-agent tools)
MCP tool names:           ~500 tokens  (deferred schemas — loaded on demand)
Skill descriptions:     ~1,000 tokens  (names+desc only at start)
Shell env injection:       ~50 tokens  (one line)
Memory.md (if present): ~2,000 tokens  (first message pair, session start only)
Turn budget reminder:      ~20 tokens  (injected at N-5 turns)
────────────────────────────────────────
Fixed overhead:        ~14,570 tokens  (~7.3% of 200K)
Available for conversation: ~185,000 tokens
Reserved buffer:        ~10,000 tokens  (never fill to 100%)
```

---

## New Tauri Commands

| Command | Description |
|---|---|
| `get_task_cost(task_id)` | Returns TaskCost |
| `switch_model(task_id, provider, model)` | Switch model, inject separator |
| `set_task_permissions(task_id, level)` | Change permission mode |
| `respond_to_permission(task_id, request_id, approved)` | Approve/deny ManualEdit operation |
| `get_memory(project_id)` | Read memory.md |
| `clear_memory(project_id)` | Clear memory.md |
| `list_skills(scope)` | List installed skills |
| `install_skill(source)` | Install from GitHub URL or local path |
| `delete_skill(name, scope)` | Remove a skill |
| `list_workflows()` | List project workflows |
| `create_workflow(name, description, body)` | Create new workflow |
| `delete_workflow(name)` | Remove workflow |
| `list_mcp_servers_v2()` | List with enabled/trust/tool counts |
| `get_subagent_models()` | List available sub-agent models |
| `set_subagent_models(models)` | Update sub-agent model list |

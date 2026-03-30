# Sub-Agent System — Implementation Plan
> Addendum to agent-implementation-plan.md
> Date: 2026-03-30

---

## How Claude Code Does It (Reference)

- `Agent` tool spawns a subagent with its own **isolated context window**
- Subagents **cannot** spawn further subagents (depth limit = 1, prevents recursive explosion)
- Subagents run fully async, parent context is **not polluted** by intermediate work
- Parent only receives the **final summary** from the subagent
- Git worktrees used for file isolation when needed

---

## Rustic's Sub-Agent Design

### Core Principles

1. **Main model is always the orchestrator** — it decides if parallel work is worth it, assigns tasks, manages dependencies
2. **Sub-agents are workers** — they execute a focused task and return a result summary
3. **Depth limit = 1** — sub-agents cannot spawn their own sub-agents
4. **File lock registry** — safety net for file conflicts (primary prevention is smart assignment by main model)
5. **Model selection** — user configures which models are available for sub-agents; main model picks from that list when spawning

---

## New Tools the Main Model Gets

### `analyze_for_parallelism`
**Not an actual tool** — the main model does this reasoning itself. The system prompt instructs it:
- "Before spawning sub-agents, analyze whether parallel execution saves time"
- "If tasks have no shared files and no data dependencies, they can run in parallel"
- "If one task needs the output of another, they must be sequential"
- "If parallel execution saves <20% of time, do it sequentially to reduce overhead"

### `spawn_subagent`

```json
{
  "name": "spawn_subagent",
  "description": "Spawn a sub-agent to execute a focused task. Use when you have identified independent parallel work. The sub-agent runs concurrently and returns a result summary when done.",
  "parameters": {
    "agent_id": "string — unique name you assign (e.g. 'refactor-auth', 'write-tests')",
    "task": "string — complete self-contained instructions for what this agent must do",
    "model": "string — model ID from available_subagent_models (shown below)",
    "files": ["string"] — file paths this agent will READ or WRITE (for lock management)",
    "mode": "string — 'read_only' | 'read_write' (default: same as current session mode)"
  }
}
```

The tool definition always includes the `available_subagent_models` list (injected at tool definition time from settings) so the main model can see what's available and pick intelligently.

### `wait_for_agents`

```json
{
  "name": "wait_for_agents",
  "description": "Wait for one or more sub-agents to complete. Returns their results.",
  "parameters": {
    "agent_ids": ["string"] — agent_ids to wait for. Use ['*'] to wait for all active agents."
  }
}
```

### `list_active_agents`

```json
{
  "name": "list_active_agents",
  "description": "List all currently running sub-agents with their status and progress.",
  "parameters": {}
}
```

### `cancel_agent`

```json
{
  "name": "cancel_agent",
  "description": "Cancel a running sub-agent.",
  "parameters": {
    "agent_id": "string"
  }
}
```

---

## File Lock Registry (Race Condition Prevention)

### Three-Layer Strategy

**Layer 1 — Prevention (Main model):**
The main model is instructed in the system prompt to assign non-overlapping file sets to each sub-agent. This is the primary protection — if done correctly, no locks are ever contested.

**Layer 2 — Safety Net (File lock registry):**
In case of incidental overlaps (e.g., both agents read a shared config file), the registry provides shared-read / exclusive-write semantics.

**Layer 3 — Conflict Detection (Post-completion):**
After all sub-agents complete, the main model reviews their results for any detected conflicts and resolves them if needed.

### Registry Design

```rust
// In crates/rustic-agent/src/task/file_lock.rs (NEW)

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{RwLock, Mutex};

pub struct FileLockRegistry {
    // Per canonical file path → RwLock
    // Readers (multiple allowed): any agent doing read_file
    // Writer (exclusive): one agent doing write_file / create_file / delete_file
    locks: Mutex<HashMap<PathBuf, Arc<RwLock<()>>>>,
}

impl FileLockRegistry {
    pub fn new() -> Self {
        Self { locks: Mutex::new(HashMap::new()) }
    }

    async fn get_lock(&self, path: &Path) -> Arc<RwLock<()>> {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let mut map = self.locks.lock().await;
        map.entry(canonical).or_insert_with(|| Arc::new(RwLock::new(()))).clone()
    }

    // Acquire shared read lock — multiple agents can hold this simultaneously
    pub async fn acquire_read(&self, path: &Path) -> tokio::sync::OwnedRwLockReadGuard<()> {
        let lock = self.get_lock(path).await;
        lock.read_owned().await
        // Guard dropped when agent's operation completes
    }

    // Acquire exclusive write lock — blocks until all readers and other writers are done
    pub async fn acquire_write(&self, path: &Path) -> tokio::sync::OwnedRwLockWriteGuard<()> {
        let lock = self.get_lock(path).await;
        lock.write_owned().await
    }
}
```

**Why tokio RwLock (not std):** The executor is async. Using tokio's async RwLock means a waiting agent yields its thread rather than blocking it, so other agents can continue making progress while one waits for a lock.

**No queue needed separately:** `tokio::sync::RwLock` IS a queue — waiting writers are served in arrival order once all readers release. No custom queue implementation needed.

**Timeout:** Wrap with `tokio::time::timeout(Duration::from_secs(30), lock.write_owned())` — if a sub-agent can't acquire a write lock within 30 seconds, return an error to the model. This prevents deadlock.

### Integration into ToolContext

```rust
pub struct ToolContext {
    pub project_root: PathBuf,
    pub permissions: PermissionLevel,
    pub snapshot_fn: Option<SnapshotFn>,
    pub permission_broker: Arc<PermissionBroker>,    // from previous plan
    pub event_tx: mpsc::UnboundedSender<TaskEvent>,  // from previous plan
    pub task_id: String,
    pub file_lock_registry: Arc<FileLockRegistry>,   // NEW
    pub agent_depth: u8,                             // NEW — 0=main, 1=subagent
    pub subagent_registry: Arc<SubagentRegistry>,    // NEW — for spawn_subagent tool
}
```

In `file_ops.rs`, before every write:
```rust
let _write_guard = timeout(
    Duration::from_secs(30),
    context.file_lock_registry.acquire_write(&absolute_path)
).await.map_err(|_| anyhow!("File lock timeout: another agent is using {}", path))?;
// Guard held until end of scope — write happens, then guard drops
```

Before every read (optional — only needed if you want to prevent reads during writes):
```rust
let _read_guard = context.file_lock_registry.acquire_read(&absolute_path).await;
```

---

## Sub-Agent Registry

Tracks active sub-agents and their results — the mechanism through which `wait_for_agents` works.

```rust
// In crates/rustic-agent/src/task/subagent.rs (NEW)

pub struct SubagentEntry {
    pub agent_id: String,
    pub task_id: String,         // the Rustic task ID for this subagent
    pub model: String,
    pub status: SubagentStatus,
    pub result: Option<String>,  // summary returned when complete
}

pub enum SubagentStatus {
    Running,
    Completed,
    Failed(String),
    Cancelled,
}

pub struct SubagentRegistry {
    agents: Mutex<HashMap<String, SubagentEntry>>,
    // Completion notifiers: agent_id -> notify channel
    notifiers: Mutex<HashMap<String, Arc<tokio::sync::Notify>>>,
}

impl SubagentRegistry {
    pub fn register(&self, agent_id: String, task_id: String, model: String)
    pub fn complete(&self, agent_id: &str, result: String)
    pub fn fail(&self, agent_id: &str, error: String)
    pub async fn wait_for(&self, agent_ids: &[String]) -> Vec<SubagentResult>
    pub fn list_active(&self) -> Vec<SubagentEntry>
}
```

---

## The `spawn_subagent` Tool Implementation

When the main model calls `spawn_subagent`:

```rust
// In tools/subagent.rs (NEW)

pub async fn execute_spawn_subagent(params: Value, context: &ToolContext) -> ToolOutput {
    // 1. Depth check — sub-agents cannot spawn sub-agents
    if context.agent_depth >= 1 {
        return ToolOutput::error("Sub-agents cannot spawn further sub-agents");
    }

    let agent_id = params["agent_id"].as_str()...;
    let task_desc = params["task"].as_str()...;
    let model_id = params["model"].as_str()...;
    let files: Vec<String> = params["files"].as_array()...;

    // 2. Resolve provider from model_id
    let (provider, provider_config) = resolve_provider_for_model(model_id, &context.ai_config)?;

    // 3. Create a new TaskExecutor for the sub-agent
    let sub_executor = TaskExecutor::new(provider, provider_config);

    // 4. Build sub-agent context (depth=1, same file lock registry, no spawn tool)
    let sub_context = ToolContext {
        project_root: context.project_root.clone(),
        permissions: context.permissions.clone(),
        snapshot_fn: context.snapshot_fn.clone(),
        permission_broker: context.permission_broker.clone(),
        event_tx: context.event_tx.clone(),  // events surface to same UI task
        task_id: format!("{}/subagent/{}", context.task_id, agent_id),
        file_lock_registry: Arc::clone(&context.file_lock_registry),
        agent_depth: 1,               // ← depth limit enforced here
        subagent_registry: Arc::clone(&context.subagent_registry),
    };

    // 5. Build initial messages for sub-agent
    let messages = vec![Message {
        role: Role::User,
        content: vec![ContentBlock::Text { text: task_desc.to_string() }],
    }];

    // 6. Register in SubagentRegistry
    context.subagent_registry.register(agent_id.clone(), sub_context.task_id.clone(), model_id.to_string());

    // 7. Spawn async — don't wait here (return immediately so main model can spawn more)
    let registry = Arc::clone(&context.subagent_registry);
    let agent_id_clone = agent_id.clone();
    let event_tx = context.event_tx.clone();
    tokio::spawn(async move {
        let mut msgs = messages;
        match sub_executor.run_turn(&sub_context.task_id, &mut msgs, &sub_context, &event_tx).await {
            Ok(()) => {
                // Extract last assistant text as summary
                let summary = extract_last_text(&msgs);
                registry.complete(&agent_id_clone, summary);
            }
            Err(e) => {
                registry.fail(&agent_id_clone, e.to_string());
            }
        }
    });

    // 8. Return immediately — main model continues and can spawn more agents
    ToolOutput::success(format!("Sub-agent '{}' spawned with model {}. Call wait_for_agents(['{}']) when you need its result.", agent_id, model_id, agent_id))
}
```

---

## The `wait_for_agents` Tool Implementation

```rust
pub async fn execute_wait_for_agents(params: Value, context: &ToolContext) -> ToolOutput {
    let ids: Vec<String> = if params["agent_ids"].as_array()
        .map(|a| a.iter().any(|v| v == "*")).unwrap_or(false) {
        context.subagent_registry.list_active().into_iter().map(|a| a.agent_id).collect()
    } else {
        params["agent_ids"].as_array()...
    };

    let results = context.subagent_registry.wait_for(&ids).await;

    // Format results for the main model
    let summary = results.iter().map(|r| {
        format!("## Agent: {}\nStatus: {:?}\n\n{}", r.agent_id, r.status, r.result.as_deref().unwrap_or("(no result)"))
    }).collect::<Vec<_>>().join("\n\n---\n\n");

    ToolOutput::success(summary)
}
```

---

## Parallel Tool Execution in the Executor

Currently the executor runs tools sequentially. For sub-agents to actually run in parallel, `spawn_subagent` itself must be non-blocking (it is — it returns immediately after spawning). But we should also run multiple `spawn_subagent` calls from the same response turn in parallel:

```rust
// In executor.rs — replace sequential loop with parallel for spawn_subagent calls

let spawn_calls: Vec<_> = tool_uses.iter()
    .filter(|(_, name, _)| name == "spawn_subagent")
    .collect();

let other_calls: Vec<_> = tool_uses.iter()
    .filter(|(_, name, _)| name != "spawn_subagent")
    .collect();

// Run spawn calls concurrently (each spawns a tokio task, returns immediately)
let spawn_results = futures::future::join_all(
    spawn_calls.iter().map(|(id, name, input)| {
        self.tools.execute(name, input.clone(), context)
    })
).await;

// Run other tools sequentially (they may have dependencies)
for (id, name, input) in &other_calls {
    ...
}
```

Actually simpler: since `spawn_subagent` is non-blocking anyway (it just spawns a tokio task), even running all tool calls sequentially works fine — spawning 5 agents one after another takes microseconds, then all 5 run truly in parallel in the background.

---

## Available Sub-Agent Models — Settings

### New config structure

```rust
// In config.rs
pub struct SubagentModel {
    pub provider_type: ProviderType,
    pub model_id: String,
    pub display_name: String,     // e.g. "claude-haiku-4-5 (Fast/Cheap)"
    pub enabled: bool,
}

pub struct AiConfig {
    // ... existing fields ...
    pub subagent_models: Vec<SubagentModel>,  // NEW — user-configured
}
```

### Settings UI — "Sub-Agent Models" section

Under Settings → AI Providers → Sub-Agent Models:

```
Sub-Agent Models
These models are available when the main agent spawns sub-agents.
Pick cheaper/faster models to reduce cost.

┌─────────────────────────────────────────────┐
│ ☑  claude-haiku-4-5         (Anthropic)     │
│ ☑  gpt-4o-mini              (OpenAI)        │
│ ☐  claude-sonnet-4-6        (Anthropic)     │
│ ☐  gpt-4o                   (OpenAI)        │
│ ☑  gemini-2.5-flash         (Google)        │
└─────────────────────────────────────────────┘
[+ Add model]
```

The list is populated from already-configured providers. User just checks/unchecks which ones sub-agents can use.

### Injecting into tool definition

When building the `spawn_subagent` tool definition, dynamically inject the available models list into the description:

```rust
fn spawn_subagent_definition(subagent_models: &[SubagentModel]) -> ToolDef {
    let model_list = subagent_models.iter()
        .filter(|m| m.enabled)
        .map(|m| format!("- {} ({})", m.model_id, m.display_name))
        .collect::<Vec<_>>()
        .join("\n");

    ToolDef {
        name: "spawn_subagent".into(),
        description: format!(
            "Spawn a sub-agent to execute a focused task in parallel.\n\
             Available models for sub-agents:\n{}", model_list
        ),
        parameters: spawn_subagent_schema(),
    }
}
```

This means the main model always sees the up-to-date list of available sub-agent models directly in the tool description.

---

## Workflow Orchestration with Sub-Agents

When a workflow is triggered via `/workflow-name`, the system:

1. Loads the workflow markdown body
2. Prepends to user message: `[Workflow: <name>]\n<workflow body>\n\n[Your task]:\n<user message>`
3. Adds a system prompt injection: *"You are acting as an orchestrator for this workflow. Analyze the steps, identify which can run in parallel, spawn sub-agents as appropriate, then synthesize their results."*

This requires no special workflow executor — the main model's own reasoning handles orchestration. The sub-agent tools give it the mechanism.

Example workflow:

```markdown
---
name: code-review-and-test
description: Review code changes and run tests in parallel
---

# Code Review + Test Workflow

1. PARALLEL:
   - Review the changed files for code quality (use a sub-agent)
   - Run the test suite (use a sub-agent)
2. SEQUENTIAL: Synthesize the review feedback and test results into a summary
3. SEQUENTIAL: If tests failed, identify which review issues may be related
```

The main model reads this, recognizes steps 1a and 1b are marked parallel, spawns two sub-agents, waits, then does steps 2 and 3 itself.

---

## UI Changes for Sub-Agents

### Chat view — sub-agent activity

When sub-agents are active, show a collapsible "Sub-agents" section in the chat:

```
┌──────────────────────────────────────────────┐
│ ▼ Sub-agents (2 running)                     │
│   ● refactor-auth     claude-haiku    Running │
│   ● write-tests       gpt-4o-mini    Running │
│   ✓ analyze-deps      gemini-flash   Done    │
└──────────────────────────────────────────────┘
```

Each sub-agent row is expandable to show its streaming output.

### New TaskEvent types for UI

```rust
pub enum TaskEvent {
    // ... existing ...
    SubagentSpawned { task_id: String, agent_id: String, model: String },
    SubagentCompleted { task_id: String, agent_id: String, result: String },
    SubagentFailed { task_id: String, agent_id: String, error: String },
    SubagentTextDelta { task_id: String, agent_id: String, text: String }, // streaming from subagent
}
```

---

## Updated Architecture Diagram

```
AppState
├── AgentTask (main)
│   ├── messages[]
│   ├── permissions: PermissionLevel
│   ├── cost: TaskCost
│   └── active_subagents: Vec<SubagentEntry>  ← NEW
│
├── SubagentRegistry (Arc, shared)  ← NEW
│   ├── agents: HashMap<agent_id, SubagentEntry>
│   └── notifiers: HashMap<agent_id, Notify>
│
├── FileLockRegistry (Arc, shared)  ← NEW
│   └── locks: HashMap<PathBuf, Arc<RwLock<()>>>
│
└── PermissionBroker (Arc, shared)  ← from previous plan

TaskExecutor (main, depth=0)
├── Tools: all built-ins + spawn_subagent + wait_for_agents + list_active_agents + cancel_agent
└── run_turn() → loop:
    ├── call provider API
    ├── for each tool_use:
    │   ├── spawn_subagent → tokio::spawn(TaskExecutor(depth=1))
    │   ├── wait_for_agents → SubagentRegistry::wait_for()
    │   └── other tools → execute normally
    └── continue until no tool calls

TaskExecutor (subagent, depth=1)
├── Tools: all built-ins ONLY (no spawn_subagent, no wait_for_agents)
├── Same FileLockRegistry (prevents file conflicts)
├── Same PermissionBroker (approval requests go to same UI)
└── Streams events back to same Tauri event channel (different task_id prefix)
```

---

## Summary of New Components

| Component | Location | Description |
|---|---|---|
| `FileLockRegistry` | `task/file_lock.rs` | Per-file async RwLock, shared across all agents |
| `SubagentRegistry` | `task/subagent.rs` | Tracks active sub-agents, provides wait mechanism |
| `spawn_subagent` tool | `tools/subagent.rs` | Spawns sub-agent, non-blocking, returns immediately |
| `wait_for_agents` tool | `tools/subagent.rs` | Blocks main agent until specified sub-agents complete |
| `list_active_agents` tool | `tools/subagent.rs` | Lists running sub-agents and their status |
| `SubagentModel` config | `config.rs` | User-configured models available for sub-agents |
| Sub-agent models settings UI | Settings → AI | Checkbox list to enable/disable models for sub-agents |
| Sub-agent activity widget | `chat-view.js` | Collapsible panel showing sub-agent status/output |

---

## Constraints & Guardrails

| Rule | Enforcement |
|---|---|
| Sub-agents cannot spawn sub-agents | `agent_depth >= 1` check in spawn_subagent tool |
| Max N concurrent sub-agents | Configurable limit (default: 5). Return error if exceeded. |
| File write lock timeout | 30 seconds — prevents deadlock if a sub-agent crashes |
| Sub-agent context is isolated | New message Vec per sub-agent — no shared memory |
| Sub-agents use same permission mode | Inherited from parent task, cannot escalate |
| Sub-agent results are summaries | Only last assistant text returned to parent — intermediate steps not added to parent context |

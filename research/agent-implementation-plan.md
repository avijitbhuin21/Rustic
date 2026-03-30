# Agent Implementation Plan
> Research & Planning Document for Rustic Agent Features
> Date: 2026-03-30

This document covers the implementation plan for:
1. Expense / token tracking
2. Memory (memory.md per project)
3. Permission modes (Chat / ManualEdit / AutoEdit / FullAuto)
4. Model switching mid-chat with separator

---

## Current State Assessment

```
crates/rustic-agent/src/
├── task/
│   ├── executor.rs       — agentic loop (runs tool calls, no cost tracking)
│   ├── permissions.rs    — PermissionLevel: Admin | ReadWrite | ReadOnly (3 levels, needs refactor)
│   └── mod.rs
├── tools/
│   ├── file_ops.rs       — file read/write
│   ├── search.rs         — search tools
│   └── terminal.rs       — shell execution
├── provider/             — Claude / OpenAI / Compatible providers
│   └── (TokenUsage struct exists but not accumulated per task)
├── mcp/                  — MCP manager
└── config.rs             — AiConfig, ProviderEntry
```

**Gaps to fill:**
- `PermissionLevel` has 3 levels → needs 4 specific modes with different rules per operation type
- No permission request/approval flow (executor currently does not ask UI before operations)
- No cost/token accumulation on the task
- No memory.md loading or tooling
- No model-switch event in message stream

---

## 1. Expense / Token Tracking

### What Claude Code Does
Claude Code tracks tokens per turn using the provider's `usage` response field. It shows:
- Input tokens (cached vs non-cached separately for Claude)
- Output tokens
- Estimated cost based on model pricing table
- Running total for the session

### Rustic's Implementation Plan

#### Data structures (in `rustic-agent`)

```rust
// In provider/mod.rs — already exists, verify fields
pub struct TokenUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_read_tokens: Option<u32>,   // Claude only
    pub cache_write_tokens: Option<u32>,  // Claude only
}

// New: accumulated per task
pub struct TaskCost {
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cache_read_tokens: u64,
    pub estimated_cost_usd: f64,          // calculated from model pricing
    pub turn_count: u32,
}
```

#### Model pricing table (in `config.rs`)

```rust
pub struct ModelPricing {
    pub input_per_million: f64,        // USD per 1M input tokens
    pub output_per_million: f64,       // USD per 1M output tokens
    pub cache_read_per_million: f64,   // USD per 1M cache read tokens (Claude)
}

// Hardcoded table, updated as needed:
fn get_pricing(model: &str) -> ModelPricing { ... }
```

#### Accumulation in executor
- After each `provider.chat()` call, extract `response.usage` and add to `TaskCost`
- Emit a new `TaskEvent::CostUpdate { task_id, cost: TaskCost }` event
- Store `TaskCost` in `AgentTask` in `AppState`

#### New Tauri command
```rust
pub fn get_task_cost(state, task_id) -> Result<TaskCost, String>
```

#### UI Display
- Show in the chat header or status bar below the chat: `~1,234 tokens · $0.003`
- Update in real-time as each turn completes
- On hover: tooltip showing breakdown (input / output / cache / turns)
- Mode indicator and cost share the same bottom bar of the chat view

---

## 2. Memory (memory.md)

### What Claude Code Does
Claude Code has an auto-memory system:
- Memory files live at `~/.claude/projects/<project>/memory/*.md`
- At session start, MEMORY.md (the index) is loaded into context automatically
- Individual memory files are loaded when the index references them
- The model can write/update memory via tools
- Memory is NOT sent on every request — loaded at session start only

### Rustic's Approach (Simpler / More Direct)

One file per project: `<project>/.rustic/memory.md`

**Loading:**
- At new chat session start (new task created): read `memory.md` if it exists → inject as a system message at the top of the conversation with a header like `[Project Memory]\n<contents>`
- NOT re-injected on every turn — only at session start
- Model can explicitly re-read it via `read_memory` tool if needed during a session

**Tools the model gets:**
```
read_memory()              → reads .rustic/memory.md, returns contents
write_memory(content)      → overwrites .rustic/memory.md completely
update_memory(find, replace) → replaces a section in memory.md
```

Actually even simpler: since we already have file read/write tools, we could just tell the model in the system prompt that `.rustic/memory.md` is its persistent memory file, and it can use the existing `read_file` / `write_file` / `edit_file` tools to manage it. No special memory tools needed.

**System prompt addition:**
```
You have access to a persistent memory file at .rustic/memory.md (relative to the project root).
Use it to store facts, decisions, preferences, and context you want to remember across sessions.
At the start of each session, this file is pre-loaded into your context.
Read it with read_file, update it with edit_file, or rewrite it with write_file.
Keep it under 500 lines. Use markdown with clear sections.
```

**Auto-loading at task creation (in `send_message` or `create_task`):**

```rust
// When building the initial messages for a new task:
let memory_path = project_root.join(".rustic/memory.md");
if memory_path.exists() {
    let memory_content = std::fs::read_to_string(&memory_path).unwrap_or_default();
    if !memory_content.trim().is_empty() {
        // Prepend as first user message or system block
        messages.insert(0, Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: format!("[Project Memory]\n{}", memory_content)
            }]
        });
        // Follow with a fake assistant ack to keep alternating user/assistant pattern
        messages.insert(1, Message {
            role: Role::Assistant,
            content: vec![ContentBlock::Text {
                text: "Memory loaded. I'll reference this context as needed.".into()
            }]
        });
    }
}
```

**UI:**
- Memory indicator in agent panel header (icon + "Memory" label, clickable to open memory.md in editor)
- When model writes to memory.md, a subtle "Memory updated" toast appears

---

## 3. Permission Modes

### Current State
`PermissionLevel: Admin | ReadWrite | ReadOnly` — 3 levels, no UI permission approval flow.

### New 4-Mode System

```
Chat         → read-only. No file writes/creates/deletes. Commands need approval.
ManualEdit   → file ops need approval each time. Commands need approval.
AutoEdit     → file ops auto-approved. Commands need approval.
FullAuto     → everything auto-approved. No prompts.
```

### Operation Permission Matrix

| Operation | Chat | ManualEdit | AutoEdit | FullAuto |
|---|---|---|---|---|
| Read file | Allow | Allow | Allow | Allow |
| Search files | Allow | Allow | Allow | Allow |
| Write file | Deny | Ask UI | Allow | Allow |
| Create file | Deny | Ask UI | Allow | Allow |
| Delete file | Deny | Ask UI | Allow | Allow |
| Edit file | Deny | Ask UI | Allow | Allow |
| Run command | Ask UI | Ask UI | Ask UI | Allow |
| Read memory | Allow | Allow | Allow | Allow |
| Write memory | Deny | Ask UI | Allow | Allow |

### Updated `PermissionLevel` enum

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum PermissionLevel {
    Chat,        // read-only; commands ask user
    ManualEdit,  // all writes ask user; commands ask user
    AutoEdit,    // writes auto-allowed; commands ask user
    FullAuto,    // nothing requires approval
}

impl Default for PermissionLevel {
    fn default() -> Self {
        Self::ManualEdit  // safe default
    }
}
```

### The Approval Flow (Most Complex Part)

When `ManualEdit` mode is active, the executor needs to pause, ask the UI, and wait for a response before executing a write or command.

#### New event types

```rust
pub enum TaskEvent {
    // ... existing events ...

    /// Executor is requesting permission — UI must respond via approve_tool_call command
    PermissionRequest {
        task_id: String,
        request_id: String,          // unique ID for this request
        operation: PermissionOp,     // what kind of operation
        description: String,         // human-readable: "Write to src/main.rs"
        preview: Option<String>,     // optional: diff preview or command string
    },
}

pub enum PermissionOp {
    WriteFile { path: String },
    CreateFile { path: String },
    DeleteFile { path: String },
    RunCommand { command: String },
}
```

#### New Tauri command

```rust
#[tauri::command]
pub fn respond_to_permission(
    state: State<'_, AppState>,
    task_id: String,
    request_id: String,
    approved: bool,
) -> Result<(), String>
```

#### Executor-side: one-shot channel per request

```rust
// In AppState (or passed via ToolContext)
pub struct PermissionBroker {
    // Map of request_id -> oneshot sender
    pending: Mutex<HashMap<String, oneshot::Sender<bool>>>,
}

impl PermissionBroker {
    pub async fn request(&self, event_tx: &Sender<TaskEvent>, task_id: &str, op: PermissionOp) -> bool {
        let request_id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(request_id.clone(), tx);

        let _ = event_tx.send(TaskEvent::PermissionRequest {
            task_id: task_id.to_string(),
            request_id,
            operation: op,
            description: op.describe(),
            preview: None,
        });

        rx.await.unwrap_or(false)  // false = denied if channel dropped
    }

    pub fn respond(&self, request_id: &str, approved: bool) {
        if let Some(tx) = self.pending.lock().unwrap().remove(request_id) {
            let _ = tx.send(approved);
        }
    }
}
```

#### In tool execution

```rust
// In file_ops.rs, before writing:
async fn write_file(path, content, context) -> ToolOutput {
    if context.permissions == PermissionLevel::Chat {
        return ToolOutput::error("Write not allowed in Chat mode");
    }
    if context.permissions == PermissionLevel::ManualEdit {
        let approved = context.permission_broker
            .request(&context.event_tx, &context.task_id, PermissionOp::WriteFile { path: path.clone() })
            .await;
        if !approved {
            return ToolOutput::error("Write denied by user");
        }
    }
    // proceed with write...
}
```

### UI for Permission Approval

When `agent-permission-request` event fires:

- Show a **permission dialog** at the bottom of the chat view (above the input box)
- Shows: operation type icon + description + optional preview
- Two buttons: **Allow** and **Deny**
- Optionally: **Allow All** (temporarily switches to AutoEdit for this session)
- If user doesn't respond within 60 seconds → auto-deny

```
┌─────────────────────────────────────────────────────┐
│  ✏  Write file: src/components/agent/chat-view.js   │
│  [Show diff ▼]                     [Deny]  [Allow]  │
└─────────────────────────────────────────────────────┘
```

### Mode Switcher in UI

- Show current mode in the chat input toolbar (e.g., `● AutoEdit ▾`)
- Click → dropdown with 4 options
- Changing mode takes effect immediately (stored in AgentTask, not globally — per task)
- Persist per-project default in settings

---

## 4. Model Switching Mid-Chat

### Design

- User can switch model at any point during a conversation
- The message history continues — just the next API call uses the new model+provider
- A **separator message** is inserted into the UI (not sent to the LLM):

```
──────────── Model: claude-opus-4-6 ────────────
```

### Implementation

#### New `ContentBlock` variant (UI-only, never sent to LLM)

```rust
pub enum ContentBlock {
    Text { text: String },
    ToolUse { id: String, name: String, input: serde_json::Value },
    ToolResult { tool_use_id: String, content: String, is_error: bool },
    ModelSwitch { from_model: String, to_model: String },  // NEW — UI display only
}
```

When serializing messages to send to the LLM API, filter out `ModelSwitch` blocks entirely.

#### New Tauri command

```rust
#[tauri::command]
pub fn switch_model(
    state: State<'_, AppState>,
    task_id: String,
    provider_type: String,  // "Claude" | "OpenAi" | "Gemini" | "Compatible"
    model: String,
) -> Result<(), String> {
    let mut agent = state.agent.lock().unwrap();
    let task = agent.tasks.get_mut(&task_id).ok_or("Task not found")?;

    let from_model = task.info.model.clone();

    // Update model on the task
    task.info.model = model.clone();
    task.info.provider_type = provider_type;

    // Inject separator into message history
    task.messages.push(Message {
        role: Role::System,  // or a new "separator" role handled specially in UI
        content: vec![ContentBlock::ModelSwitch {
            from_model,
            to_model: model,
        }],
    });

    Ok(())
}
```

Actually using `Role::System` for separators may clash with system prompts. Better approach: use a dedicated `Role::Separator` in Rustic, or mark the message with a special `is_ui_only: bool` flag so the serializer knows to skip it.

#### UI in chat-view.js

```javascript
// When rendering a message with a ModelSwitch block:
if (block.type === 'model_switch') {
    const sep = el('div', { class: 'chat-model-switch' });
    sep.appendChild(el('span', { class: 'chat-model-switch__line' }));
    sep.appendChild(el('span', { class: 'chat-model-switch__label' },
        `Model: ${block.to_model}`
    ));
    sep.appendChild(el('span', { class: 'chat-model-switch__line' }));
    msgEl.appendChild(sep);
}
```

#### Model switcher in chat input area

- Add a model selector dropdown to the chat input toolbar (left of the send button)
- Shows current model abbreviated (e.g., `claude-sonnet-4-6` → `Sonnet 4.6`)
- Clicking opens a dropdown grouped by provider
- Selecting calls `switch_model` command

---

## 5. Updated Architecture Overview

### New fields on `AgentTask`

```rust
pub struct AgentTask {
    pub info: TaskInfo,
    pub messages: Vec<Message>,
    pub permissions: PermissionLevel,     // existing
    pub cost: TaskCost,                   // NEW — accumulated token/cost data
    pub memory_loaded: bool,              // NEW — whether memory.md was pre-loaded
}
```

### New fields on `TaskInfo`

```rust
pub struct TaskInfo {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub status: TaskStatus,
    pub provider_type: String,
    pub model: String,
    pub permissions: PermissionLevel,     // NEW — expose to UI
    pub cost: TaskCost,                   // NEW — expose to UI
}
```

### New `ToolContext` fields

```rust
pub struct ToolContext {
    pub project_root: PathBuf,
    pub permissions: PermissionLevel,
    pub snapshot_fn: Option<SnapshotFn>,
    pub permission_broker: Arc<PermissionBroker>,  // NEW — for ManualEdit approval flow
    pub event_tx: mpsc::UnboundedSender<TaskEvent>, // NEW — needed by broker
    pub task_id: String,                           // NEW — needed by broker
}
```

### Updated `TaskEvent`

```rust
pub enum TaskEvent {
    TextDelta { task_id: String, text: String },
    ToolUse { task_id: String, tool_name: String, tool_input: serde_json::Value },
    ToolResult { task_id: String, tool_use_id: String, output: String, is_error: bool },
    StatusChange { task_id: String, status: TaskStatus },
    MessageComplete { task_id: String, message: Message },
    CostUpdate { task_id: String, cost: TaskCost },               // NEW
    PermissionRequest { task_id: String, request_id: String,
                        operation: PermissionOp, description: String,
                        preview: Option<String> },                // NEW
}
```

---

## 6. New Tauri Commands Summary

| Command | Description |
|---|---|
| `get_task_cost(task_id)` | Returns `TaskCost` for a task |
| `switch_model(task_id, provider_type, model)` | Switch model mid-chat, inject separator |
| `set_task_permissions(task_id, level)` | Change permission mode for a task |
| `respond_to_permission(task_id, request_id, approved)` | Respond to a ManualEdit approval request |
| `get_memory(project_id)` | Read the project's memory.md |
| `clear_memory(project_id)` | Clear memory.md |

---

## 7. Implementation Order (Suggested)

Each step is self-contained and testable:

**Step 1 — Permission modes refactor** (backend only)
- Replace `PermissionLevel` enum with 4-level version
- Update all permission checks in `file_ops.rs` and `terminal.rs`
- Update `set_permissions` command and UI mode selector

**Step 2 — Permission approval flow** (backend + frontend)
- Add `PermissionBroker` to `AppState`
- Add `PermissionRequest` to `TaskEvent`
- Add `respond_to_permission` command
- Add approval UI in chat-view (inline at bottom of chat)

**Step 3 — Token/cost tracking** (backend + frontend)
- Verify `TokenUsage` is populated by all providers
- Add `TaskCost` accumulation in executor
- Add `CostUpdate` event
- Add cost display in chat header

**Step 4 — Memory** (backend + frontend)
- Load `.rustic/memory.md` at task creation if it exists
- Add memory indicator to agent panel header
- Add system prompt instructions about memory

**Step 5 — Model switching** (backend + frontend)
- Add `ModelSwitch` content block
- Add `switch_model` command
- Add model selector dropdown to chat input toolbar
- Add separator rendering in chat-view

---

## 8. Frontend Changes Summary

### `chat-view.js`
- Render `model_switch` content blocks as visual separators
- Show permission approval widget (above input area) when `agent-permission-request` fires
- Show token count + cost in chat header
- Add model selector dropdown to input toolbar
- Add mode indicator (Chat / ManualEdit / AutoEdit / FullAuto) to input toolbar

### `agent-panel.js`
- Add memory indicator in header (icon, clicks to open memory.md)
- Show cost total per task in task list

### New event listeners
- `agent-permission-request` → show approval widget
- `agent-cost-update` → update cost display
- `agent-task-status` → already exists

---

## 9. How Memory Compares to Claude Code

Claude Code's auto-memory system:
- Lives in `~/.claude/projects/<path>/memory/` — multiple files
- Has an index file (MEMORY.md) that maps to individual memory files by topic
- Each file has YAML frontmatter (name, description, type: user|feedback|project|reference)
- First 200 lines of MEMORY.md loaded into every session
- Individual memory files loaded when referenced

Rustic's approach (simpler, still effective):
- Single file: `<project>/.rustic/memory.md`
- No index, no frontmatter — just plain markdown with sections
- Loaded at session start, not every turn
- Model writes directly using existing file tools
- Per-project (same as Claude Code's per-project memory)

**Why simpler is better here:** Claude Code's multi-file system solves the "200 line index limit" problem by splitting memory into topic files. For a first implementation, a single file under 500 lines is fine. Can add multi-file support later if needed.

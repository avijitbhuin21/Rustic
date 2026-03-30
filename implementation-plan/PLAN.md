# Rustic Agent — Implementation Plan

Each phase is self-contained and testable before moving to the next.
Phases build on each other — complete in order.

---

## Phase 0 — Task Completion Tool
**Scope:** Backend + frontend. Implement first — everything else depends on a clean stop signal.

### 0.1 — task_complete tool definition
`crates/rustic-agent/src/tools/mod.rs`

Add `task_complete` to `BuiltinTools::definitions()`:
```rust
ToolDef {
    name: "task_complete".into(),
    description: "Signal that the task is fully done. Call this immediately when complete. \
                  Stops execution and shows the user a structured summary. \
                  Do NOT send a plain text message saying you're done — use this tool.".into(),
    parameters: json!({
        "type": "object",
        "required": ["summary", "changes"],
        "properties": {
            "summary": { "type": "string", "description": "What was accomplished" },
            "changes": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["file", "action", "description"],
                    "properties": {
                        "file":        { "type": "string" },
                        "action":      { "type": "string", "enum": ["created","modified","deleted","read-only"] },
                        "description": { "type": "string" }
                    }
                }
            },
            "notes": { "type": "string", "description": "Optional warnings or follow-up suggestions" }
        }
    })
}
```

### 0.2 — New TaskEvent variant
`crates/rustic-agent/src/task/executor.rs`
```rust
pub enum TaskEvent {
    // ... existing variants ...
    TaskComplete {
        task_id: String,
        summary: String,
        changes: Vec<FileChange>,   // reuse existing FileChange struct from checkpoint
        notes: Option<String>,
    },
}

// FileChange already exists in checkpoint module — reuse:
// pub struct FileChange { pub file_path: String, pub change_type: ChangeType }
// Extend ChangeType or add a description field
```

### 0.3 — Handle in executor loop
`executor.rs` — in the tool dispatch section:
```rust
"task_complete" => {
    // Parse summary, changes, notes from tool_input
    // Seal the checkpoint
    if let Some(snap) = &context.snapshot_fn { /* finalise */ }
    // Emit completion event
    let _ = event_tx.send(TaskEvent::TaskComplete { task_id, summary, changes, notes });
    // Return a ToolOutput so the API gets a valid tool_result
    // Then BREAK the outer loop immediately — no further provider calls
    tool_results.push(build_complete_result(&tool_id));
    messages.push(Message { role: Role::User, content: tool_results });
    break; // ← exit the agentic loop
}
```

The `break` is the key — when `task_complete` is called, the loop exits after adding the tool result. No further provider call is made.

### 0.4 — Tauri event forwarding
`src-tauri/src/commands/agent.rs`
```rust
TaskEvent::TaskComplete { task_id, summary, changes, notes } => {
    let _ = app_events.emit("agent-task-complete", AgentTaskCompleteEvent {
        task_id, summary, changes, notes
    });
}
```

### 0.5 — Completion card in chat-view.js
Listen for `agent-task-complete` event. Render a styled completion card:
```javascript
function renderCompletionCard(data) {
    const card = el('div', { class: 'chat-completion-card' });

    // Header
    const header = el('div', { class: 'chat-completion-card__header' });
    header.appendChild(icon('M5 13l4 4L19 7', 16)); // checkmark
    header.appendChild(el('span', {}, 'Task complete'));
    card.appendChild(header);

    // Summary
    card.appendChild(el('p', { class: 'chat-completion-card__summary' }, data.summary));

    // Changes list
    if (data.changes?.length) {
        const changesEl = el('div', { class: 'chat-completion-card__changes' });
        changesEl.appendChild(el('div', { class: 'chat-completion-card__changes-title' }, 'Changes'));
        for (const change of data.changes) {
            const row = el('div', { class: `chat-completion-card__change chat-completion-card__change--${change.action}` });
            const actionIcon = { created: '✚', modified: '✏', deleted: '✕', 'read-only': '◎' }[change.action] || '·';
            row.appendChild(el('span', { class: 'change-icon' }, actionIcon));
            row.appendChild(el('span', { class: 'change-file' }, change.file));
            row.appendChild(el('span', { class: 'change-desc' }, change.description));
            changesEl.appendChild(row);
        }
        card.appendChild(changesEl);
    }

    // Notes
    if (data.notes) {
        const notesEl = el('div', { class: 'chat-completion-card__notes' });
        notesEl.appendChild(el('span', { class: 'notes-label' }, 'Notes'));
        notesEl.appendChild(el('p', {}, data.notes));
        card.appendChild(notesEl);
    }

    return card;
}
```

### 0.6 — Sub-agent completion
When a sub-agent calls `task_complete`:
- Its summary + changes become the result payload in `SubagentRegistry::complete()`
- Replaces the previous "extract last assistant text" heuristic — always structured now
- Main model receives: summary + changes list (formatted), not raw conversation text

### 0.7 — System prompt
Add to system prompt:
```
When your task is fully complete, call task_complete immediately.
Do not send a plain-text "I'm done" message — the tool is the only valid completion signal.
Do not ask follow-up questions after calling it — wait for the user.
Include every file you created, modified, or deleted in the changes array.
```

**Done when:** Calling `task_complete` immediately stops the loop, emits the event, and a styled completion card renders in the chat. Sub-agent results arrive as structured summaries.

---

## Phase 1 — Permission Modes Refactor
**Scope:** Backend only. No new UI yet.

### 1.1 — Replace PermissionLevel enum
`crates/rustic-agent/src/task/permissions.rs`
- Replace `Admin | ReadWrite | ReadOnly` with `Chat | ManualEdit | AutoEdit | FullAuto`
- Update `check_permission()` logic per the operation matrix in requirements.md
- Add `Action::Write` sub-variants if needed (CreateFile, EditFile, DeleteFile, RunCommand)

### 1.2 — Update all permission checks in tools
- `file_ops.rs`: Chat → deny writes; ManualEdit → placeholder (returns PERMISSION_PENDING for now); AutoEdit/FullAuto → allow
- `terminal.rs`: Chat/ManualEdit/AutoEdit → placeholder; FullAuto → allow

### 1.3 — Update AppState and Tauri commands
- `set_permissions` command: accept new level strings
- `create_task`: default to ManualEdit
- Update `project_permissions` default

### 1.4 — Mode selector UI
`src/components/agent/chat-view.js`
- Add mode pill to chat input toolbar: `● ManualEdit ▾`
- Dropdown with 4 options
- Calls `set_task_permissions(task_id, level)` on change
- Persist per-project default via `set_project_default_mode(project_id, level)` setting

**Done when:** Switching modes in UI changes the permission level, Chat mode blocks all write tool calls with a clear error.

---

## Phase 2 — ManualEdit Approval Flow
**Scope:** Backend + frontend. Most complex phase.

### 2.1 — PermissionBroker
`crates/rustic-agent/src/task/permission_broker.rs`
```rust
pub struct PermissionBroker {
    pending: Mutex<HashMap<String, oneshot::Sender<bool>>>,
}
impl PermissionBroker {
    pub async fn request(&self, event_tx, task_id, op: PermissionOp) -> bool
    pub fn respond(&self, request_id: &str, approved: bool)
}
```

### 2.2 — Add PermissionRequest to TaskEvent
- New variant: `PermissionRequest { task_id, request_id, operation: PermissionOp, description, preview }`
- New enum: `PermissionOp { WriteFile(path), CreateFile(path), DeleteFile(path), RunCommand(cmd) }`

### 2.3 — Wire broker into ToolContext
- Add `permission_broker: Arc<PermissionBroker>` and `event_tx` to `ToolContext`
- Pass from `send_message` command where executor is called
- Add `PermissionBroker` to `AppState`

### 2.4 — Use broker in tools
- In `file_ops.rs` and `terminal.rs`: if ManualEdit → `broker.request().await` before executing
- If `false` returned → return `ToolOutput` with code `PERMISSION_DENIED`

### 2.5 — New Tauri command
```rust
pub fn respond_to_permission(state, task_id, request_id, approved: bool) -> Result<(), String>
```
Calls `broker.respond(request_id, approved)`

### 2.6 — Approval widget in chat-view.js
- Listen for `agent-permission-request` Tauri event
- Show inline widget above input area:
  ```
  ┌──────────────────────────────────────────────┐
  │ ✏  Write: src/auth.rs        [Deny] [Allow]  │
  └──────────────────────────────────────────────┘
  ```
- On Allow: call `respond_to_permission(task_id, request_id, true)`
- On Deny: call `respond_to_permission(task_id, request_id, false)`
- Auto-deny after 60 seconds with countdown

**Done when:** In ManualEdit mode, every write and command shows the approval widget and waits for user response.

---

## Phase 3 — Token / Cost Tracking
**Scope:** Backend + frontend. Self-contained.

### 3.1 — Verify TokenUsage is populated
Check all 3 providers return non-zero `input_tokens` / `output_tokens` in response.

### 3.2 — TaskCost struct
`crates/rustic-agent/src/task/cost.rs`
```rust
pub struct TaskCost {
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cache_read_tokens: u64,
    pub estimated_cost_usd: f64,
    pub turn_count: u32,
}
pub fn calculate_cost(model: &str, usage: &TokenUsage) -> f64  // pricing table
```

### 3.3 — Accumulate in executor
- After each `provider.chat()` call: add usage to `task.cost`
- Emit `TaskEvent::CostUpdate { task_id, cost }` after every turn

### 3.4 — Expose via Tauri command
`get_task_cost(task_id) -> Result<TaskCost, String>`

### 3.5 — Cost display in chat-view.js
- Listen for `agent-cost-update`
- Show in chat header: `~1,234 tokens · $0.003`
- Hover tooltip: breakdown (input / output / cache / turns)

**Done when:** Token count and estimated cost update live after each agent turn.

---

## Phase 4 — Shell Detection + Tool Output Caps
**Scope:** Backend. Improves reliability for all subsequent phases.

### 4.1 — Shell detection at task start
- In `send_message` on first message (or at `create_task`): run `uname -s || echo Windows`
- Store `ShellEnv { os: String, shell: String }` on `AgentTask`
- Inject one line into system prompt: `"Environment: bash on Linux"` / `"Environment: PowerShell on Windows"`

### 4.2 — Tool output hard cap
- In `terminal.rs`: after command runs, cap output at 16KB
- If truncated, append: `\n[Output truncated at 16KB — {N} more lines. Use head/tail/grep to filter.]`

### 4.3 — Structured error codes
- All tool `ToolOutput` errors prepend a code: `STALE_READ:`, `CONTENT_DELETED:`, `PERMISSION_DENIED:`, `LOCK_TIMEOUT:`, `ALREADY_APPLIED:`, `OUTPUT_TRUNCATED:`
- Update system prompt to teach model each code and its recovery action

### 4.4 — Turn budget
- Add `TurnBudget { max: u32, used: u32 }` to `ToolContext`
- Increment on each provider call in executor
- At `max - 5`: inject warning message into conversation
- At `max`: set status `TurnLimitReached`, break loop
- Expose `extend_turn_budget(task_id, additional: u32)` Tauri command
- UI: show warning banner with "Continue (+20)" button

**Done when:** Long-running agent automatically warns at turn 45, stops at 50, user can extend.

---

## Phase 5 — File Tools Redesign
**Scope:** Backend. Replaces existing file_ops.rs.

### 5.1 — FileLockRegistry
`crates/rustic-agent/src/task/file_lock.rs`
```rust
pub struct FileLockRegistry {
    locks: Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>,
}
impl FileLockRegistry {
    pub async fn atomic_rmw<F>(&self, path: &Path, f: F) -> Result<()>
    // holds lock for entire duration of closure
}
```
30-second timeout → `LOCK_TIMEOUT` error.

### 5.2 — Rewrite file_ops.rs
Remove: `write_file` for existing files, `read_file` as a tool (use terminal instead)

Add:
- `create_file(path, content)` — rejects if file has content (`FILE_HAS_CONTENT` error)
- `edit_file(path, old_string, new_string, hint_line?)` — atomic RMW, idempotency check, STALE_READ / CONTENT_DELETED errors with bounded context
- `apply_patch(path, hunks[])` — atomic multi-hunk, rollback on failure
- `insert_lines(path, after_line, content)` — atomic line insert
- `delete_lines(path, start_line, end_line)` — atomic line delete

### 5.3 — Error response bounds
- STALE_READ: return ±150 lines around detected location (cap 300 lines / 8KB)
- CONTENT_DELETED: return ±150 lines around `hint_line` (cap 300 lines / 8KB) + nearby symbol list
- All errors include structured code prefix

### 5.4 — System prompt update
Add file navigation workflow instructions:
- grep -n / awk 'NR>=X&&NR<=Y{print NR": "$0}' patterns
- PowerShell equivalents
- Never read >300 lines at once
- Always note the line number before editing (use hint_line)
- On STALE_READ: retry using provided context
- On CONTENT_DELETED: do not retry, escalate to orchestrator

**Done when:** All file edits go through locked RMW. No full-file overwrites possible on existing files.

---

## Phase 6 — Memory (memory.md)
**Scope:** Backend + frontend. Small and self-contained.

### 6.1 — Load memory at task start
In `send_message` (on first message for a task):
- Check for `<project_root>/.rustic/memory.md`
- If exists and non-empty: prepend two messages to conversation:
  - User: `[Project Memory]\n<contents>`
  - Assistant: `Memory loaded. I'll reference this context as needed.`
- Set `task.memory_loaded = true`

### 6.2 — System prompt addition
```
You have a persistent memory file at .rustic/memory.md (project root).
Use it to store facts, decisions, and preferences to remember across sessions.
It was pre-loaded at the start of this session. Use run_command to read it,
and edit_file/create_file to update it. Keep under 500 lines.
```

### 6.3 — Memory indicator in agent-panel.js
- Small memory icon in project header
- Click opens `.rustic/memory.md` in the main editor
- If file doesn't exist yet: create empty file then open

### 6.4 — Memory update toast
- When `edit_file` or `create_file` targets `.rustic/memory.md`: emit `MemoryUpdated` event
- UI shows subtle toast: "Memory updated"

**Done when:** Memory file auto-loads at session start, model can update it via file tools.

---

## Phase 7 — Model Switching Mid-Chat
**Scope:** Backend + frontend.

### 7.1 — ModelSwitch content block
In `provider/mod.rs`:
```rust
pub enum ContentBlock {
    // existing...
    ModelSwitch { from_model: String, to_model: String },  // UI-only, never serialized to API
}
```
Update all serializers to skip `ModelSwitch` blocks when sending to LLM.

### 7.2 — switch_model Tauri command
```rust
pub fn switch_model(state, task_id, provider_type: String, model: String) -> Result<(), String>
```
- Updates `task.info.model` and `task.info.provider_type`
- Pushes `ModelSwitch` message to `task.messages`

### 7.3 — Model selector in chat-view.js
- Dropdown left of send button showing current model abbreviated
- Grouped by provider
- On select: calls `switch_model`
- Renders `ModelSwitch` block as:
  ```
  ──────────── Model: claude-opus-4-6 ────────────
  ```

**Done when:** User can switch model mid-conversation, separator shows in chat, subsequent turns use new model.

---

## Phase 8 — MCP Config Upgrade
**Scope:** Backend + frontend.

### 8.1 — TOML config parser
`crates/rustic-agent/src/config.rs` or new `config/mcp.rs`:
- Parse `~/.rustic/config.toml` and `<project>/.rustic/mcp.toml`
- Support stdio (command/args/env/cwd) and HTTP (url/headers) transports
- Fields: enabled, trust, allowed_tools, disabled_tools, timeout_ms, required
- Env var expansion: `${VAR}` and `${VAR:-default}`

### 8.2 — JSON compat reader
- Also read `<project>/.mcp.json` (Claude Code format, `mcpServers` root key)
- Merge with TOML config (TOML wins on conflict)

### 8.3 — Two-level MCP loading
- At session start: count total tools across all enabled servers
- < 20: load all names + descriptions flat
- 20–100: load names + BM25 index, add `search_mcp_tools(query)` tool
- > 100: load server names only, add `get_server_tools(server_name)` + `search_mcp_tools(query)` tools
- `use_mcp_tool(server, tool, args)` loads full schema on demand at call time

### 8.4 — BM25 index
`crates/rustic-agent/src/context/mcp_loader.rs`
- Build in-memory BM25 index from tool names + descriptions at session start
- `search_mcp_tools(query, top_k=5)` tool: returns matching tools with server name, description

### 8.5 — UI updates for MCP config
- mcp-config.js: show config source (TOML vs JSON), trust/allowed_tools display
- Warning if more than 100 tools detected: shows loading strategy

**Done when:** All three MCP loading strategies work. Users can configure via TOML or .mcp.json.

---

## Phase 9 — Skills System
**Scope:** Backend + frontend.

### 9.1 — Skill discovery
`crates/rustic-agent/src/context/skill_loader.rs`
- Scan `.rustic/skills/`, `~/.rustic/skills/`, `.agents/skills/`
- Parse SKILL.md frontmatter (name, description, allowed-tools, disable-model-invocation)
- Build index of `{name, description, path, scope}`

### 9.2 — Skill loading
- Session start: inject `name + description` for all skills into system prompt (capped at 250 chars each)
- On activation: load full SKILL.md body
- BM25 match between user message and skill descriptions for auto-activation threshold

### 9.3 — Skills installer
`install_skill(source: String)` Tauri command:
- Parse as GitHub shorthand (owner/repo) or full URL
- Use `git2` to fetch
- Walk directory for SKILL.md files
- Copy to `.rustic/skills/<name>/` (project) or `~/.rustic/skills/<name>/` (global)
- Maintain `skills-lock.json` with GitHub tree SHA

### 9.4 — Skills UI panel
`src/components/agent/skills-panel.js`
- List installed skills (name, description, scope, source)
- Install from URL button
- Create new skill form (name + description + body textarea → writes SKILL.md)
- Delete skill button

**Done when:** Skills can be installed from GitHub or created manually. They auto-load at session start with deferred full body.

---

## Phase 10 — Workflows System
**Scope:** Backend + frontend. Simple.

### 10.1 — Workflow discovery
`crates/rustic-agent/src/context/workflow_loader.rs`
- Scan `<project>/.rustic/workflows/*.md`
- Parse frontmatter (name, description)
- Workflows are NOT injected into session context — zero context cost

### 10.2 — Workflow trigger
- `/workflow-name` in chat: load workflow body, prepend to user message
- System prompt injection: "You are orchestrating the workflow below. Analyze steps for parallelism, spawn sub-agents where appropriate."
- Workflow body becomes the task description

### 10.3 — Workflows UI panel
`src/components/agent/workflows-panel.js`
- List workflows with name + description
- Create new workflow form
- Click to trigger in current task
- Delete workflow button

**Done when:** Workflows can be created, listed, and triggered. Main model acts as orchestrator.

---

## Phase 11 — Sub-Agent System
**Scope:** Backend + frontend. Largest phase.

### 11.1 — SubagentRegistry
`crates/rustic-agent/src/task/subagent.rs`
```rust
pub struct SubagentRegistry {
    agents: Mutex<HashMap<String, SubagentEntry>>,
    completion_tx: Mutex<HashMap<String, broadcast::Sender<SubagentResult>>>,
}
impl SubagentRegistry {
    pub fn register(parent_task_id, agent_id, model)
    pub fn complete(parent_task_id, agent_id, result)
    pub fn fail(parent_task_id, agent_id, error)
    pub async fn wait_for_any(parent_task_id) -> SubagentResult
    pub async fn wait_for_all(parent_task_id, ids) -> Vec<SubagentResult>
    pub fn active_for_task(parent_task_id) -> Vec<SubagentEntry>
}
```

### 11.2 — spawn_subagent tool
`crates/rustic-agent/src/tools/subagent_tools.rs`
- Depth check: if `context.agent_depth >= 1` → error
- Resolve provider from model_id
- Create child `ToolContext` with `agent_depth = 1`, same lock registry
- `tokio::spawn` new `TaskExecutor::run_turn` for sub-agent
- Register in SubagentRegistry
- Return immediately with agent_id

### 11.3 — Reactive injection in executor
`crates/rustic-agent/src/task/executor.rs`
After turn ends with no tool calls:
- Check `subagent_registry.active_for_task(task_id)`
- If any: `wait_for_any(task_id).await` (suspends on broadcast channel)
- On wake: if result > 2K tokens → summarize via cheap model
- Inject completion message → loop back

### 11.4 — wait_for_all_agents tool
- Loop calling `wait_for_any` until all listed IDs have completed status
- Return all results

### 11.5 — Sub-agent models config
`config.rs`: add `subagent_models: Vec<SubagentModel>`
- Populated from configured providers
- `spawn_subagent` tool definition dynamically includes enabled list

### 11.6 — Sub-agent model settings UI
Settings → AI → Sub-Agent Models: checkbox list

### 11.7 — Sub-agent panel in chat-view.js
- Collapsible section: "Sub-agents (N running)"
- Each row: agent_id, model, status indicator
- Expandable: shows streaming sub-agent output
- Listen for SubagentSpawned / SubagentCompleted / SubagentFailed / SubagentTextDelta events

**Done when:** Main model can spawn parallel sub-agents with different models. Reactive injection wakes main model as each finishes. File locks prevent write conflicts.

---

## Phase 12 — Agent Panel UI Redesign
**Scope:** Frontend only. No backend changes.

### 12.1 — Three-tab layout
Replace current single-list panel with tabs: Agent (active) | History | Terminals.

### 12.2 — Project sections with task rows
- Expandable project rows showing task count badge
- Task rows: status dot, title, cost pill, status label
- Per-project `+` new task button

### 12.3 — History tab
- Show Completed/Failed tasks per project
- Search box (filters by title)
- Click to reopen chat view for that task

### 12.4 — Terminals tab
- List of all terminals spawned by agents (`term_id`, task context, status)
- Click → opens terminal panel to that terminal
- Requires agent to emit `TerminalSpawned { task_id, term_id, command }` event when `run_command` creates a persistent terminal (vs a one-shot command)

### 12.5 — Stop/abort button
- Square stop icon in task row (visible while Running)
- Calls new `abort_task(task_id)` backend command
- Backend: cancellation `AtomicBool` flag in executor, checked before each tool call

**Done when:** All three tabs work, running tasks show live cost, stop button cancels execution.

---

## Phase 13 — Chat View Redesign
**Scope:** Frontend only. Incremental — build on existing chat-view.js.

### 13.1 — Message type renderers
Replace the generic `tool_use` / `tool_result` blocks with type-specific renderers:
- `run_command` → `$ command` pill + collapsible output
- `edit_file` / `apply_patch` / `create_file` → file change badge + checkpoint diff button (already done for checkpoint marker)
- Read operations → compact single-line `Read: path [X–Y]`
- Generic tool → current expandable block (fallback)

### 13.2 — Chat header bar
Add above the message area:
- Task title (editable)
- Model badge + switch button
- Permission mode badge
- Token/cost live display
- Stop button (while Running)

### 13.3 — Sub-agent inline rows
- On `SubagentSpawned` event: insert collapsible row in message stream
- Expanding shows sub-agent's own message thread (recursive renderer)
- Status updates on `SubagentCompleted` / `SubagentFailed`

### 13.4 — File attachment + image paste
- Paperclip button → file picker → adds file paths as pills above textarea
- Paste from clipboard → detect image → convert to base64 content block
- Sent as additional content blocks with user message

### 13.5 — Input toolbar
Below textarea: model selector | mode selector | slash command trigger | attach button | send button

**Done when:** All message types render distinctly, sub-agents show inline, attachments work.

---

## Phase 14 — Slash Commands
**Scope:** Frontend + minor backend (list endpoints already exist).

### 14.1 — Slash picker overlay
On `/` keydown in textarea:
- Show floating overlay above input
- List: skills (from `list_skills`), MCP servers (from `list_mcp_servers`), workflows (from `list_workflows`)
- Labeled with `[Skill]`, `[MCP]`, `[Workflow]` color pills
- Filter as user types after `/`

### 14.2 — BM25 filter in picker
Client-side BM25 scoring on skill/workflow descriptions as user types.

### 14.3 — Insert on select
Enter or click → inserts `@skill-name` or `@mcp-server` or `/workflow-name` token into textarea.

**Done when:** Typing `/` shows context-aware picker, selecting inserts the correct token.

---

## Phase 15 — Sensitive File Protection
**Scope:** Backend (tool execution layer) + frontend (confirmation UI).

### 15.1 — Tier classification
In `file_ops.rs` and `terminal.rs`:
- Tier-1 check: match absolute path against hardcoded patterns (`id_rsa`, `*.pem`, etc.) → return `SENSITIVE_FILE_BLOCKED` immediately, no override
- Tier-2 check: match against `.env*`, `credentials.*`, `secrets.*` patterns → emit `SensitiveFileRequest` event, await oneshot (same broker pattern as ManualEdit)
- Tier-3 check: load `.gitignore` at task start (using `ignore` crate already in deps), check path → same as tier-2 with different message

### 15.2 — SensitiveFileRequest event + Tauri command
- New `TaskEvent::SensitiveFileRequest { task_id, request_id, path, tier, reason }`
- New command `respond_to_sensitive_file(task_id, request_id, approved)`
- UI: shows warning popup with file path, tier label, "Allow once" / "Always allow this session" / "Block" buttons

### 15.3 — FullAuto warning modal
One-time modal on first FullAuto switch per project. Store acknowledgement in project settings.

### 15.4 — Per-project allowlist
Load `<project>/.rustic/allowed-files.txt` at task start. Paths in this file skip tier-2/3 confirmation.

**Done when:** Model can't read `.env` or private keys without explicit approval. FullAuto shows warning on first use.

---

## Implementation Order Summary

| Phase | What | Touches |
|---|---|---|
| **0** | **Task completion tool** ✓ | tools, executor, chat-view |
| 1 | Permission modes refactor | permissions.rs, tools, UI |
| 2 | ManualEdit approval flow | broker, executor, chat-view |
| 3 | Token/cost tracking | cost.rs, executor, chat-view |
| 4 | Shell detection + output caps + turn budget | executor, terminal.rs |
| 5 | File tools redesign | file_lock.rs, file_ops.rs |
| 6 | Memory (memory.md) | executor, agent-panel, chat-view |
| 7 | Model switching | executor, chat-view |
| 8 | MCP config upgrade | config.rs, mcp_loader.rs, mcp-config.js |
| 9 | Skills system | skill_loader.rs, skills-panel.js |
| 10 | Workflows system | workflow_loader.rs, workflows-panel.js |
| 11 | Sub-agent system | subagent.rs, subagent_tools.rs, executor, chat-view |
| 12 | Agent panel UI redesign | agent-panel.js |
| 13 | Chat view redesign | chat-view.js |
| 14 | Slash commands | chat-view.js, skills/workflows/mcp list APIs |
| 15 | Sensitive file protection | file_ops.rs, terminal.rs, new broker, UI |

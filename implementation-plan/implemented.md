# Rustic Agent — What Has Been Implemented

This document tracks everything that has been built. Use it at the start of a new session to know where to begin.

---

## Phase 0 — Task Completion Tool (DONE)

- `task_complete` tool definition added to `BuiltinTools::definitions()` in `crates/rustic-agent/src/tools/mod.rs`
- `TaskEvent::TaskComplete { task_id, summary, notes, diff: TaskDiff }` variant added to executor
- Executor loop intercepts `task_complete` call: extracts summary/notes, calls `compute_diff_fn`, emits `TaskComplete`, breaks immediately
- `AgentTaskCompleteEvent` struct in `src-tauri/src/commands/agent.rs`, forwarded as `agent-task-complete` Tauri event
- `onAgentTaskComplete(callback)` added to `src/lib/tauri-api.js`
- `appendTaskComplete(taskId, summary, notes, diff)` added to `src/state/agent.js`, registered in `initAgentEvents()`
- `renderCompletionCard(summary, notes, diff)` renders styled completion card in `src/components/agent/chat-view.js`
- Task complete card CSS added to `src/styles/agent.css`

---

## Checkpoint System — Per-Turn Diff (DONE)

- `DiffStatus`, `FileDiff`, `TaskDiff` structs added in `crates/rustic-agent/src/checkpoint/mod.rs`
- `compute_checkpoint_diff(db, task_id, checkpoint_id)` — per-turn diff using before/after snapshot inference
- `compute_task_diff(db, task_id)` — full task diff across all checkpoints
- Both in `crates/rustic-agent/src/checkpoint/snapshot.rs`
- `get_file_snapshots_after_message` and `get_all_file_snapshots_for_task` added to `crates/rustic-db/src/checkpoint_repo.rs`
- `get_checkpoint_diff(task_id, checkpoint_id)` Tauri command registered in `src-tauri/src/lib.rs`
- `getCheckpointDiff(taskId, checkpointId)` added to `src/lib/tauri-api.js`
- `renderCheckpointMarker(cp, taskId)` in chat-view.js renders "View diff" (lazy inline diff card) + "Revert" buttons
- Checkpoint action styles added to `src/styles/agent.css`

---

## Task Persistence (DONE)

All DB methods exist in `crates/rustic-db/src/task_repo.rs`:
- `insert_task`, `get_task`, `list_tasks_for_project`, `update_task_status`, `update_task_title`
- `upsert_message`, `insert_message`, `get_messages_for_task`, `get_next_sort_order`
- `delete_messages_for_task`, `delete_task`

Wired into Tauri commands in `src-tauri/src/commands/agent.rs`:
- `create_task` — persists `TaskRow` to SQLite on creation
- `send_message` — auto-title from first user message (first 70 chars, filesystem-safe) via `db.update_task_title()`; saves all messages to DB (delete + re-insert) after each executor turn completes
- `list_tasks` — loads from DB via `list_tasks_for_project()`, hydrates in-memory `AgentTask` entries for tasks not already loaded
- `get_task_messages` — returns in-memory messages if present; falls back to DB and hydrates memory
- `delete_task` — removes from both in-memory state and DB (messages + task row)

---

## Cancellation / Stop Button (DONE)

- `cancellation_tokens: HashMap<String, Arc<AtomicBool>>` added to `AgentState` in `src-tauri/src/state.rs`
- Fresh `Arc<AtomicBool>` created per `send_message` call, stored in `cancellation_tokens`
- Passed into executor via `ToolContext.cancel_token: Option<Arc<AtomicBool>>`
- Executor checks token at top of every loop iteration and before each tool execution; emits `TaskEvent::StatusChange { status: TaskStatus::Cancelled }` and breaks
- `abort_task(task_id)` Tauri command: sets the `AtomicBool` to true
- `abortTask(taskId)` added to `src/lib/tauri-api.js`
- Stop button rendered on the last user message while task is `Running` in chat-view.js; calls `api.abortTask()`
- Registered in `src-tauri/src/lib.rs`

---

## FullAuto Sensitive Files Modal (DONE)

- `sensitive_files_allowed: bool` field added to `AgentTask` in `src-tauri/src/state.rs`
- `set_task_sensitive_access(task_id, allowed)` Tauri command sets the flag
- `setTaskSensitiveAccess(taskId, allowed)` added to `src/lib/tauri-api.js`
- `setTaskPermissions(taskId, level)` exported from `src/state/agent.js`:
  - For `FullAuto`: shows confirmation modal with two options before applying
  - For other modes: resets `sensitive_files_allowed` to false
- `showFullAutoModal()` Promise-based modal with two choices:
  - "Ask before reading sensitive files" (default, `sensitive_files_allowed = false`)
  - "Allow all — including sensitive files" (`sensitive_files_allowed = true`)
- Modal CSS added to `src/styles/agent.css`
- `sensitive_files_allowed` passed through to `ToolContext` (infrastructure ready for Phase 15 to use)

---

## UX Message Actions (DONE)

All in `src/components/agent/chat-view.js`:
- Copy button (`.chat-message__action-btn`) on every message
- Stop button (`.chat-message__stop-btn`) on the last user message while `isStreaming`/`Running`
- Retry button (`.chat-message__retry-btn`) on the last user message when status is `Failed`
- `extractMessageText(msg)` helper for copy
- Message action styles in `src/styles/agent.css`

---

---

## Phase 1 — Permission Modes Refactor (DONE)

- `PermissionLevel` enum replaced: `Admin | ReadWrite | ReadOnly` → `Chat | ManualEdit | AutoEdit | FullAuto` in `crates/rustic-agent/src/task/permissions.rs`
- `ToolContext::check_permission()` updated in `crates/rustic-agent/src/tools/mod.rs`: Chat=read-only, ManualEdit=read-only (broker Phase 2), AutoEdit=no-execute, FullAuto=all
- `file_ops.rs` write denials emit `PERMISSION_DENIED` (Chat) or `PERMISSION_PENDING` (ManualEdit) structured error codes
- `terminal.rs` execute denials emit `PERMISSION_PENDING` for Chat/ManualEdit/AutoEdit, only FullAuto runs commands
- `set_task_permissions(task_id, level)` Tauri command added in `src-tauri/src/commands/agent.rs` — sets per-task permission level
- `set_permissions` updated to parse new level strings; `parse_permission_level()` helper added
- `set_task_permissions` registered in `src-tauri/src/lib.rs`
- `setTaskPermissions(taskId, level)` added to `src/lib/tauri-api.js`
- `agent.js` `setTaskPermissions` updated to call per-task `api.setTaskPermissions` instead of project-level `api.setPermissions`
- Permission mode pill added to chat input toolbar in `chat-view.js`: colored dot + label, dropdown with 4 modes, updates task on selection
- CSS for `.chat-input-toolbar`, `.chat-mode-pill`, `.chat-mode-dropdown` added to `src/styles/agent.css`

---

---

## Phase 2 — ManualEdit Approval Flow (DONE)

- `TaskEvent` and `PermissionOp` moved to `crates/rustic-agent/src/task/mod.rs`; `EventTx` type alias added
- `TaskEvent::PermissionRequest { task_id, request_id, operation, description, preview }` variant added
- `PermissionOp { WriteFile, CreateFile, RunCommand }` enum with `describe()` method
- `crates/rustic-agent/src/task/permission_broker.rs` — `PermissionBroker` with `request()` (async, 60s timeout → auto-deny) and `respond()` methods using `oneshot` channels
- `ToolContext` in `tools/mod.rs` extended with `permission_broker: Arc<PermissionBroker>`, `event_tx: EventTx`, `task_id: String`
- `needs_write_approval()` and `needs_exec_approval()` helpers added to `ToolContext`
- `file_ops.rs`: ManualEdit writes call `broker.request().await`; Chat mode hard-denies; AutoEdit/FullAuto auto-allow
- `terminal.rs`: ManualEdit + AutoEdit commands call `broker.request().await`; Chat hard-denies; FullAuto auto-allows
- `executor.rs` `run_turn` signature simplified — removed `task_id` and `event_tx` params, uses `context.task_id` / `context.event_tx` directly
- `AgentState` in `state.rs` gets `permission_broker: Arc<PermissionBroker>`
- `send_message` in `commands/agent.rs`: creates `event_tx` before `ToolContext`, passes broker + event_tx + task_id into context; handles `PermissionRequest` event → emits `agent-permission-request` Tauri event
- `respond_to_permission(task_id, request_id, approved)` Tauri command registered
- `respondToPermission` + `onAgentPermissionRequest` added to `src/lib/tauri-api.js`
- `agentStore` gains `permissionRequests` map (taskId → pending requests array)
- `addPermissionRequest`, `removePermissionRequest`, `respondToPermission` added to `src/state/agent.js`
- Approval widget rendered in `chat-view.js`: shows between messages and input, one row per pending request with operation icon, description, preview, countdown timer (auto-deny at 0), Deny and Allow buttons
- CSS for `.chat-approval-area` / `.chat-approval-widget` and sub-elements added to `src/styles/agent.css`

---

---

## Phase 3 — Token / Cost Tracking (DONE)

- `cache_read_tokens: u32` added to `TokenUsage` in `crates/rustic-agent/src/provider/mod.rs`
- `ClaudeUsage` in `provider/claude.rs` now captures `cache_read_input_tokens` (via `#[serde(default)]`)
- `openai.rs` updated to include `cache_read_tokens: 0` in both `TokenUsage` construction sites
- `crates/rustic-agent/src/task/cost.rs` (new): `TaskCost` struct with `total_input_tokens`, `total_output_tokens`, `total_cache_read_tokens`, `estimated_cost_usd`, `turn_count`; `TaskCost::add_turn()` accumulates per-turn; `calculate_cost()` with a pricing table covering Claude, OpenAI, and Gemini models
- `task/mod.rs`: `pub mod cost` added; `TaskEvent::CostUpdate { task_id, cost: TaskCost }` variant added; `use crate::task::cost::TaskCost` imported
- `task/executor.rs`: `task_cost: TaskCost` local variable in `run_turn`; after each `provider.chat()` call: `task_cost.add_turn(model, &response.usage)` + emits `TaskEvent::CostUpdate`
- `lib.rs`: `pub use task::cost::TaskCost` exported
- `state.rs`: `TaskCostMap` type alias added; `AppState.task_costs: TaskCostMap` (Arc<Mutex<HashMap>>) added; `AgentTask.cost: TaskCost` field added for future use
- `commands/agent.rs`: `AgentCostUpdateEvent { task_id, cost }` struct added; `task_costs_arc` cloned before spawn; `TaskEvent::CostUpdate` handler updates the cost map AND emits `agent-cost-update` Tauri event; `get_task_cost(task_id)` Tauri command returns current `TaskCost` for a task
- `src-tauri/src/lib.rs`: `get_task_cost` registered in `invoke_handler`
- `src/lib/tauri-api.js`: `getTaskCost(taskId)` and `onAgentCostUpdate(callback)` added
- `src/state/agent.js`: `updateTaskCost(taskId, cost)` added; registered in `initAgentEvents` via `api.onAgentCostUpdate`; store comment updated to note `cost` field on task
- `src/components/agent/chat-view.js`: `chat-header-bar` + `chat-header-bar__cost` added above messages area; `updateCostDisplay()` formats `~1.2k tokens · $0.003` with full tooltip breakdown; subscribed to tasks/activeTaskId state changes
- `src/styles/agent.css`: `.chat-header-bar` and `.chat-header-bar__cost` styles added

---

---

## Phase 4 — Shell Detection + Output Caps + Turn Budget (DONE)

- `system_prompt: Option<String>` added to `ProviderConfig` in `provider/mod.rs`
- Claude provider: `config.system_prompt` takes priority over `Role::System` message; passed as `system` field
- OpenAI provider: `config.system_prompt` prepended as system message if not already present
- System prompt built in `send_message` (`commands/agent.rs`): includes shell environment line (`PowerShell on Windows` / `bash on macOS` / `bash on Linux`), `task_complete` instructions, and all structured error code explanations (PERMISSION_DENIED, OUTPUT_TRUNCATED, STALE_READ, CONTENT_DELETED, LOCK_TIMEOUT, ALREADY_APPLIED)
- `TurnBudget { used: u32, max: u32 }` struct added to `task/mod.rs` with `TurnBudget::new(max)` → `Arc<Mutex<TurnBudget>>`; `TaskStatus::TurnLimitReached` variant added; `TaskEvent::TurnBudgetWarning { task_id, turns_remaining }` variant added
- `ToolContext` gains `turn_budget: Arc<Mutex<TurnBudget>>` field in `tools/mod.rs`
- Executor (`task/executor.rs`): at top of each loop: increments `used`, stops with `TurnLimitReached` at `used >= max`; injects budget warning text block into tool results message at `remaining == 5`; emits `TurnBudgetWarning` event
- Terminal output hard-capped at 16KB in `terminal.rs`: `truncate_utf8()` helper; appends `OUTPUT_TRUNCATED: Truncated at 16KB — N more lines.` message when exceeded
- `TurnBudget` exported from `lib.rs`
- `state.rs`: `TurnBudgetMap` type alias; `AppState.turn_budgets: TurnBudgetMap` added; `AgentState.default_turn_budget: u32` (default 50)
- `commands/agent.rs`: `turn_budget` Arc created per `send_message` call, stored in `turn_budgets` map, passed to `ToolContext`; `AgentTurnBudgetWarningEvent` struct added; `TurnBudgetWarning` event forwarded as `agent-turn-budget-warning`; `extend_turn_budget(task_id, additional)` command updates live budget or fallback default
- `src-tauri/src/lib.rs`: `extend_turn_budget` registered
- `tauri-api.js`: `extendTurnBudget(taskId, additional)` and `onAgentTurnBudgetWarning(callback)` added
- `agent.js`: `turnBudgetWarnings` map in store; `onAgentTurnBudgetWarning` handler updates it; status change to non-running clears warning for that task
- `chat-view.js`: `budgetBanner` element between approvalArea and inputArea; `renderBudgetBanner()` shows amber warn banner at 5 remaining, red limit-reached banner with "Continue (+20 turns)" button (calls `extendTurnBudget` + `sendMessage`); subscribed to `turnBudgetWarnings` state
- `agent.css`: `.chat-budget-banner` styles with `--warn` and `--limit` variants

---

---

## Phase 5 — File Tools Redesign (DONE)

- `crates/rustic-agent/src/task/file_lock.rs` (new): `FileLockRegistry` with per-path `Arc<tokio::sync::Mutex<()>>` registry; `get_lock(path)` returns the Arc for callers to lock with `tokio::time::timeout(30s, lock.lock()).await`; `LOCK_TIMEOUT_SECS = 30` constant
- `task/mod.rs`: `pub mod file_lock` added
- `tools/mod.rs`: `file_lock: Arc<FileLockRegistry>` field added to `ToolContext`; `BuiltinTools::execute` updated to route `edit_file | apply_patch | insert_lines | delete_lines` to `file_ops::execute`
- `tools/file_ops.rs` rewritten:
  - Removed: `write_file` (full file overwrite removed — use `edit_file`/`apply_patch`)
  - `read_file(path, start_line?, end_line?)`: supports optional line range (1-indexed), renders with line numbers when range specified
  - `create_file(path, content)`: unchanged, returns `FILE_HAS_CONTENT` if already exists
  - `edit_file(path, old_string, new_string, hint_line?)`: atomic locked RMW; `STALE_READ` with ±150 lines context when old_string not found; `ALREADY_APPLIED` when new_string already present; `CONTENT_DELETED` when file missing
  - `apply_patch(path, hunks[{old_string, new_string}])`: multi-hunk atomic — applies all to in-memory copy, writes only if all succeed; `STALE_READ` on any hunk failure (rollback = don't write)
  - `insert_lines(path, after_line, content)`: locked insert at line N (0 = before first line); preserves trailing newline
  - `delete_lines(path, start_line, end_line)`: locked delete of line range (1-indexed inclusive); preserves trailing newline
  - `list_directory`: unchanged
  - `build_stale_read_context(content, hint_line)`: ±150 lines around hint, capped at 300 lines / 8KB, annotated with line numbers and position header
- `lib.rs`: `pub use task::file_lock::FileLockRegistry` exported
- `state.rs`: `pub file_lock: Arc<FileLockRegistry>` added to `AppState`; initialized with `FileLockRegistry::new()` in `AppState::new()`; `FileLockRegistry` added to imports
- `commands/agent.rs`: `file_lock = Arc::clone(&state.file_lock)` cloned before spawn; passed as `file_lock: Arc::clone(&file_lock)` in `ToolContext`; system prompt updated with file navigation workflow (grep -n, awk NR ranges, PowerShell equivalents, 300-line limit, hint_line guidance, STALE_READ/CONTENT_DELETED/FILE_HAS_CONTENT error handling)

---

---

## Phase 6 — Memory (memory.md) (DONE)

- `TaskEvent::MemoryUpdated { task_id }` variant added to `crates/rustic-agent/src/task/mod.rs`
- `maybe_emit_memory_updated(path, ctx)` helper in `tools/file_ops.rs` — called after every successful write in `create_file`, `edit_file`, `apply_patch`, `insert_lines`, `delete_lines`; emits `MemoryUpdated` when path ends with `.rustic/memory.md`
- `send_message` restructured in `src-tauri/src/commands/agent.rs`: project root now resolved before message build so memory can be loaded; on first message for a task, reads `<project_root>/.rustic/memory.md` and prepends two messages (`[Project Memory]\n<content>` user + assistant ack) to `task.messages` before the user message
- System prompt extended with `## Project memory` section explaining the file location, update rules, and 500-line limit
- `AgentMemoryUpdatedEvent` struct added; `TaskEvent::MemoryUpdated` forwarded as `agent-memory-updated` Tauri event
- `get_memory(project_id)` Tauri command: creates `.rustic/memory.md` (and parent dir) if missing, returns content
- `clear_memory(project_id)` Tauri command: writes empty string to memory.md if it exists
- Both commands registered in `src-tauri/src/lib.rs`
- `onAgentMemoryUpdated`, `getMemory`, `clearMemory` added to `src/lib/tauri-api.js`
- `showMemoryToast()` in `src/state/agent.js`: shows a brief "Memory updated" toast; subscribed via `api.onAgentMemoryUpdated` in `initAgentEvents()`
- Memory indicator button added per project in `src/components/agent/agent-panel.js`: clipboard/notepad icon, hidden until project row hover, click calls `getMemory` (ensures file exists) then `openFile` to open memory.md in the editor
- CSS for `.agent-project__memory` (hover-reveal icon) and `.memory-toast` / `.memory-toast--visible` (slide-up toast) added to `src/styles/agent.css`

---

---

## Phase 7 — Model Switching Mid-Chat (DONE)

- `ContentBlock::ModelSwitch { from_model, to_model }` added to `provider/mod.rs` — UI-only marker, serializes to DB, never sent to API
- `claude.rs` `convert_content_blocks()` handles `ModelSwitch → null` + filters null entries before serialization
- `executor.rs`: before every `provider.chat()` call, strips `ModelSwitch` blocks from messages and drops messages that become empty
- `update_task_model(id, provider_type, model)` added to `rustic-db/src/task_repo.rs`
- `switch_model(task_id, provider_type, model)` Tauri command: updates in-memory model/provider, pushes `ModelSwitch` message, persists to DB, emits `agent-model-switched` event; registered in `lib.rs`
- `switchModel` and `onAgentModelSwitched` added to `tauri-api.js`
- `onAgentModelSwitched` handler in `agent.js`: updates store, appends ModelSwitch message for chat re-render
- `abbreviateModel()` helper in `chat-view.js` (e.g. `claude-sonnet-4-20250514` → `Sonnet 4`)
- Model selector button in input toolbar (left of mode pill): dropdown grouped by provider, calls `api.switchModel`
- `renderModelSwitchSeparator()` renders `──── Model: Sonnet 4 ────` separator in message stream
- CSS for model button, dropdown, and separator added to `agent.css`

---

---

## Phase 8 — MCP Config Upgrade (DONE)

- `McpSource { Manual, Json }` enum added to `crates/rustic-agent/src/mcp/config.rs`; `source` field added to `McpServerConfig` with `#[serde(default)]`
- `McpManager::load_from_json_file(path)` in `mcp/mod.rs` — parses Claude Code `.mcp.json` format (`mcpServers` key), supports stdio and SSE transports; `remove_json_servers()` clears Json-sourced servers before reload
- `mcp_manager: Arc<Mutex<McpManager>>` in `src-tauri/src/state.rs` (was bare `McpManager`)
- All 4 MCP Tauri commands updated to clone the Arc before calling `.lock()`
- `send_message` in `commands/agent.rs`: clones Arc, loads `.mcp.json` if present at task start, connects all enabled MCP servers in an async `spawn_blocking` call, gathers tool defs, appends MCP system prompt section (full names+desc < 20 tools, names only 20–100, count > 100); passes `mcp_manager` Arc and `mcp_tool_defs` to `ToolContext`
- `ToolContext` in `tools/mod.rs` gains `mcp_manager: Option<Arc<Mutex<McpManager>>>` and `mcp_tool_defs: Vec<ToolDef>`
- `BuiltinTools::is_builtin(name)` static method added for routing dispatch
- `executor.rs`: combines builtin + MCP tool defs before each provider call; routes non-builtin tool calls through `spawn_blocking` + `McpManager::call_tool`; returns `ToolOutput` from `Value` result
- `import_mcp_json(project_id)` Tauri command added; registered in `lib.rs`
- `importMcpJson(projectId)` added to `tauri-api.js`
- `mcp-config.js` updated: source badge (`.mcp.json` pill on Json-sourced servers); "Import .mcp.json" button when `projectId` is provided
- CSS for badge and import button added to `agent.css`

---

## Phase 9 — Skills System (DONE)

- `crates/rustic-agent/src/skills/mod.rs` (new): `SkillDef { name, description, scope, path, allowed_tools }`, `SkillScope { Project, Global }`; `discover_skills(project_root)` scans `.rustic/skills/`, `.agents/skills/`, `~/.rustic/skills/`; `parse_skill_frontmatter(content)` extracts `name`, `description`, `allowed-tools` from YAML frontmatter; `skill_body(content)` returns content after closing `---`; `build_skills_system_section(skills)` builds system prompt section
- `crates/rustic-agent/src/tools/skill_tools.rs` (new): `read_skill(name)` builtin tool — discovers skills, returns full SKILL.md body; included in `BuiltinTools::definitions()` and `is_builtin()` list
- `send_message` in `commands/agent.rs`: calls `discover_skills(&project_root)` + `build_skills_system_section` and appends skills to system prompt at session start
- `src-tauri/src/commands/skills.rs` (new): `list_skills`, `get_skill_body`, `create_skill`, `delete_skill`, `install_skill` (downloads GitHub ZIP archive, finds SKILL.md files, copies skill dirs to `.rustic/skills/<name>/`, writes `skills-lock.json`)
- `zip` crate added to `src-tauri/Cargo.toml` for ZIP extraction
- All 5 skills commands registered in `src-tauri/src/commands/mod.rs` and `src-tauri/src/lib.rs`
- `listSkills`, `getSkillBody`, `createSkill`, `deleteSkill`, `installSkill` added to `src/lib/tauri-api.js`
- `src/components/agent/skills-panel.js` (new): skills list with name/description/scope badge, install-from-GitHub bar, create form, delete + view-body actions
- `src/components/settings/agent-settings.js`: Skills placeholder replaced with `createSkillsPanel(activeProjectId)`
- CSS for skills panel, install bar, list items, badges, create form, body modal added to `agent.css`

---

## Phase 10 — Workflows System (DONE)

- `crates/rustic-agent/src/workflows/mod.rs` (new): `WorkflowDef { name, description, path }`; `discover_workflows(project_root)` scans `<project>/.rustic/workflows/*.md`; `parse_workflow_frontmatter(content)` extracts `name` and `description` from YAML frontmatter; `workflow_body(content)` returns content after closing `---`
- `pub mod workflows` added to `crates/rustic-agent/src/lib.rs`; re-exports `WorkflowDef`, `discover_workflows`, `workflow_body`
- `src-tauri/src/commands/workflows.rs` (new): `list_workflows`, `get_workflow_body`, `create_workflow` (creates `<project>/.rustic/workflows/<name>.md`), `delete_workflow`
- All 4 workflow commands added to `src-tauri/src/commands/mod.rs` and `src-tauri/src/lib.rs`
- `listWorkflows`, `getWorkflowBody`, `createWorkflow`, `deleteWorkflow` added to `src/lib/tauri-api.js`
- `src/components/agent/workflows-panel.js` (new): workflows list with name/description, create form (name + description + body textarea), per-workflow Trigger (play icon) + View (eye icon) + Delete actions; Trigger dispatches `workflow-trigger` custom event with `{ name, body }`
- `src/components/settings/agent-settings.js`: Workflows placeholder replaced with `createWorkflowsPanel(activeProjectId)`
- `src/components/agent/chat-view.js`: listener for `workflow-trigger` event inserts workflow body into chat textarea (prepends if text already present, sets directly if empty)
- CSS for `.workflows-panel`, `.workflows-item`, `.workflows-create-form`, `.workflows-body-modal` added to `agent.css`

---

## Phase 11 — Sub-Agent System

- `crates/rustic-agent/src/task/subagent.rs` (new): `SubagentRegistry` (Arc-wrapped, `Mutex<HashMap>` per parent task), `SubagentEntry`, `SubagentStatus`, `SubagentResult`, `SubagentCompletionEvent`; `register`, `complete`, `fail`, `active_for_task`, `all_for_task`, `wait_for_any` (async, polls Tokio `Notify`)
- `crates/rustic-agent/src/task/mod.rs`: added `pub mod subagent`; four new `TaskEvent` variants: `SubagentSpawned`, `SubagentCompleted`, `SubagentFailed`, `SubagentTextDelta`
- `crates/rustic-agent/src/tools/subagent_tools.rs` (new): `spawn_subagent`, `wait_for_all_agents`, `list_active_agents`, `cancel_agent` tools; depth guard (depth >= 1 → PERMISSION_DENIED); spawned sub-agents run in a `tokio::spawn` with their own `ToolContext` (agent_depth=1, 30-turn budget); text events forwarded to parent as `SubagentTextDelta`; completion/failure reported back to registry
- `crates/rustic-agent/src/tools/mod.rs`: added `pub mod subagent_tools`; three new `ToolContext` fields: `subagent_registry: Arc<SubagentRegistry>`, `agent_depth: u8`, `ai_config: Arc<AiConfig>`; four new `is_builtin` entries; `execute` routes subagent tool names to `subagent_tools::execute`; `definitions` includes `subagent_tools::definitions()`
- `crates/rustic-agent/src/task/executor.rs`: reactive sub-agent injection — when `tool_uses.is_empty()` and active sub-agents exist, `wait_for_any` awaits one completion/failure, injects a `[Sub-agent '…' completed/FAILED]` user message, and continues the loop
- `crates/rustic-agent/src/lib.rs`: re-exports `SubagentRegistry`, `SubagentResult`, `SubagentCompletionEvent`
- `src-tauri/src/state.rs`: imports `SubagentRegistry`; `AppState` gains `subagent_registry: Arc<SubagentRegistry>`; initialized in `AppState::new()`
- `src-tauri/src/commands/agent.rs`: four new event structs (`AgentSubagentSpawnedEvent`, etc.); `send_message` extracts `ai_config: Arc<AiConfig>` and clones `subagent_registry` before spawning; `ToolContext` constructed with `subagent_registry`, `agent_depth: 0`, `ai_config`; event loop handles all four new `TaskEvent` variants and emits `agent-subagent-*` Tauri events
- `src/lib/tauri-api.js`: `onAgentSubagentSpawned`, `onAgentSubagentCompleted`, `onAgentSubagentFailed`, `onAgentSubagentTextDelta` listener wrappers
- `src/state/agent.js`: `subagents: {}` added to store initial state; `initSubagentEvents()` subscribes to all four sub-agent events and maintains `subagents[taskId][agentId]` with `{ agentId, model, status, output }`; called from `initAgentEvents()`
- `src/components/agent/chat-view.js`: `subagentsPanel` element added between `approvalArea` and `budgetBanner`; `renderSubagentsPanel()` renders header with running count, per-agent rows with status dot/spinner, expandable output; subscribed to `agentStore('subagents')`
- `src/styles/agent.css`: `.chat-subagents-panel`, `.chat-subagents-header`, `.chat-subagent-row`, `.chat-subagent-row__status`, `.chat-subagent-spinner`, `.chat-subagent-row__id`, `.chat-subagent-row__model`, `.chat-subagent-output` styles

---

## Phase 12 — Agent Panel UI Redesign (DONE)

Frontend-only rewrite of `src/components/agent/agent-panel.js`.

- Three-tab layout: **Agent**, **History**, **Terminals** (tab bar on the left of the header; settings icon buttons on the right)
- `TERMINAL_STATUSES` set: `Completed | Failed | Cancelled | TurnLimitReached | Stopped`
- `formatCost(cost)` helper: renders `$0.003` (USD) or `~1.2k` (tokens) or `~N` (small token count)
- **Agent tab**: shows tasks where status is NOT in terminal statuses, grouped by project
  - Project row: toggle arrow (▶/▼), project name, running-count badge (accent pill), memory button, + new task button
  - Expand/collapse per project (local `Set<projectId>`, all expanded by default)
  - Tasks sorted: Running first; each row shows status icon, title, cost pill, status label, stop button (hover, Running only, calls `api.abortTask`), delete button (hover)
- **History tab**: tasks in terminal statuses, with search box (case-insensitive substring filter on title)
  - Grouped by project with read-only project headers
  - Task rows show status icon, title, cost pill, date (relative: "Today", "Yesterday", "Nd ago", or locale date)
  - Empty state: "No matching tasks" or "No history yet"
- **Terminals tab**: calls `api.listTerminals()` on each activation
  - Each terminal row: label + cwd (mono); click dispatches `focus-terminal` custom DOM event with `{ sessionId }`
  - Empty state: "No agent terminals"; error state: "Failed to load terminals"
- Store subscriptions: `tasks` and `activeTaskId` → re-render agent/history tabs; `projects` → re-render agent/history tabs
- `historySearchQuery` persists across tab switches (local variable)
- New CSS classes added to `src/styles/agent.css`:
  - `.agent-panel__header`, `.agent-panel__tabs`, `.agent-panel__tab`, `.agent-panel__tab--active`
  - `.agent-tab-content`
  - `.agent-project__toggle`, `.agent-project__count`
  - `.agent-task__cost`, `.agent-task__status-label`, `.agent-task__stop`, `.agent-task__date`
  - `.agent-history`, `.agent-history__search`, `.agent-history__list`, `.agent-history__empty`
  - `.agent-project__header--history`
  - `.agent-terminals`, `.agent-terminals__list`, `.agent-terminals__empty`
  - `.agent-terminal-row`, `.agent-terminal-row__label`, `.agent-terminal-row__cwd`

---

## Phase 13 — Chat View Redesign (DONE)

Frontend-only changes to `src/components/agent/chat-view.js` and `src/styles/agent.css`.

### 13.1 — Tool use type-specific renderers

New top-level functions added to `chat-view.js` (outside `createChatView`, alongside `renderCompletionCard` etc.):

- `renderToolUse(block)` — dispatcher: routes by tool name to specialized renderers; returns `null` for `task_complete` (already handled as completion card)
- `renderCommandTool(input)` — renders `run_command` as a pill with terminal icon + green monospace command text; truncates at 80 chars with tooltip
- `renderFileTool(name, input)` — renders file write/edit/create tools as a colored inline badge (`Edited:`, `Created:`, `Wrote:`, `Patched:`) with file path; blue for modified, green for created
- `renderReadTool(name, input)` — renders `read_file`, `list_directory`, `grep_search` as a compact dimmed row with eye icon and label + path + optional line range
- `renderGenericTool(name, input)` — collapsible panel with tool name + chevron toggle; body shows pretty-printed JSON input
- `renderToolResult(block)` — replaces the old single-pre approach: output > 10 lines gets collapsed to first 6 lines + "Show all (N lines)" expand button; shows `Output truncated` badge when content includes `OUTPUT_TRUNCATED:` or `[Truncated at`
- `renderMessages` loop updated: `tool_use` → `renderToolUse(block)`, `tool_result` → `renderToolResult(block)`

New CSS classes: `.chat-tool-cmd`, `.chat-tool-cmd__pill`, `.chat-tool-cmd__text`, `.chat-tool-file`, `.chat-tool-file--modified`, `.chat-tool-file--created`, `.chat-tool-file__label`, `.chat-tool-file__path`, `.chat-tool-read`, `.chat-tool-generic`, `.chat-tool-generic__header`, `.chat-tool-generic__name`, `.chat-tool-generic__toggle`, `.chat-tool-generic__body`, `.chat-tool-generic__body--collapsed`, `.chat-tool-result__expand`, `.chat-tool-result__truncated-badge`

### 13.2 — Enhanced header bar

- `headerTitle` element (`.chat-header-bar__title`) added to left side of header bar, shows current task title (truncated, flex-1)
- `headerStop` button (`.chat-header-bar__stop`) added to right side; hidden by default (`.chat-header-bar__stop--hidden`); shown only when task status is `Running`; calls `api.abortTask(taskId)` on click; disabled while aborting
- `updateHeaderBar()` function: updates title text and toggles stop button visibility; called from the same store subscriptions as `updateCostDisplay()` and on initial render
- New CSS: `.chat-header-bar__title` (flex:1, truncating), `.chat-header-bar__stop` (red tinted button), `.chat-header-bar__stop--hidden`

### 13.4 — File attachment + image paste

- `attachedFiles` array tracks `{ name, type, base64? }` entries
- `attachmentPills` container (`.chat-attachments`) inserted above textarea; hidden when empty
- `attachBtn` (`.chat-attach-btn`, paperclip icon) added to input toolbar between mode pill and send button; triggers hidden `<input type="file" multiple>`
- `fileInput` change handler: images → base64 via `readFileAsBase64()`; other files → name+type only
- `textarea` paste handler: detects clipboard images, converts to base64, adds pill
- `renderAttachmentPills()` renders pills with image thumbnail previews and × remove buttons
- `readFileAsBase64(file)` helper uses `FileReader`
- Send button and keydown handler updated: if `attachedFiles.length > 0`, appends `[Attached images: name1, name2]` note to message text; clears `attachedFiles` after send
- New CSS: `.chat-attachments`, `.chat-attachment-pill`, `.chat-attachment-pill__thumb`, `.chat-attachment-pill__name`, `.chat-attachment-pill__remove`, `.chat-attach-btn`

---

## Phase 14 — Slash Commands in Chat Input (DONE)

Frontend-only changes to `src/components/agent/chat-view.js` and `src/styles/agent.css`.

- **Slash picker state** (`slashPickerItems`, `slashPickerFiltered`, `slashPickerIndex`, `slashPickerOpen`, `slashPickerLoaded`) added inside `createChatView` after `attachedFiles`
- **`slashPicker` overlay element** (`.slash-picker.slash-picker--hidden`) created and appended to `inputArea`
- **`loadSlashItems()`** — lazy-loads skills, workflows, and MCP servers from the API on first `/` trigger; populates `slashPickerItems`
- **`getSlashContext(textarea)`** — parses textarea value up to cursor for a `/query` token (at start or after whitespace); returns `{ slashStart, slashEnd, query }` or `null`
- **`filterSlashItems(query)`** — case-insensitive substring filter with prefix-match sorting; returns up to 10 results (12 if empty query)
- **`renderSlashPicker()`** — renders rows with type badge (Skill/Workflow/MCP), name, and description; highlights active row; attaches `mousedown` handler with `e.preventDefault()` to avoid textarea blur
- **`insertSlashToken(ctx, token)`** — replaces the `/query` slice in textarea with `token` and repositions cursor
- **`selectSlashItem(item)`** — workflows: fetches body via `api.getWorkflowBody()` and inserts it (fallback to `/{name}`); skills/MCP: inserts `@{name}`
- **`openSlashPicker(query)` / `closeSlashPicker()`** — toggle picker visibility and state
- **`textarea` keydown handler** updated: when picker is open, ArrowDown/ArrowUp navigate rows, Enter selects, Escape closes; normal Enter-to-send only fires when picker is closed
- **`textarea` input event** added: calls `getSlashContext`; opens picker if context found, closes it otherwise
- **`textarea` blur event** added: closes picker after 150ms delay (allows click events to fire first)
- New CSS: `.slash-picker`, `.slash-picker--hidden`, `.slash-picker__item`, `.slash-picker__item--active`, `.slash-picker__badge`, `.slash-picker__badge--skill`, `.slash-picker__badge--workflow`, `.slash-picker__badge--mcp`, `.slash-picker__name`, `.slash-picker__desc`
- `.chat-input-area` already had `position: relative` — no change needed

---

## Phase 15 — Sensitive File Protection (DONE)

Three-tier file access control applied to all file tools (`read_file`, `create_file`, `edit_file`, `apply_patch`, `insert_lines`, `delete_lines`, `list_directory`).

### Backend (`crates/rustic-agent/src/`)

- `task/mod.rs`: Added `PermissionOp::SensitiveFile { path, tier, reason }` variant with `describe()` returning `sensitive_file_tier2` or `sensitive_file_tier3` operation strings
- `tools/mod.rs`: Added `allowed_paths: Vec<String>` field to `ToolContext`
- `tools/file_ops.rs`: Added `check_sensitive_path(rel_path, full_path, context)` async helper implementing:
  - **Tier 1 (always block)**: `id_rsa`, `id_ed25519`, `id_ecdsa`, `id_dsa`, `.pem`, `.p12`, `.pfx`, `.key` files, `.aws/credentials`, `service-account*.json` — returns `SENSITIVE_FILE_BLOCKED` error, no override possible
  - **Allowlist check**: paths in `.rustic/allowed-files.txt` skip tier-2/3
  - **Tier 2 (sensitive)**: `.env`, `.env.*`, `credentials*`, `secrets*`, `*.secret`, `*.token` — prompts via permission broker unless `sensitive_files_allowed = true`
  - **Tier 3 (gitignored)**: any file matched by `.gitignore` rules via `ignore::gitignore::GitignoreBuilder` — prompts via broker unless `sensitive_files_allowed = true`
  - Wired into all 7 file operation functions before any I/O or other permission checks
- `tools/subagent_tools.rs`: `allowed_paths` cloned from parent context into child `ToolContext`

### Tauri backend (`src-tauri/src/commands/agent.rs`)

- `send_message`: reads `.rustic/allowed-files.txt` into `allowed_paths` and passes it to `ToolContext`
- System prompt error codes: added `SENSITIVE_FILE_BLOCKED` description
- `PermissionOp::SensitiveFile` events flow through the existing `agent-permission-request` Tauri event (no new commands needed); `respond_to_permission` handles them automatically

### Frontend

- `src/components/agent/chat-view.js`: `renderApprovalArea` updated — sensitive file requests get:
  - Warning triangle icon instead of edit/terminal icon
  - `chat-approval-widget--sensitive` or `chat-approval-widget--gitignored` class on the widget
  - Tier badge (`Sensitive` / `Gitignored`) prepended to the description
- `src/styles/agent.css`: Added styles for `--sensitive` (yellow left border), `--gitignored` (peach left border), and `__tier-badge` variants

---

## What Is NOT Yet Implemented

All planned phases are now complete.

### Additional decisions made (not yet in plan.md):
- **Context window**: Proactive condensation at 85% threshold using cheap model summarization; sliding window fallback — add to Phase 4 or new phase
- **API retry**: 429/500 → configurable delay + retry; tool call errors → self-healing loop — add to Phase 4
- **Thinking tokens**: Display Claude thinking blocks in collapsible section — add to Phase 13
- **Skill installation**: Use `reqwest` + tar download (not `git2`) for GitHub skills — update Phase 9
- **MCP**: `.mcp.json` only — TOML removed from scope — update Phase 8
- **npm/npx skills**: Auto-detect via `.agents/skills/` scan in addition to `.rustic/skills/`

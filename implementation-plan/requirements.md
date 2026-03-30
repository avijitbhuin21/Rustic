# Rustic Agent — Requirements
> All agent features to be implemented

---

## 0. Task Completion Tool

The model signals it is done by calling `task_complete` — a dedicated tool, not a text message.

### Why a tool instead of text
- Text "I'm done" does not stop the loop — the executor keeps waiting for more tool calls
- A tool call is unambiguous: it immediately breaks the loop, no extra provider calls
- The structured output is machine-readable for the UI to render a styled completion card
- Prevents the model from over-explaining or padding after finishing

### The `task_complete` tool (LLM provides text only)

```json
{
  "name": "task_complete",
  "parameters": {
    "summary": "One paragraph: what was accomplished and why",
    "notes":   "Optional: warnings, caveats, follow-up suggestions"
  }
}
```

The model does **NOT** provide a file list. File changes are computed from the checkpoint system — ground truth, not LLM guess.

### How file changes are computed (checkpoint-based diff)

Rustic already snapshots every file before the first write to it during a task (`snapshot_file` stores the before-content in SQLite). At `task_complete` time:

1. Collect all `FileSnapshotRow` records for this task across all its checkpoints
2. Deduplicate by file path — keep the **earliest** snapshot per file (the true pre-task state)
3. For each snapshotted file, compare snapshot content to current file on disk:
   - `was_new=true`  + file exists   → **Created** (all lines are additions)
   - `was_new=false` + file exists   → **Modified** (diff before vs after)
   - `was_new=false` + file missing  → **Deleted** (all lines are deletions)
4. Generate a unified diff string and count insertions/deletions per file using the `similar` crate

New structs in `checkpoint/mod.rs`:
```rust
pub struct TaskDiff {
    pub files: Vec<FileDiff>,
    pub total_insertions: usize,
    pub total_deletions: usize,
}

pub struct FileDiff {
    pub path: String,
    pub status: DiffStatus,     // Created | Modified | Deleted
    pub insertions: usize,
    pub deletions: usize,
    pub unified_diff: String,   // full diff text for the expand view
}

pub enum DiffStatus { Created, Modified, Deleted }
```

New function: `pub fn compute_task_diff(db: &Database, task_id: &str) -> Result<TaskDiff>`

### Executor behaviour when `task_complete` is called
1. Call `compute_task_diff(db, task_id)` — pure computation, no LLM involved
2. Immediately break the agentic loop — no further provider calls
3. Set task status to `Completed`
4. Emit `TaskEvent::TaskComplete { task_id, summary, notes, diff: TaskDiff }`

### UI rendering — completion card

```
┌───────────────────────────────────────────────────────────────┐
│  ✓  Task complete                                              │
│                                                                │
│  Refactored the auth module to use typed User structs          │
│  instead of raw strings throughout.                            │
│                                                                │
│  ▼  3 files changed   +47  -12                                 │  ← click to expand
│    ✚  src/auth.rs            +31  -8   ████████░░  [diff]      │  ← click file for diff
│    ✚  src/auth_service.rs    +12  -4   █████░░░░░  [diff]      │
│    ✕  src/old_helper.rs        0  -0   (deleted)               │
│                                                                │
│  Notes: logout() still uses &str — flagged for follow-up       │
└───────────────────────────────────────────────────────────────┘
```

- The **file list section** is collapsed by default, expands on click
- Each file row shows: status icon, path, insertion/deletion counts, mini bar chart
- Clicking a file opens the **diff view** inline (or in a side panel) — same style as the existing checkpoint diff viewer Rustic already has
- The diff is syntax-highlighted, line-numbered, with green additions and red deletions

### System prompt instruction
```
When your task is fully complete, ALWAYS call task_complete immediately.
Do not send a plain-text message saying you are done.
Do not ask "Is there anything else?" — wait for the user to reply.
The tool stops execution and the UI will automatically show the user what changed.
Call it as soon as you have nothing left to do — do not pad with extra explanation.
```

### Sub-agent completion
Sub-agents also call `task_complete`. The backend computes their diff the same way. Their `{ summary, diff }` becomes the structured result injected into the main model's context via reactive injection — replacing the fragile "extract last assistant text" heuristic.

---

## 1. Permission Modes

Four distinct modes replacing the current 3-level system:

| Mode | File Writes | Commands | Default |
|---|---|---|---|
| **Chat** | Denied | Ask user | — |
| **ManualEdit** | Ask user per-operation | Ask user | ✓ |
| **AutoEdit** | Auto-allowed | Ask user | — |
| **FullAuto** | Auto-allowed | Auto-allowed | — |

- Mode is **per-task** (not global)
- Persisted per-project as the default for new tasks
- Switchable mid-chat via dropdown in chat input toolbar
- ManualEdit shows an inline approval widget above the input box when agent wants to write or run a command

---

## 2. Token / Cost Tracking

- Accumulate `input_tokens`, `output_tokens`, `cache_read_tokens` per task across all turns
- Estimate USD cost from a hardcoded model pricing table
- Emit `CostUpdate` event after every turn
- Display live in chat header: `~1,234 tokens · $0.003`
- Tooltip breakdown: input / output / cache / turn count
- Pricing table covers Claude, OpenAI, Gemini models (updated manually)

---

## 3. Memory (memory.md)

- One file per project: `<project>/.rustic/memory.md`
- Auto-loaded at task creation (injected as first message pair, NOT re-sent every turn)
- Model uses existing terminal + write tools to manage it — no special memory tools
- System prompt tells model what it is and how/when to update it
- Memory indicator in agent panel header — click opens the file in editor
- Toast notification when model writes to memory.md
- Keep under 500 lines guideline enforced via system prompt

---

## 4. Model Switching Mid-Chat

- `switch_model(task_id, provider, model)` Tauri command
- Injects a `ModelSwitch { from, to }` content block into message history (UI-only, never sent to LLM)
- Rendered as a visual separator: `──── Model: claude-opus-4-6 ────`
- Model selector dropdown in chat input toolbar (grouped by provider)
- Abbreviated display names (e.g. `claude-sonnet-4-6` → `Sonnet 4.6`)

---

## 5. File Tools (Write Operations — Locked)

All write tools go through an atomic read-modify-write lock (per-file `tokio::Mutex`).

### Write tools
| Tool | Description |
|---|---|
| `create_file(path, content)` | New or empty files only. Rejects if file has content. |
| `edit_file(path, old_string, new_string, hint_line?)` | String-anchored replacement. Atomic RMW under lock. |
| `apply_patch(path, hunks[])` | Multiple hunks atomically. Rolls back all on any failure. |
| `insert_lines(path, after_line, content)` | Line-number insertion under lock. |
| `delete_lines(path, start_line, end_line)` | Line-number deletion under lock. |

`write_file` for existing files is **removed entirely**.

### Read / Navigation — via terminal only
Model uses `run_command` with:
- `grep -n` / `Select-String` — find symbols with line numbers
- `awk 'NR>=X&&NR<=Y{print NR": "$0}'` — read line range with real line numbers
- `wc -l` / `(Get-Content f).Count` — file size
- `cat -n | sed -n 'X,Yp'` — line-numbered range reads

System prompt instructs: always use line-number flags, never read >300 lines at once, never read a whole file >300 lines.

### Error responses
- **STALE_READ**: old_string found elsewhere (content changed) → return ±150 lines around new location
- **CONTENT_DELETED**: fuzzy search finds nothing close → return ±150 lines around `hint_line`, current nearby symbols, explicit "do not retry — escalate" message
- **LOCK_TIMEOUT**: 30s wait exceeded → retry suggestion
- All error responses hard-capped at 300 lines / 8KB

### Idempotency
Before applying `edit_file`, check if `new_string` already exists at that location. If yes → return `"Already applied — no change needed."`

---

## 6. Tool Output Hard Caps

Every tool result is capped before being returned to the model:

| Tool | Cap | Truncation message |
|---|---|---|
| `run_command` | 16KB | `[Truncated at 16KB — N more lines. Use head/tail/grep to filter.]` |
| `edit_file` error | 8KB / 300 lines | Hard cap on context window |
| `apply_patch` error | 8KB | Per failing hunk |
| MCP tool results | 25KB (configurable) | `[MCP output truncated — N chars omitted]` |
| Sub-agent result | Summarized if >2K tokens | Auto-summarized before main context injection |

---

## 7. Turn Budget

- Default: **50 turns per task** (configurable in settings)
- At turn N-5: inject `"[5 turns remaining — begin wrapping up]"`
- At turn N: stop agent loop, set status `TurnLimitReached`, show UI warning
- User can extend by clicking "Continue" (+20 turns)

---

## 8. Structured Error Codes

All tool errors include a machine-readable code at the start:

| Code | Meaning | Model action |
|---|---|---|
| `STALE_READ` | Content exists but changed | Retry with corrected old_string (provided) |
| `CONTENT_DELETED` | Target no longer exists | Do not retry — escalate to orchestrator |
| `PERMISSION_DENIED` | User denied approval | Do not retry this operation |
| `LOCK_TIMEOUT` | File locked by another agent | Retry after a moment |
| `OUTPUT_TRUNCATED` | Tool output was cut | Refine command to produce less output |
| `TURN_LIMIT_REACHED` | Task hit turn budget | Wrap up |
| `ALREADY_APPLIED` | Edit already exists | No action needed |
| `FILE_TOO_LARGE` | File exceeds safe threshold | Use terminal navigation tools |

---

## 9. Shell / OS Detection

- At task creation: run `uname -s || echo Windows` once
- Store result on the `AgentTask`
- Inject into system prompt: `"Shell environment: bash (Linux)"` or `"Shell: PowerShell (Windows)"`
- All tool error messages and system prompt examples use the correct shell syntax for the detected environment

---

## 10. MCP Configuration

### Config files
- Global: `~/.rustic/config.toml` — `[mcp_servers.<name>]` sections
- Project: `<project>/.rustic/mcp.toml` — same format, overrides global
- Compat: `<project>/.mcp.json` — Claude Code format, read-only import

### Fields per server
- `command`, `args`, `env`, `cwd` — stdio transport
- `url`, `headers` — HTTP transport
- `sse_url` — SSE transport (legacy)
- `enabled`, `trust`, `allowed_tools`, `disabled_tools`, `timeout_ms`, `required`

### Dynamic loading based on tool count
- `< 20 tools` → flat loading (all names + descriptions in context)
- `20–100 tools` → flat names + BM25 search tool in context
- `> 100 tools` → two-level hierarchy (server names only; drill down on demand)

---

## 11. Skills

### Format (Agent Skills Open Standard)
```
<project>/.rustic/skills/<name>/SKILL.md
~/.rustic/skills/<name>/SKILL.md
```

SKILL.md frontmatter: `name`, `description` (required) + `allowed-tools`, `compatibility`, `disable-model-invocation` (optional)

### Loading
- Session start: name + description only (~100 tokens per skill)
- On activation: full SKILL.md body
- Supporting files (scripts/, references/): on explicit reference only

### Installation
`rustic skills add owner/repo` — fetches via `git2`, finds SKILL.md files, copies to `.rustic/skills/`

### Manual creation
UI form (name + description + body) or direct file creation

### Auto-activation
BM25 match between user message and skill descriptions at session start

---

## 12. Workflows

### Format
```
<project>/.rustic/workflows/<name>.md
```

```markdown
---
name: deploy-staging
description: Deploy current branch to staging environment
---
Body is the full prompt sent to the agent when triggered.
```

- User-triggered only (no auto-activation, zero context cost until triggered)
- Manually created only (no installation)
- Triggered via `/workflow-name` or picker UI

### Workflow orchestration
When triggered, system prompt injection tells main model to act as orchestrator:
- Analyze workflow steps for parallelism
- Spawn sub-agents for parallel steps
- Use reactive completion injection to process results as they arrive
- Synthesize final result

---

## 13. Sub-Agent System

### Tools (main model only — depth=0)
| Tool | Behavior |
|---|---|
| `spawn_subagent(id, task, model, files[])` | Spawn and return immediately (non-blocking) |
| `wait_for_all_agents(ids[])` | Explicit block until all listed agents complete |
| `list_active_agents()` | Current status of all sub-agents |
| `cancel_agent(id)` | Cancel a running sub-agent |

Sub-agents get all file/terminal tools but NOT `spawn_subagent` (depth hard-limited to 1).

### Reactive completion injection
- After main model's turn ends with no tool calls: executor checks for active sub-agents
- If any active: wait for ANY to complete (tokio broadcast channel, zero CPU)
- On completion: inject `"[Sub-agent 'id' completed]\n<result>\n[N agents still running: ...]"` as new user message
- Main model re-invokes, processes result, continues or waits for more

### Available sub-agent models
- Settings → AI → Sub-Agent Models: checkbox list of all configured models
- User selects which models are available for sub-agents (cheaper/faster recommended)
- `spawn_subagent` tool description dynamically includes this list
- Main model picks model intelligently per task (simple search → Haiku, complex generation → Sonnet)

### File conflict prevention
1. Main model assigns non-overlapping `files[]` arrays per sub-agent
2. Per-file `tokio::Mutex` for all write operations (atomic RMW)
3. 30-second lock timeout — returns `LOCK_TIMEOUT` error
4. Deleted content returns `CONTENT_DELETED` — sub-agent escalates to orchestrator

### Sub-agent result summarization
If sub-agent result >2,000 tokens before injection into main context: auto-summarize using a cheap model (Haiku / Flash) first.

### Max concurrent sub-agents
Configurable (default: 5). Returns error if exceeded.

### UI
Collapsible "Sub-agents" section in chat view showing status, model, expandable output per agent.

---

## 14. Checkpoint Per-Turn Diff & Enhanced Revert UI

### Per-turn diff (already implemented)
- `compute_checkpoint_diff(db, task_id, checkpoint_id) -> Result<TaskDiff>` — shows exactly what changed in one agent turn
- Before state: earliest snapshot per file in that checkpoint (taken before first write)
- After state: earliest snapshot of that file in any later checkpoint (= state at start of next writing turn), or current disk state
- Exposed as `get_checkpoint_diff(task_id, checkpoint_id)` Tauri command

### Checkpoint marker in chat (updated UI)
The checkpoint marker on assistant messages now has two buttons:
- **View diff** — lazy-loads the per-turn diff and renders it inline as a collapsible file list with +/- counts and inline diff expand per file (reuses `renderCompletionCard`)
- **Revert** — reverts all files in this checkpoint to their pre-turn state (existing behavior)

### Edit message + revert suggestion
When the user edits a previous message (future feature), if that message has an associated checkpoint, the system will prompt: *"This message has associated file changes. Revert files to their state before this message?"* — Yes / No / Just resend.

---

## 15. Agent Panel UI Redesign

### Left panel layout

```
[Agent]  [History]  [Terminals]                 [+ New Task]
──────────────────────────────────────────────
▶ project-one                           [+ task]
  ├─ ◉ Refactor auth module    $0.03  Running
  └─ ✓ Add unit tests          $0.01  Done

▶ project-two                           [+ task]
  └─ ◉ Fix bug in parser       $0.02  Running
```

**Tabs:**
- **Agent** — active tasks per project (Running / Idle). Completed/closed tasks are hidden here.
- **History** — completed and stopped tasks, grouped by project, with search.
- **Terminals** — list of all terminals spawned by agents. Click to open in the terminal panel.

**Project row:**
- Expandable
- Shows active task count badge
- `+` button to create new task for this project

**Task row (active):**
- Status dot: spinning = Running, ✓ = Completed, ✕ = Failed
- Title (truncated)
- Cost (live update from CostUpdate event)
- Status label
- On click: opens chat view on the right
- On hover: show delete button (with confirmation)

**New task:**
- Dropdown button at top-right of panel
- Or `+` per-project button
- Opens dialog: project selector, task title, permission mode selector

### Parallel execution
Multiple tasks across different projects (and within the same project) run on separate threads with no shared state except the per-file lock registry. No UI changes needed — the panel just shows all running tasks simultaneously.

---

## 16. Chat View Redesign

### Message types rendered differently

| Source | Display |
|---|---|
| User text | Blue-tinted bubble, right-aligned |
| Assistant text | Neutral bubble, markdown rendered |
| `run_command` tool use | `$ command` pill. Output collapsible; if output is a file path, clicking opens it in the editor |
| `edit_file` / `apply_patch` | "Edited: path/to/file" with checkpoint diff button |
| `create_file` | "Created: path/to/file" |
| `read_file` / grep / terminal read | "Read: path [lines X–Y]" — compact single line |
| Any tool (generic) | Collapsible tool block showing name + input, result expandable |
| Sub-agent spawned | Collapsible `↳ Sub-agent: agent-id [model]` — expands to show its own message thread |
| Task complete | Completion card (already implemented) |

### Top of chat
- Task title (editable on click)
- Model indicator + switch button
- Permission mode badge
- Token/cost display (live)
- Stop/abort button (kills executor, sets status Failed)

### Bottom of chat (below input)
- Expandable file changes section (same as task completion card but always visible after task finishes)
- Model selector dropdown (left of send button)
- Permission mode selector (right of model selector)
- Slash command picker (see §18)

### File attachment + image paste
- Paperclip button in input toolbar → file picker
- Paste image from clipboard → auto-converted to base64 content block
- Attached files shown as pills above the textarea
- Sent as additional content blocks in the user message

### Sub-agent inline panel
When a sub-agent is spawned, a collapsible row appears in the chat:
```
  ↳ [●] sub-agent-id  haiku  Running...
      ├─ $ grep -n "fn login" src/auth.rs
      ├─ Edited: src/auth.rs  [diff]
      └─ ✓ Complete: Updated login signature
```
- Inherits all rendering rules of the parent chat
- Status updates live via SubagentTextDelta events

---

## 17. Slash Commands in Chat Input

When user types `/` in the chat textarea, show a picker overlay:

```
/
├─ [Skill]    code-review      Review code for issues
├─ [Skill]    test-writer      Write unit tests
├─ [MCP]      filesystem       Browse and read files
├─ [MCP]      github           GitHub API access
├─ [Workflow] deploy-staging   Deploy to staging env
└─ [Workflow] run-tests        Run full test suite
```

- BM25 search on further typing (e.g. `/rev` narrows to `code-review`)
- Labels: `[Skill]`, `[MCP]`, `[Workflow]` with distinct colors
- Enter/click to insert the name into the message
- Skills and MCP are also auto-discovered by the model at runtime; slash just lets the user explicitly tag one

---

## 18. Sensitive File Protection

### Three-tier system

| Tier | Patterns | Behavior |
|---|---|---|
| **Blocked** | `id_rsa`, `id_ed25519`, `*.pem`, `*.p12`, `*.pfx`, `*.key`, `~/.aws/credentials`, service account JSONs | Read/write **always blocked** — no override, in any mode |
| **Sensitive** | `.env`, `.env.*`, `credentials.*`, `secrets.*`, `*.secret`, `*.token` | Per-access confirmation popup, even in FullAuto |
| **Gitignored** | Any file matching `.gitignore` rules | Warning shown on first access per session; requires one confirmation per session |

### Detection
- Before any `read_file` / `edit_file` / `create_file` / `run_command` that references a path:
  - Check tier-1 patterns → block immediately, return `SENSITIVE_FILE_BLOCKED` error
  - Check tier-2 patterns → show `SensitiveFileRequest` event to UI, await one-shot channel (same pattern as ManualEdit)
  - Check gitignore → same as tier-2 but message says "this file is gitignored"
- Gitignore check: load `.gitignore` at task start, cache rules for the session

### FullAuto confirmation modal — two options
When the user switches to FullAuto, show a modal with **two choices**:

```
┌─────────────────────────────────────────────────────┐
│  Switch to Full Auto mode?                           │
│                                                      │
│  The agent will run all commands and edit files      │
│  without asking for permission.                      │
│                                                      │
│  How should sensitive files be handled?              │
│                                                      │
│  ○  Ask before reading .env, credentials, etc.       │
│     (Recommended)                                    │
│                                                      │
│  ○  Allow all — including sensitive files            │
│     (.env, gitignored files, credentials)            │
│     Tier-1 secrets (private keys) are always blocked │
│                                                      │
│               [Cancel]  [Confirm]                    │
└─────────────────────────────────────────────────────┘
```

- **Ask before sensitive files** (default) — FullAuto for normal files, but tier-2/tier-3 still show a confirmation popup
- **Allow all** — tier-2 and gitignored files are silently allowed; only tier-1 (private keys, certs) remain blocked forever

The choice is stored as `sensitive_files_allowed: bool` on the task. It can be changed later by re-selecting the FullAuto option.

### Per-project allowlist
`<project>/.rustic/allowed-files.txt` — line-separated paths the user has explicitly pre-approved. These skip the confirmation prompt for tier-2 and gitignored files.

---

## 19. Additional UX Enhancements

### Abort/stop task
- While a task is Running, the **last user message** shows a stop/cancel overlay button — clicking the message you just sent stops the task
- Backend: `abort_task(task_id)` Tauri command sets an `Arc<AtomicBool>` cancellation flag
- Executor checks the flag at the start of each loop iteration (before provider call and before each tool execution)
- On cancellation: emit `StatusChange(Stopped)`, break the loop

### Copy message
- Copy icon appears on hover over any message block
- Copies raw text content (not HTML) to clipboard

### Retry
- Retry icon on the last user message if task status is Failed
- Resends the same message, clearing the failed state

### Token budget visual
- Progress bar in chat header: used / max (e.g. `14,570 / 200,000`)
- Color shifts yellow at 70%, red at 90%

### Truncation indicator
- When tool output was capped, show a yellow pill in the tool result block: `Output truncated at 16KB`

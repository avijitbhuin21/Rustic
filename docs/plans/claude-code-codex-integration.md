# Claude Code & Codex Integration Plan

**Goal:** Let Rustic users run agent tasks through their own Claude Code (Pro/Max subscription) or OpenAI Codex (ChatGPT Plus/Pro subscription) by spawning the official CLI binaries the user has already authenticated locally. The user brings their own login; Rustic never sees credentials. This is the same pattern T3 Code uses, which Anthropic has explicitly confirmed is allowed.

**What we are NOT doing:** Forwarding OAuth tokens, spoofing client identity, building our own auth harness, or asking users for an Anthropic API key for the subscription path. Those approaches get accounts banned (OpenClaw precedent). The "BYO API key" path through `provider/claude.rs` already exists and stays untouched.

---

## A. Implemented

What has actually shipped, ordered by chunk. Citations point at the canonical implementation site so future readers can confirm the claim.

### A.1 Backend foundation (chunk 1)
* `harness/` module skeleton — [crates/rustic-agent/src/harness/mod.rs](../../crates/rustic-agent/src/harness/mod.rs): `Harness` + `HarnessSession` traits, `HarnessEvent` enum, `HarnessRegistry` with `shutdown_all()`.
* Cross-platform process spawning — [crates/rustic-agent/src/harness/process_spawn.rs](../../crates/rustic-agent/src/harness/process_spawn.rs): `cmd /C` shim resolution on Windows, `CREATE_NO_WINDOW`, Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` so Node descendants die with the parent.
* NDJSON framing — [crates/rustic-agent/src/harness/stream_json.rs](../../crates/rustic-agent/src/harness/stream_json.rs): `NdjsonReader` (also `Stream`-impl), mutex-guarded `NdjsonWriter`, always `\n`-terminated.
* Auth probe — [crates/rustic-agent/src/harness/auth_check.rs](../../crates/rustic-agent/src/harness/auth_check.rs): four-state `HarnessAuthStatus`, 5-second timeout on `--version`, scans `~/.claude/.credentials.json` and `~/.codex/auth.json`.

### A.2 Claude Code happy path (chunk 2)
* `ClaudeCodeHarness` + `ClaudeCodeSession` — [crates/rustic-agent/src/harness/claude_code.rs](../../crates/rustic-agent/src/harness/claude_code.rs): spawns `claude --print --output-format stream-json --input-format stream-json --include-partial-messages --permission-mode <mode> --cwd <root>`.
* `ProviderType::ClaudeCode` + `is_harness_provider_key()` — [crates/rustic-agent/src/config.rs](../../crates/rustic-agent/src/config.rs).
* Tauri dispatch boundary — [src-tauri/src/commands/agent/harness_runtime.rs](../../src-tauri/src/commands/agent/harness_runtime.rs): forwards `HarnessEvent`s to existing `agent-stream` / `agent-task-status` / `agent-task-complete` / `agent-cost-update` / `agent-request-usage` events. Bypasses `TaskExecutor` entirely.
* App-quit cleanup — [src-tauri/src/commands/app.rs](../../src-tauri/src/commands/app.rs): `confirm_quit` calls `harness_registry.shutdown_all()` before `app.exit(0)`.
* Subscriptions card in AI Settings — [src/components/settings/ai-settings.js](../../src/components/settings/ai-settings.js).
* Reserved Global scratch dir — already created at startup in [src-tauri/src/lib.rs](../../src-tauri/src/lib.rs) as `<app_data>/global_scope/`.

### A.3 Char-by-char streaming + tool events (chunk 3a)
* `--include-partial-messages` flag passed on spawn so `stream_event.content_block_delta.text_delta` events fire per-character.
* Event-map extension — [crates/rustic-agent/src/harness/event_map.rs](../../crates/rustic-agent/src/harness/event_map.rs): translates `system:init`, `stream_event` (text & thinking deltas), `assistant.tool_use`, `user.tool_result`, `result`. 12 unit tests.
* Tool / thinking / tool-result events forwarded through to existing `agent-tool-use` / `agent-tool-result` / `agent-thinking-delta` Tauri events.

### A.4 Three-button permission card (chunk 3b)
* Detected `control_request` envelope with `subtype: can_use_tool` — [crates/rustic-agent/src/harness/event_map.rs](../../crates/rustic-agent/src/harness/event_map.rs).
* `respond_to_permission` writes the canonical `control_response` envelope — [crates/rustic-agent/src/harness/claude_code.rs](../../crates/rustic-agent/src/harness/claude_code.rs):
  * `Accept` → `{"behavior":"allow","updatedInput":{}}`
  * `AcceptForSession` → `behavior:allow` + `updatedPermissions: [addRules{tool, behavior:allow, destination:session}]` so the CLI's own session allowlist remembers it.
  * `Deny` → `{"behavior":"deny","message":"User declined tool execution."}`.
* Tauri command `respond_to_permission` accepts both legacy `approved: bool` and the new `decision: "accept"|"acceptForSession"|"deny"`. Native-task path collapses `acceptForSession` → `true` until A-side native allowlists ship.
* Frontend "Allow / Allow for session / Deny" — visible only for harness tasks — in [src/components/agent/chat-view.js](../../src/components/agent/chat-view.js).

### A.5 Real diffs for Edit/Write/MultiEdit (chunk 3c)
* `formatToolInput` produces a unified-diff string — [src/components/agent/chat-view/tool-meta.js](../../src/components/agent/chat-view/tool-meta.js): `--- a/path` / `+++ b/path` headers + `-old`/`+new` lines, `MultiEdit` becomes `@@ edit N of M @@` hunks.
* Scratch editor opens with `lang='diff'` for these tools.
* `TOOL_META` covers all Claude Code tool names: `Read`, `Glob`, `Grep`, `Bash`, `BashOutput`, `KillShell`, `Edit`, `MultiEdit`, `Write`, `NotebookEdit`, `TodoWrite`, `Task`, `WebFetch`, `WebSearch`, `ExitPlanMode`, `AskUserQuestion`. (Note: `Task` still renders as a generic card — see B.1.)

### A.6 Interrupt + delete cleanup (chunk 4a)
* `ClaudeCodeSession::interrupt()` writes `control_request` with `subtype: interrupt`.
* `abort_task` — [src-tauri/src/commands/agent/runtime.rs](../../src-tauri/src/commands/agent/runtime.rs): flips cancel token, then for harness tasks fires interrupt on a worker thread.
* `delete_task` + `delete_tasks_for_project` call `shutdown_harness_for_task` before DB delete — [src-tauri/src/commands/agent/mod.rs](../../src-tauri/src/commands/agent/mod.rs).

### A.7 Resume support (chunk 4b)
* DB migration `008_harness_session_id` — [crates/rustic-db/src/migrations/008_harness_session_id.sql](../../crates/rustic-db/src/migrations/008_harness_session_id.sql).
* `TaskRow.harness_session_id: Option<String>` with `update_task_harness_session_id` setter.
* On `HarnessEvent::SessionReady`, persist the CLI's session id; on first spawn for a task, hydrate it as `--resume <id>`.

### A.8 Crash recovery + interrupt escalation (chunk 4c)
* `HarnessSession::stderr_tail()` trait method (default empty); `ClaudeCodeSession` returns its rolling 64 KB tail.
* Recv-loop close without `TurnComplete` → captured as crash, surfaces stderr tail in chat, drops registry slot. Resume-failure self-heal: if `system:init` never fired, clears the stale `harness_session_id`.
* `abort_task` escalation: write interrupt → wait 2 s → if still in registry, force `shutdown()` (Job Object kill).

### A.9 Auth detection panel (chunk 5)
* `probe_harness_auth(kind, binary_path)` Tauri command — [src-tauri/src/commands/agent/harness_probe.rs](../../src-tauri/src/commands/agent/harness_probe.rs).
* Subscriptions card probes on mount, before Enable, and on Re-check. Disables Enable unless `authenticated`. Shows specific text for `not_installed` / `not_authenticated` / `probe_failed`.

### A.10 Tool-block persistence (chunk 6)
* `harness_runtime.rs` builds proper Anthropic-API interleaved blocks (`Assistant: [Text, ToolUse]` / `User: [ToolResult]`) so reload-after-restart shows tool cards, not just text.
* Three helpers: `append_assistant_text`, `append_assistant_tool_use`, `append_user_tool_result`.

### A.11 Slash command autocomplete (chunk 7)
* `list_claude_code_slash_commands(project_root)` — [src-tauri/src/commands/agent/harness_slash.rs](../../src-tauri/src/commands/agent/harness_slash.rs): scans `~/.claude/commands/*.md` + `<project>/.claude/commands/*.md`, plus 12 hardcoded builtins. H1-or-first-line description extraction. 5 unit tests.
* Frontend: chat input `/` picker shows commands with a "Claude" badge for harness tasks. Selection inlines literal `/foo ` (forwarded verbatim to CLI stdin).

### A.12 Mid-turn steering — queue + harness Stop & send (chunk 8)
* `pendingUserInput: {[taskId]: [{text, images}]}` state slice + `queueMessage` / `clearQueuedMessage` actions in [src/state/agent.js](../../src/state/agent.js).
* Send button has three modes (`'send' | 'stop' | 'queue'`) driven by `(isRunning, hasInputContent)`.
* Auto-drain on Running → not-Running transition, concatenating text bodies with `\n\n`.
* "Stop & send" secondary button visible only for harness tasks while streaming with input typed.
* Yellow "Queued" pill rows above the input with dismiss `×`.

### A.13 Polish bundle (chunk 9)
* Cancel branch persists `turn_messages` before early return — partial state from a cancelled turn now survives reload.
* "Advanced ▾" binary-path-override input in the Subscriptions card. Re-uses `ProviderEntry.base_url` slot to avoid a DB migration. Re-probes on `change`.

### A.14 Lifecycle quirks already handled

These were called out in the plan and verified:
* Windows `cmd /C` shim resolution + `CREATE_NO_WINDOW` + Job Object kill-on-close (§8.1).
* App quit kills every live session (§8 / §16).
* Crash recovery surfaces stderr tail (§15).
* Interrupt escalation ladder (write → 2 s → TerminateProcess) on Windows (§13.5).
* Resume across app restart via `--resume <session_id>` (§16).
* Cwd for Global orchestrator chats uses the per-user scratch dir registered at startup (§13.2).

---

## B. Remaining

**Snapshot (April 2026):** B.1 – B.10, B.12, and B.14 are DONE (entries below preserved as a paper trail of what shipped, with file pointers). B.13 dropped. **The only Phase 3 item still outstanding is B.11 (per-task git worktrees).** Two live smoke tests are owed before declaring full Phase 1+2 victory: (a) image-input forwarding (B.8 — paste an image into a Claude Code task, confirm the model sees it); (b) Codex end-to-end round-trip (B.10 — enable Codex provider, send a simple message, confirm streaming + tool cards + approval flow work).

Ordered roughly by user-visible impact, not by chunk size.

### B.1 Task tool → nested subagent card (Phase 1, §6.2)
**Status:** DONE.
**What shipped:** [src-tauri/src/commands/agent/harness_runtime.rs](../../src-tauri/src/commands/agent/harness_runtime.rs) maintains a per-turn `task_subagents: HashMap<tool_use_id, agent_id>` map; on a `Task` tool_use it emits `agent-subagent-spawned`, on the matching tool_result it emits `agent-subagent-completed` / `-failed`. `agent_id` is derived by a `slugify_agent_name` helper that mirrors `slugifyAgentName` in [src/components/agent/chat-view.js](../../src/components/agent/chat-view.js) (lowercase, non-alphanum→hyphen, trim, 30-cap), with a unit test pinning the two impls together. `chat-view.js` routes `Task` to `renderSubagentCard` alongside the native `spawn_subagent`. `AgentSubagent*Event` structs in [src-tauri/src/commands/agent/mod.rs](../../src-tauri/src/commands/agent/mod.rs) flipped `pub(super)` so the harness module can emit them. Card transitions running → completed straight from the tool_result content (Claude Code doesn't stream sub-agent text deltas).

### B.2 Native-provider Stop & send (Phase 1, §14)
**Status:** DONE.
**What shipped:** [crates/rustic-agent/src/task/executor.rs](../../crates/rustic-agent/src/task/executor.rs) now keeps a per-iteration `partial_assistant_text: Arc<Mutex<String>>` accumulator wired into the stream callback's `TextDelta` arm; cleared at the top of each iteration. On the `Err("Task cancelled")` branch, if the buffer is non-empty, the executor pushes a `Message { Assistant, [Text{partial}] }` to `messages` and emits `MessageComplete` before returning. The existing post-`run_turn` persistence in [src-tauri/src/commands/agent/mod.rs](../../src-tauri/src/commands/agent/mod.rs) handles the rest. Tool calls from the cancelled iteration are intentionally discarded (their JSON would be incomplete; re-feeding would confuse the model). `chat-view.js` dropped the `isHarnessTask` gate on `stopSendBtn.style.display`.

### B.3 Native-provider session-scoped allowlist (Phase 1, §5.1)
**Status:** DONE.
**What shipped:** [crates/rustic-agent/src/task/permission_broker.rs](../../crates/rustic-agent/src/task/permission_broker.rs) gained `NativePermissionDecision` (Accept/AcceptForSession/Deny), a per-task `session_allowlist: HashMap<task_id, HashSet<signature>>`, and a `respond_with_decision` method; legacy `respond(bool)` collapses to Accept|Deny for backwards compat. `request()` early-returns `true` when the op's signature matches an allowed one. Signature derivation: `WriteFile`→`write_file`; `CreateFile`→`create_file`; `RunCommand(cmd)`→`run_command:<basename(first_word)>` (so trusting `npm install` allows other `npm` calls but not `rm`); `SensitiveFile { .. }`→`None` (always re-prompts regardless of decision — security tier opts out). [src-tauri/src/commands/agent/runtime.rs](../../src-tauri/src/commands/agent/runtime.rs) routes the 3-state UI decision through `respond_with_decision`; `delete_task` / `delete_tasks_for_project` clear the per-task allowlist. Frontend: dropped the harness-only gate; all three buttons send string decisions uniformly.

### B.4 Onboarding-wizard panel (§4)
**Status:** DONE.
**What shipped:** [src/components/onboarding/onboarding-wizard.js](../../src/components/onboarding/onboarding-wizard.js) has a "Use a subscription instead" subsection in the providers step. New `buildSubscriptionCard` helper probes on mount; renders Probing / Installed & signed in / Not signed in / Not installed / Probe failed states; exposes Sign in / Enable / Re-check buttons. Sign in opens a terminal in the bottom panel via `createTerminalSession`, types the configured command (`claude` or `codex login`) after a 250ms beat, and watches `terminalStore.sessions` for the tab to disappear → auto re-probe. Enable mirrors the Settings card path (`setAiProvider` + `saveProviderConfigs` + dispatch `rustic:provider-configs-changed` so Continue/Skip updates). Install-help URL is provider-aware. Both Claude Code and Codex rows now ship in the wizard. CSS for `onboarding__divider`/`__subsection-title`/`__inline-code` in [src/styles/onboarding.css](../../src/styles/onboarding.css).

### B.5 Idle reaper (§8)
**Status:** DONE.
**What shipped:** Default `async fn last_active(&self) -> Instant` on the `HarnessSession` trait (returns `Instant::now()` for harnesses that don't track activity → never reaped). `HarnessRegistry::reap_idle(threshold)` snapshots the registry first (no lock held during per-session checks), filters by threshold, and reaps with a `Arc::ptr_eq` guard so a freshly-spawned session for the same task can't be killed by a stale candidate. `ClaudeCodeSession` got a `last_active: Arc<AsyncMutex<Instant>>` field bumped by the reader task on every envelope and by `send_user_message` at entry; `CodexSession` gets the same treatment. [src-tauri/src/lib.rs](../../src-tauri/src/lib.rs) spawns a `tauri::async_runtime` task at startup ticking every 60s with `Duration::from_secs(15 * 60)` threshold (skips first tick to avoid no-op startup reap). Resume across reap is automatic via the persisted `harness_session_id` + `--resume <id>` (Claude) or `thread/resume` (Codex).

### B.6 Concurrency-cap warning (§9) + B.14 live counter
**Status:** DONE (shipped together).
**What shipped:** New `HarnessRegistry::task_ids() -> Vec<String>` snapshot reader. New `harness_active_task_ids` Tauri command exposing the snapshot to the frontend. [src/components/agent/agent-panel.js](../../src/components/agent/agent-panel.js) polls every 5s with a cheap set-equality check so renders only happen on actual changes. Header pill `agent-panel__live-agents` shows global count (hidden when zero); per-project `agent-project__cap-warning` banner appears when ≥ 4 harness sessions live in that project. Banner shows even when project section is collapsed so the warning can't get hidden by accident. Triangle-with-exclamation icon on amber background per [src/styles/chat-features.css](../../src/styles/chat-features.css).

### B.7 "Subscription session" cost label (§10)
**Status:** DONE.
**What shipped:** [src/components/agent/chat-view.js](../../src/components/agent/chat-view.js) `updateCostDisplay` derives `isSubscriptionTask` from the active task's `provider_type`. Subscription tasks render the literal "subscription" in `progressCostLabel` and replace the header `$` pill with an `∞ subscription` pill (italic, aqua icon) carrying the `chat-header-stat--cost-subscription` modifier; tooltip explains "Tokens billed against your Claude subscription — no per-call USD cost." Progress-bar tooltip swaps "Est. cost: $X" for "Billing: Claude subscription (no per-call USD)". Native API-key tasks render unchanged. CSS: [src/styles/chat-layout.css](../../src/styles/chat-layout.css).

### B.8 Image-input forwarding verification (§7.2)
**Status:** DONE — unit-test verified; live smoke test still owed.
**What shipped:** Extracted the stream-json envelope construction from [crates/rustic-agent/src/harness/claude_code.rs](../../crates/rustic-agent/src/harness/claude_code.rs) `send_user_message` into a free `build_user_envelope(text, images)` helper. 4 unit tests cover text-only, text + image, image-only (correctly drops empty text block — the API rejects them), and multi-image ordering. Envelope shape verified to match Anthropic Messages API content-block protocol exactly (`{type:"image", source:{type:"base64", media_type, data}}`). Live smoke test (paste real image into chat, confirm model sees it) still pending.

### B.9 Multi-client queue events (§14)
**Status:** DONE — forward-compat scaffolding.
**What shipped:** New Tauri commands `notify_input_queued(task_id, preview, image_count, queue_depth)` and `notify_input_delivered(task_id, count)` in [src-tauri/src/commands/agent/runtime.rs](../../src-tauri/src/commands/agent/runtime.rs); each just emits the corresponding event. JS bindings + listener APIs in [src/lib/tauri-api.js](../../src/lib/tauri-api.js). [src/state/agent.js](../../src/state/agent.js) `queueMessage` calls `notifyInputQueued` (preview truncated to 240 chars; full text never crosses the boundary); `drainPendingUserInput` calls `notifyInputDelivered`. Listener stubs in `initInputQueueEvents` are no-ops today (originating window already mutated state synchronously); document where state-mirroring code goes when multi-window lands. Behavior today: identical UX. Behavior tomorrow: drop-in mirror logic in the two listener stubs.

### B.10 Codex (Phase 2)
**Status:** DONE — feature-complete; live smoke test still owed.
**What shipped:**
* `ProviderType::Codex` enum variant + `is_harness_provider_key("Codex")` returns true ([crates/rustic-agent/src/config.rs](../../crates/rustic-agent/src/config.rs)).
* JSON-RPC 2.0 framing module [crates/rustic-agent/src/harness/jsonrpc.rs](../../crates/rustic-agent/src/harness/jsonrpc.rs): `JsonRpcMessage` enum (Request/Notification/Response/Error), `decode`/`encode_request`/`encode_response`/`encode_error`, `RequestId` (string|int), `PendingRequests` correlator with `allocate`/`resolve`/`fail_all`. 8 unit tests.
* [crates/rustic-agent/src/harness/codex.rs](../../crates/rustic-agent/src/harness/codex.rs): real `CodexSession` spawning `codex app-server --listen stdio://`, sending `initialize` + `thread/start` (or `thread/resume`), capturing `thread.id` as session_id. `send_user_message` fires `turn/start` with text + image inputs; `interrupt` fires `turn/interrupt`. Approval flow: `pending_approvals: HashMap<request_id, PendingApproval>`; reader task captures `item/{commandExecution,fileChange,permissions}/requestApproval` server requests, emits `HarnessEvent::PermissionRequest` with the right tool-card label and `itemId`; `respond_to_permission` echoes the original RequestId with the v2 camelCase decision payload (`accept | acceptForSession | decline`). Permission-mode→sandboxMode/approvalPolicy mapping. 5 unit tests.
* [crates/rustic-agent/src/harness/event_map_codex.rs](../../crates/rustic-agent/src/harness/event_map_codex.rs): translation table for `item/agentMessage/delta` (text), `item/reasoning/textDelta`/`summaryTextDelta` (thinking), `item/started`/`item/completed` (tool cards for commandExecution/fileChange/mcpToolCall/dynamicToolCall/webSearch), `turn/completed` + `thread/tokenUsage/updated` (usage), `error` (errors). Thread/turn lifecycle notifications no-op cleanly. 8 unit tests.
* [src-tauri/src/commands/agent/harness_runtime.rs](../../src-tauri/src/commands/agent/harness_runtime.rs) dispatches by `provider_type` → boxed `dyn Harness`; spawn-failure error bubble adapts wording per provider.
* Subscriptions card in [src/components/settings/ai-settings.js](../../src/components/settings/ai-settings.js) and onboarding wizard row in [src/components/onboarding/onboarding-wizard.js](../../src/components/onboarding/onboarding-wizard.js); install-help URL is provider-aware (Codex → developers.openai.com/codex/cli, Claude → docs.claude.com).
* Schema dump committed under [docs/codex-schema/](../codex-schema/) (190 v2 schemas + top-level + v1) so future Codex CLI versions can be diffed against this 0.125.x baseline.
**Known follow-up polish:** file-change diff rendering for `fileChange` tool items (currently a generic card); path-scope nuance for `item/permissions/requestApproval` (currently treated as plain accept/decline); MCP-elicitation + free-form `item/tool/requestUserInput` flows.

### B.11 Per-task git worktrees (Phase 3)
**Status:** TODO. Plan calls this Phase 3.
**What's needed:** Per-task worktree spawn in [crates/rustic-git/](../../crates/rustic-git/), worktree-merge UX for completed tasks.
**Estimate:** ~3–5 days.

### B.12 MCP server passthrough (Phase 3)
**Status:** DONE.
**What shipped:**
* New `mcp_config_path: Option<PathBuf>` field on `HarnessSessionOpts` ([crates/rustic-agent/src/harness/mod.rs](../../crates/rustic-agent/src/harness/mod.rs)).
* `ClaudeCodeSession::spawn` argv builder factored into a free `build_spawn_args` helper ([crates/rustic-agent/src/harness/claude_code.rs](../../crates/rustic-agent/src/harness/claude_code.rs)) that appends `--mcp-config <path>` when `mcp_config_path` points at an existing file. Existence check is intentional — when the user hasn't opened the MCP panel yet there's no `mcp.json` on disk, and passing the flag in that case would make the CLI error before the first turn. We deliberately do **not** pass `--strict-mcp-config`, so Rustic-managed servers stack on top of any servers the user added via `claude mcp add` (CLI-level ones live in `~/.claude.json`).
* Project-scope `.mcp.json` is auto-discovered by Claude Code from `--cwd`, so no extra plumbing is needed for project-scoped servers.
* Codex spawn ignores `mcp_config_path` with a `tracing::debug` note ([crates/rustic-agent/src/harness/codex.rs](../../crates/rustic-agent/src/harness/codex.rs)): the `app-server` binary reads MCP servers from `~/.codex/config.toml`'s `[mcp_servers]` section directly, and there is no JSON-RPC override surface on `initialize`/`thread/start` to inject them. Translating Rustic's JSON config into the user's TOML would risk clobbering hand-edits, so users register MCP servers for Codex via the `codex` CLI itself; Rustic's MCP panel governs Claude Code only.
* Dispatch wiring: `harness_runtime.rs` resolves `<app_data_dir>/mcp.json` once via `tauri::Manager::path(app).app_data_dir()` and threads it into `HarnessSessionOpts.mcp_config_path` for every fresh session spawn (resumed sessions reuse the live process, no re-injection needed).
* Three unit tests in [crates/rustic-agent/src/harness/claude_code.rs](../../crates/rustic-agent/src/harness/claude_code.rs): (1) flag absent when path unset; (2) flag absent when path doesn't exist on disk; (3) flag present and pointing at the right path when the file exists.

### B.13 Rate-limit pill (§10.1)
**Status:** DROPPED.
**Rationale:** Schema-verified absent — Claude Code's stream-json `result` envelope doesn't carry rate-limit data; that field only flows into the StatusLine hook input, not the protocol output. Codex emits `account/rateLimits/updated` notifications in its v2 schema, so a Codex-only rate-limit pill is feasible later if there's demand. Until then the cost pill ("subscription" — see B.7) communicates the right thing for harness tasks: "you're on a flat plan, watch your weekly window". Re-open this if/when Anthropic's stream-json adds the field, or with a Codex-only scope.

### B.14 Concurrency-cap UI counter (§9)
**Status:** DONE (shipped with B.6 — see that entry).

---



Rustic's existing `Provider` trait ([crates/rustic-agent/src/provider/mod.rs](../../crates/rustic-agent/src/provider/mod.rs)) models a **model API client**: send messages + tools, receive a stream of `TextDelta` / `ThinkingDelta` / `ServerToolUse` events. The Rust backend owns the agent loop — it executes tools, manages permissions, runs the conversation.

Claude Code and Codex are **complete agents**, not models. They have their own:
- System prompts and tool definitions
- Tool execution (Read/Write/Edit/Bash/Grep/Task/etc.)
- Permission modes
- MCP server support
- Session/resume state

If we shoehorn them into `Provider`, we'd be re-implementing things the CLI already does, and fighting the existing tool-loop in [crates/rustic-agent/src/task/](../../crates/rustic-agent/src/task/).

Cleaner: introduce a sibling abstraction — call it `Harness` — that represents an external agent process. It has a simpler surface than `Provider`:

```rust
// crates/rustic-agent/src/harness/mod.rs (new)
#[async_trait]
pub trait Harness: Send + Sync {
    async fn start_session(&self, opts: HarnessSessionOpts) -> Result<HarnessSession>;
}

pub trait HarnessSession: Send + Sync {
    fn send_user_message(&self, text: String, images: Vec<Image>) -> Result<()>;
    fn respond_to_permission(&self, request_id: String, decision: PermissionDecision) -> Result<()>;
    fn respond_to_question(&self, request_id: String, answer: String) -> Result<()>;
    fn interrupt(&self) -> Result<()>;
    fn shutdown(&self) -> Result<()>;
    fn events(&self) -> impl Stream<Item = HarnessEvent>;
}
```

`HarnessEvent` maps 1:1 to the existing frontend event protocol (`onAgentStream`, `onAgentToolUse`, `onAgentToolResult`, `onAgentPermissionRequest`, `onAgentTaskStatus`, `onAgentCostUpdate`, `onAgentTodoUpdated`, etc. — all already wired up in [src/state/agent.js](../../src/state/agent.js)). That means **the frontend barely changes** — the harness just emits the same events the existing providers do.

The task runtime in [crates/rustic-agent/src/task/](../../crates/rustic-agent/src/task/) gets a branch: if the task's selected provider is a harness, dispatch to harness runtime instead of the model→tool-loop pipeline.

### 1.1 What to skip from the existing pipeline

When dispatching to a harness, **bypass these existing pieces** — the CLI handles them itself, and double-injecting causes duplicated context and conflicting tool definitions:

- **System prompt assembly** ([crates/rustic-agent/src/system_prompt.rs](../../crates/rustic-agent/src/system_prompt.rs)) — Claude Code and Codex generate their own system prompts including tool definitions, project context, and operating instructions. Injecting Rustic's own system prompt on top would confuse the agent. Skip entirely for harness providers.
- **Tool registration** — the existing tool list in [crates/rustic-agent/src/tools/](../../crates/rustic-agent/src/tools/) is for Rustic's native agent loop. Harness providers ship their own tool implementations; do not register Rustic's tools with them.
- **Per-turn project context injection** — anything Rustic appends to the user message (open-files snapshot, git status, etc.) should be skipped or moved to a Rustic-as-MCP-server in a future phase. For Phase 1: skip cleanly; the user types what they want.

This is enforced at the dispatch boundary in `task/runtime.rs` — the harness branch builds a minimal `HarnessSessionOpts { cwd, permission_mode, resume_session_id }` and passes raw user input through, no transformation.

---

## 2. Transport: the official CLI in stream-json mode

Both Claude Code and Codex expose a documented headless protocol designed for exactly this — programmatic UIs wrapping the CLI. **No SDK needed; we drive the binary directly from Rust with `tokio::process::Command`.**

### Claude Code

```
claude --print \
       --output-format stream-json \
       --input-format stream-json \
       --permission-mode <ifNeeded|acceptEdits|bypassPermissions> \
       --cwd <project_root>
```

- Stdin: newline-delimited JSON, one envelope per user message / tool-permission response / interrupt
- Stdout: newline-delimited JSON, one envelope per event (`assistant`, `user`, `system`, `result`, etc.)
- Auth: inherits from `~/.claude/` — the user must have run `claude` interactively at least once and signed in
- Resume: `--resume <session_id>` to continue a past conversation
- The binary path is configurable (settings setting), defaulting to `claude` on PATH

### Codex

```
codex app-server
```

- JSON-RPC 2.0 over stdio
- Method names like `session.create`, `session.sendInput`, notifications for streaming events
- Auth: inherits from `~/.codex/` — user runs `codex login` first
- T3 Code's transport adapter at [references/t3code/apps/server/src/provider/Layers/CodexSessionRuntime.ts](../../references/t3code/apps/server/src/provider/Layers/CodexSessionRuntime.ts) is a working reference for the protocol shape

### Why CLI not SDK

The `@anthropic-ai/claude-agent-sdk` is a Node library that itself spawns `claude` and parses the same stream-json. From Rust, going through the SDK would require bundling a Node helper process — adds complexity, more memory, no benefit. Driving the CLI directly keeps everything in the existing Rust runtime.

---

## 3. Module layout (new code)

```
crates/rustic-agent/src/
├── harness/
│   ├── mod.rs              # Harness trait, HarnessEvent enum, registry
│   ├── claude_code.rs      # ClaudeCodeHarness — spawns `claude` CLI
│   ├── codex.rs            # CodexHarness — spawns `codex app-server`
│   ├── stream_json.rs      # Shared NDJSON framing helpers
│   ├── event_map.rs        # Translate harness output → HarnessEvent → TaskEvent
│   └── auth_check.rs       # Detect installed binaries + login state
├── task/
│   └── runtime.rs          # ADD: branch on provider_kind to dispatch to harness
└── ...
```

Frontend additions live mostly in `src/state/` and `src/components/agent/`:

```
src/components/agent/
└── chat-view.js            # ADD: tool-card components for Claude Code's tool set
src/components/onboarding/
└── (extend)                # ADD: Claude Code + Codex auth detection panels
src/state/
└── agent.js                # MINIMAL CHANGES — most events already match
```

---

## 4. Auth & onboarding

We never store credentials. We **detect** that the user has authenticated:

| Provider | Detection |
|---|---|
| Claude Code | Run `claude --version`. If the binary exists, check for `~/.claude/` and a non-empty `credentials.json` (or platform-equivalent). If missing, prompt the user to run `claude` once in a terminal and sign in. |
| Codex | Run `codex --version`. Check `~/.codex/` for auth state. If missing, prompt to run `codex login`. |

Onboarding flow (extends [src/components/onboarding/](../../src/components/onboarding/)):

1. Detect installed binaries; show a row per provider with status: `Installed & authenticated` / `Installed, not signed in` / `Not installed`.
2. For `Not installed`: link to the official install page (do not auto-install).
3. For `Not signed in`: show a button that opens a terminal in Rustic's existing terminal panel with the right command pre-filled (`claude` or `codex login`). Re-detect on terminal exit.
4. Settings panel ([src/components/settings/ai-settings.js](../../src/components/settings/ai-settings.js)) gets a new "Subscriptions" section showing the same status, plus a "binary path override" field for users who installed to a non-PATH location.

When a task is launched against a harness provider that isn't authenticated, surface a clear in-chat error (the existing `classifySendError` flow in [src/state/agent.js](../../src/state/agent.js#L638) handles this — add a new error kind `harness_not_authenticated`).

---

## 5. Permission bridging

Rustic's existing permission model (`FullAuto`, `Supervised`, `ReadOnly`, sensitive-file gating) needs to map cleanly onto each CLI's mode:

| Rustic mode | Claude Code flag | Codex equivalent |
|---|---|---|
| `FullAuto` (no sensitive) | `--permission-mode acceptEdits` | `approval_policy: "on-failure"`, `sandbox_mode: "workspace-write"` |
| `FullAuto` (sensitive allowed) | `--permission-mode bypassPermissions` | `approval_policy: "never"`, `sandbox_mode: "danger-full-access"` |
| `Supervised` | `--permission-mode ifNeeded` | `approval_policy: "on-request"`, `sandbox_mode: "workspace-write"` |
| `ReadOnly` | `--permission-mode plan` (or equivalent) | `sandbox_mode: "read-only"` |

When the CLI emits a permission request event, we forward it via `onAgentPermissionRequest` (existing event, see [src/state/agent.js:242](../../src/state/agent.js#L242)). User approval flows back through `respond_to_permission` on the harness session, which writes the decision envelope to the CLI's stdin.

### 5.1 Three-option permission decision (extending the existing two-button flow)

Rustic's existing `respondToPermission(taskId, requestId, approved)` ([src/state/agent.js:976](../../src/state/agent.js#L976)) takes a boolean. Claude Code's `canUseTool` callback supports **three** outcomes:

| Decision | Effect |
|---|---|
| `accept` | Allow this single tool call; the next call will prompt again. |
| `acceptForSession` | Allow this tool with these arguments (or this tool's class) for the rest of the session — never prompt again. |
| `deny` | Reject the call; the agent gets an error and decides what to do next. |

Without the middle option, the user gets re-prompted constantly for the same tool, which is the dominant friction point in any agent UI. We need to extend:

- **Backend:** `PermissionDecision` enum with three variants. `respond_to_permission` signature changes from `bool` to that enum.
- **Tauri command** (`commands/agent/runtime.rs` or wherever the permission response is handled): accept the new shape.
- **Frontend store:** `respondToPermission(taskId, requestId, decision: 'accept' | 'acceptForSession' | 'deny')`.
- **UI:** the existing inline permission card in [src/components/agent/chat-view.js](../../src/components/agent/chat-view.js) gets a third button "Allow for session" between Allow and Deny. Native API-key providers can ignore the middle option (treat as `accept`) for now — Phase 2 follow-up to add session-scoped allowlists for native providers.

The permission card's existing "remember this" toggle (if any) becomes redundant — the third button replaces that pattern with something agents understand.

---

## 6. Tool rendering

Claude Code emits its own tool set (`Read`, `Write`, `Edit`, `Bash`, `Glob`, `Grep`, `TodoWrite`, `Task`, `WebFetch`, `WebSearch`, `NotebookEdit`, plus user-installed MCP tools). Rustic's existing tool cards in [src/components/agent/chat-view.js](../../src/components/agent/chat-view.js) render Rustic's own tool names (`create_file`, `edit_file`, `apply_patch`, `run_command`, etc.).

Mapping strategy:
- **Map common ones to existing cards** where the visual is essentially identical (Read → file viewer card, Write/Edit → diff card, Bash → command card, Grep/Glob → search card).
- **Generic fallback card** for anything we don't recognize (TodoWrite, Task, MCP tools, user-defined). Shows tool name as title, JSON-pretty-printed input collapsed by default, output below. This guarantees no tool ever renders as raw JSON in the message body.
- **TodoWrite gets special treatment** — Claude Code uses it heavily; render as a checklist that updates in place (Rustic's existing `onAgentTodoUpdated` event flow in [src/state/agent.js:326](../../src/state/agent.js#L326) already supports this; we just need to translate Claude's TodoWrite tool calls into that event).

A small mapping table lives in `crates/rustic-agent/src/harness/event_map.rs` so when Claude emits `tool_use { name: "Edit", input: {...} }`, it's translated to a Rustic `ToolUse` event with the tool name normalized.

### 6.1 Real diffs for `Edit` and `Write`

Claude Code's `Edit` tool input carries `file_path`, `old_string`, and `new_string`; `Write` carries `file_path` and `content`. Rendering these as raw JSON in the tool card is unreadable. Rustic already has a diff renderer (used in checkpoint and git views — see [src/components/git/](../../src/components/git/) and [src/styles/git.css](../../src/styles/git.css)). Reuse it:

- **`Edit` cards:** read `old_string`/`new_string` from the tool input, render as a unified diff inside the card. Show file path as the card title with line range.
- **`Write` cards:** read current on-disk content of `file_path` (best-effort — file may not exist for new files), diff against `content`, render. For brand-new files, render as "+ all lines" without a base.
- **Codex equivalent tools:** Codex's edit-emitting tools have a slightly different shape. Map them to the same renderer in `event_map.rs`.

Wiring: the harness translates these tool inputs into a structured `ToolUse` event with a `diff_payload: Option<DiffPayload>` field. The frontend tool-card branches: if `diff_payload` is present, render diff view; otherwise generic JSON card.

### 6.2 Skill / subagent (Task tool) cards

Claude Code's `Task` tool spawns a subagent. Rustic already has a subagent rendering system ([src/state/agent.js:427](../../src/state/agent.js#L427)) with `onAgentSubagentSpawned`/`Completed`/`Failed`/`TextDelta` events. Translate `Task` tool calls into those events and Rustic's existing subagent UI just works — nested conversation card with streaming progress, final report, token usage. Phase 2 work; flagging now so we don't render `Task` as a generic card and re-do the work later.

---

## 7. Input affordances (slash commands & image forwarding)

### 7.1 Slash commands

Claude Code reads custom slash commands from `~/.claude/commands/` (user-global) and `<project>/.claude/commands/` (project-scoped). When the user types `/foo`, the CLI expands it into the appropriate prompt template. Forwarding the literal `/foo ...` string to the CLI's stdin works transparently — **no integration work needed for execution**.

What we DO need: surface the available slash commands as **autocomplete in our chat input** ([src/components/agent/chat-view.js](../../src/components/agent/chat-view.js)) so users can discover them.

- On harness session start, read `~/.claude/commands/*.md` and `<project>/.claude/commands/*.md`. Each file's name (without `.md`) is the command; the first line of the file (or the H1) is the description.
- When the chat input begins with `/`, show a dropdown filtered by typed prefix. Selecting one inserts the full slash command name; the user types arguments after it.
- Do the same for Codex's slash command directory if/when Codex supports the equivalent.
- Built-in CLI slash commands (`/clear`, `/help`, etc.) — hardcode a baseline list per provider.

Cache per-session at start; refresh on file-watcher event for the commands directory.

### 7.2 Image input forwarding

Rustic's existing `sendMessage` ([src/state/agent.js:549](../../src/state/agent.js#L549)) already accepts images as `{ media_type, data }` objects (base64). Stream-json's user envelope supports `image` content blocks in the same shape:

```json
{ "type": "user", "content": [
    { "type": "text", "text": "..." },
    { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": "..." } }
] }
```

The harness's `send_user_message(text, images)` already takes images in the trait — just translate to that envelope. Verify Phase 1: paste an image into the chat with a Claude Code task active and confirm the agent sees it.

---

## 8. Process & session lifecycle

One CLI process per active conversation. Rules:

- **Lazy spawn** — process starts on first user message in a task, not on task creation.
- **Idle reaper** — if a task is idle (no streaming, no pending input) for N minutes, kill the process. Resume by spawning a fresh one with `--resume <session_id>` next time the user sends a message.
- **Hard kill on task delete** — `deleteTaskAction` already exists ([src/state/agent.js:1156](../../src/state/agent.js#L1156)); add a backend cleanup that ensures the process dies before DB cleanup.
- **App quit** — kill all live harness processes from the existing close-requested handler in [src/main.js:200](../../src/main.js#L200).
- **Crash recovery** — if a CLI process exits unexpectedly, mark the task `Failed` and surface the exit code + last few stderr lines in an in-chat error bubble.

State lives in:
- `HarnessRegistry` (Rust): `Map<task_id, HarnessSession>` — owns the running processes
- `agentStore.tasks` (JS): unchanged — already tracks status/streaming/messages

Resume: store the CLI's session ID on the task row in `rustic-db`. On task reopen, pass `--resume` (Claude) or session-restore RPC (Codex). The DB schema needs one new nullable column: `harness_session_id TEXT`.

### 8.1 Windows-specific spawning (critical for Rustic's primary platform)

Naive `Command::new("claude").spawn()` from Rust on Windows fails for non-obvious reasons. The known landmines:

- **`.cmd` shim resolution.** Claude Code installs as `claude.cmd` (a Node shim), not `claude.exe`. Windows PATH lookup with `Command::new("claude")` does NOT auto-resolve `.cmd` extensions the way `cmd.exe` does. We need to either: (a) explicitly find the `.cmd` via `where claude` parsing, or (b) spawn through `cmd /C claude ...` so the shell does the resolution. T3 Code uses approach (b) — `shell: process.platform === "win32"` ([CodexSessionRuntime.ts:687](../../references/t3code/apps/server/src/provider/Layers/CodexSessionRuntime.ts#L687)). We'll match that: on Windows, spawn `cmd.exe /C claude --output-format stream-json ...` instead of `claude` directly.
- **`CREATE_NO_WINDOW` flag.** Without it, every spawn flashes a console window. Use `std::os::windows::process::CommandExt::creation_flags(0x0800_0000)` (`CREATE_NO_WINDOW`).
- **No SIGINT.** Windows has no SIGINT — the interrupt-escalation ladder from Section 13.5 needs a Windows path: stdin envelope → `GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT)` (requires sharing a console group, which `cmd /C` complicates) → `TerminateProcess` as the hard kill. In practice for our use case, **fall back to TerminateProcess directly on Windows** if the stdin interrupt doesn't ack within 2s; the graceful-via-CTRL-BREAK path is unreliable enough that it's not worth implementing.
- **Process-group cleanup.** When we kill the parent `cmd.exe`, the child Node process can survive. Use a Job Object (`CreateJobObject` + `JobObjectExtendedLimitInformation` with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`) to guarantee descendants die when the parent dies. This is non-negotiable for the idle-reaper and app-quit cleanup paths.
- **Line endings.** Stream-json envelopes are NDJSON — `\n` only, never `\r\n`. Rust's `BufWriter::write_all(b"...\n")` is fine; just don't accidentally call `writeln!` with the platform's line ending in the framing layer.
- **Path separators in `cwd`.** Claude Code accepts both `/` and `\` on Windows but is more reliable with `\`. Pass project paths as-is (Rustic stores them with native separators in [crates/rustic-core](../../crates/rustic-core/)); don't normalize.

These quirks are isolated to a new `harness/process_spawn.rs` module that abstracts platform differences. `claude_code.rs` and `codex.rs` stay platform-clean.

---

## 9. Parallel tasks

The architecture is naturally parallel — each task = its own process. Rustic's task system already supports concurrent tasks per project; harnesses inherit that for free. **Two things to add:**

1. **Concurrency cap (UI-side):** soft limit of 4 active harness sessions before warning the user about memory pressure. Surfaced in the agent-panel header.
2. **Worktree isolation (later phase):** for users running multiple agents on the same project simultaneously, scaffold per-task git worktrees so file edits don't clobber each other. Defer to Phase 3 — start with a "you have 2 agents running in this project, file conflicts possible" warning banner.

---

## 10. Costs & telemetry

The CLIs emit token/cost info in their `result` envelopes (Claude) or notification messages (Codex). Translate these into Rustic's existing `onAgentRequestUsage` and `onAgentCostUpdate` events ([src/state/agent.js:251](../../src/state/agent.js#L251)) — same event shape, the existing per-turn cost badge and total cost pill just work.

One caveat: subscription usage doesn't have a clean USD figure (Anthropic exposes "tokens used" but not dollar cost on Pro/Max). Show "subscription session — token usage" instead of a dollar amount when the task's provider is a harness.

### 10.1 Subscription rate-limit indicator

For users on Pro/Max, the *real* budget is the rolling 5-hour usage window, not dollars. Claude Code's `result` envelopes include rate-limit info (`rate_limits` block with `requests_remaining`, `tokens_remaining`, `reset_at` for the active window). Surface this:

- **New backend event** `onAgentRateLimitUpdate { task_id, window: '5h'|'weekly', percent_used, resets_at_iso }` emitted whenever the harness sees a rate-limit block in a result envelope.
- **New state slice** in [src/state/agent.js](../../src/state/agent.js): `rateLimits: { taskId -> { window, percent, resetsAt } }`.
- **UI:** a compact pill in the chat header (next to the cost pill) for harness tasks. Rendering:
  - `< 50% used` → no pill (cleaner default)
  - `50–80%` → neutral pill: `5h: 67% · resets 14:30`
  - `80–95%` → warning pill (amber)
  - `≥ 95%` → danger pill + tooltip explaining what happens at limit (Claude Code stops responding until reset)
- **Codex equivalent:** Codex emits its own rate-limit signals in JSON-RPC notifications. Map to the same pill. If the data isn't surfaced, hide the pill cleanly — never show a stale value.
- **Native API-key providers:** the pill stays hidden — those users are billed per-token directly, the existing dollar pill is the right metric.

Cheap to add since we're already parsing result envelopes for cost data. Phase 1 scope.

---

## 11. Memory & performance footprint

Per concurrent active harness session, on top of Rustic's baseline (~100–150 MB Tauri app):

| Process | Memory (active) | Memory (idle) |
|---|---|---|
| `claude` CLI (Node) | ~150–300 MB (grows with conversation length, MCP servers add ~30–100 MB each) | 0 (process killed) |
| `codex app-server` (Rust) | ~50–100 MB | 0 |

So 1 Claude session + 1 Codex session running = roughly **+250–400 MB total** while active, returning to ~0 when idle (assuming idle reaper is wired up). Speed-wise, IPC over stdio is microsecond-level — the dominant latency is network round-trip to Anthropic/OpenAI servers, same as the existing providers.

---

## 12. Phased rollout

### Phase 1 — Claude Code MVP + mid-turn steering (~7–10 days)
**Core integration:**
- New `harness/` module, `Harness` trait, `ClaudeCodeHarness`
- `harness/process_spawn.rs` — Windows-aware spawning with Job Object + `CREATE_NO_WINDOW` (Section 8.1)
- Stream-json reader/writer + event translation for common event types (assistant text, tool_use, tool_result, permission_request, result, thinking, rate_limit)
- Binary detection via `claude --version`, settings UI for binary path override (Section 13 / 4)
- Onboarding panel with auth-status detection
- Provider picker entry: "Claude Code (subscription)"
- DB column for `harness_session_id`, resume support
- Idle reaper, lazy spawn, hard kill on delete
- **Bypass existing system-prompt + tool-injection pipeline** at the harness dispatch boundary (Section 1.1)

**UI / rendering:**
- Generic fallback tool card; specific cards for Read/Write/Edit/Bash/Grep
- Real diffs for `Edit` and `Write` tool cards (Section 6.1)
- Three-option permission card: Allow / Allow for session / Deny (Section 5.1)
- Slash command autocomplete in chat input (Section 7.1)
- Image input forwarding via stream-json `image` content blocks (Section 7.2)
- Subscription rate-limit pill in chat header (Section 10.1)

**Cross-provider feature:**
- **Mid-turn steering (Section 14):** queue + interrupt-and-send for both Claude Code AND existing native API-key providers. Frontend UI changes apply to all providers.

**Out of scope for Phase 1:** Codex, parallel-session UX, MCP forwarding, worktrees, `Task` subagent rendering, session-scoped allowlists for native providers.

### Phase 2 — Codex + tool polish (~3–4 days)
- `CodexHarness` with JSON-RPC client over stdio
- Codex-specific permission mode mapping
- TodoWrite → Rustic todo panel translation for Claude (existing `onAgentTodoUpdated` flow)
- Task tool (subagents) → render as nested conversation card (Rustic's subagent system at [src/state/agent.js:427](../../src/state/agent.js#L427) is the integration point)
- Concurrency cap + warning banner ("3 agents active — file conflicts possible if same project")

### Phase 3 — Same-project parallelism (~3–5 days)
- Per-task git worktree scaffolding (extend [crates/rustic-git/](../../crates/rustic-git/))
- Worktree-merge UX for completed tasks
- MCP server passthrough — let users register MCP servers that the harness inherits

**Total estimate:** ~11–16 days of focused work across all three phases.

---

## 13. Resolved decisions

1. **Binary path config — RESOLVED.** Match T3 Code's proven pattern (confirmed in [references/t3code/apps/server/src/provider/Layers/ClaudeAdapter.ts:2835](../../references/t3code/apps/server/src/provider/Layers/ClaudeAdapter.ts#L2835)): store a `binaryPath` setting per provider (Claude Code / Codex), default to the binary name on PATH (`"claude"` / `"codex"`), allow explicit absolute-path override.
   - Settings UI (extends [src/components/settings/ai-settings.js](../../src/components/settings/ai-settings.js)): a "Subscriptions" section. Each provider has an enable toggle + a binary-path field. When the user enables a provider or edits the path, run `<binary> --version` to verify and surface the reported version. If the command fails, show an inline error with the underlying stderr ("not found", "permission denied", etc.).
   - Same flow for Codex — run `codex --version` to detect and validate.

2. **Global working directory — RESOLVED.** Use a per-user scratch directory under Tauri's app-data dir (e.g. `<appdata>/rustic/global-scratch/`). Created on first Global chat, never deleted by Rustic. The directory exists purely to satisfy Claude Code's `--cwd` requirement; no real project files live there.

3. **Multi-account support — RESOLVED: not supported.** The user authenticates with each CLI themselves (`claude` interactive, `codex login`). Rustic uses whatever `~/.claude/` and `~/.codex/` contain. If a user wants to switch accounts, they switch at the CLI level. No `CLAUDE_HOME` per-task plumbing.

4. **Streaming edits to open buffers — RESOLVED.** Rely on the existing file-watcher in [src-tauri/src/watcher.rs](../../src-tauri/src/watcher.rs) to detect on-disk changes and trigger Rustic's standard "external edit" reload flow on open buffers. No special harness-specific path. If we hit perceptible lag during Phase 1 testing (Windows file events can be 50–200 ms), we'll add a fast-path that uses the harness's own `tool_result` event for `Edit`/`Write` to reload the buffer in-memory before the watcher fires. Don't pre-optimize.

5. **Interrupt semantics — RESOLVED.** Map Rustic's existing `abortTask` ([src/components/agent/agent-panel.js:509](../../src/components/agent/agent-panel.js#L509)) to a graceful → forceful escalation:
   1. Write an `interrupt` envelope to the CLI's stdin (Claude Code's stream-json supports this; Codex has an explicit JSON-RPC method).
   2. If the CLI doesn't acknowledge within 2 s, send SIGINT to the child.
   3. If still alive after another 3 s, SIGKILL.
   - For native API-key providers (existing path), interrupt means dropping the in-flight HTTP stream — already implemented; leave alone.

---

## 14. Mid-turn user input (steering) — NEW FEATURE

This is a **net-new capability** that applies to **both the harness providers AND the existing native API-key providers**. The user wants to type and send a message while the agent is mid-execution to steer it ("actually, do X first" / "skip that file" / "stop, I changed my mind"). This is a significant UX upgrade and worth doing properly across all providers.

### Two distinct behaviors, exposed as two buttons

While a task is `Running`, the input box stays enabled. The send action splits in two:

| Action | UX | Semantics |
|---|---|---|
| **Queue** (default — primary button) | Send button label changes to "Queue" while streaming. After clicking, the message renders inline with a "queued" pill. | Message is held client-side (or stdin-side for harnesses) and delivered as the next user turn the moment the current turn ends. Multiple queued messages get concatenated with newlines. |
| **Interrupt & send now** (secondary — small button next to send) | Stop-icon button, only visible when streaming. | Aborts current turn immediately, then sends the typed message as a new user turn. Conversation history includes whatever assistant output had been streamed before the interrupt. |

Both are non-destructive — the user can always recover. Queueing is safer (no work lost); interrupting is faster (steers immediately).

### Implementation per provider type

**Harness providers (Claude Code, Codex):**
- Queue: write the new user-message envelope to stdin while the CLI is mid-turn. Both CLIs handle queueing internally — they finish the current turn, then process the queued user message as the next turn. **No client-side queue logic needed** — the CLI is the queue.
- Interrupt & send: write `interrupt` envelope, wait for ack, then write the new user-message envelope. The escalation ladder from Section 13.5 applies if the CLI hangs.

**Native API-key providers (existing `provider/claude.rs`, `openai.rs`, `gemini.rs`, `compatible.rs`):**
- Queue: hold the message in a per-task pending-input queue in [crates/rustic-agent/src/task/](../../crates/rustic-agent/src/task/). When the model emits `stop_reason: end_turn` (or equivalent) and the tool-loop has no more tool calls to dispatch, drain the queue, concatenate, and immediately start the next turn with that as the new user message.
  - **Edge case — agent in a tool loop:** the agent might do `tool_use → tool_result → assistant_text → tool_use → ...` for many cycles before hitting `end_turn`. Don't inject queued input mid-loop; wait for genuine turn end. This keeps the agent's reasoning coherent.
- Interrupt & send: cancel the in-flight HTTP stream, write the assistant's partial text and any completed tool results to the conversation history, append the user's new message, start a new turn. Same code path as the existing `abortTask`, plus an immediate follow-up `sendMessage`.

### Frontend changes

- [src/state/agent.js](../../src/state/agent.js): add `pendingUserInput: { taskId -> [{ text, images }] }` to `agentStore`. Add `queueMessage(taskId, text, images)` action that appends; show as a queued bubble in the chat. `sendMessage` becomes context-aware: if task is `Running`, route to queue.
- [src/components/agent/chat-view.js](../../src/components/agent/chat-view.js): render queued user messages with a distinct "Queued" pill until they're sent. Add the small "interrupt & send now" button next to the send button, visible only when `isStreaming`.
- New backend events: `onAgentInputQueued` (echoes back so all clients see the queue state), `onAgentInputDelivered` (when a queued message becomes the active user turn — used to remove the "queued" pill and treat it as a normal message).

### Why include native providers in the same plan

The user explicitly called this out as desired across the board. Doing it once, in the agent-panel UI, means we ship the feature uniformly and don't need a "Claude Code only" caveat. The implementation is split — harness providers get it for free via stdin; native providers need ~1 day of work in the task runtime — but the UX is identical.

---

## 15. Risk register

| Risk | Mitigation |
|---|---|
| Anthropic changes the stream-json schema | Keep `event_map.rs` narrow and well-typed; pin a tested CLI version range; fail loudly on unknown envelope kinds rather than silently dropping. |
| User's `claude` CLI is too old / missing flags | Detect version on startup; show clear "update Claude Code to ≥ X" message. |
| Process leak on crash | Rust `Drop` impl on `HarnessSession` kills the child; tracked by `HarnessRegistry`; integration test that simulates panic. |
| Token-cost display confusing on subscription | Hide USD; show "tokens used (subscription)" instead. |
| User runs same project in two parallel harness sessions and they clobber each other | Phase 1: warning banner. Phase 3: real worktree isolation. |
| Anthropic's stance on "wrapping the CLI" changes | Code path is isolated to `harness/`; if banned, we just remove the menu entries — no entanglement with the API-key path. |
| Mid-turn queue delivery races with `end_turn` detection | Native-provider queue drains only after the tool-loop confirms no pending tool dispatches AND the model emitted a real `end_turn`. Add an integration test that fires queue + tool_use simultaneously to catch ordering bugs. |

---

## 16. Done definition (Phase 1)

- User can install Rustic, install `claude` CLI separately, sign in via `claude` once, and from Rustic's agent panel start a task using "Claude Code (subscription)".
- Streaming text, tool calls, and permission prompts render correctly.
- Conversation persists across restart (resume works).
- Closing the task tab kills the underlying process.
- App quit cleans up all running harness sessions.
- An unauthenticated user gets a clear, actionable error.

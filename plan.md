# Rustic vs CLAURST — Gap Closure Plan

This is a working document of what CLAURST does that Rustic doesn't, plus live bugs and UX fixes from our own daily use. Ranked by impact-per-effort and grounded in our actual architecture (Tauri desktop app, multi-task USP, 5 crates, harness mode for Claude Code/Codex CLIs).

LSP is explicitly **out of scope**. Claude Code itself ships with no built-in LSP (plugin-only, opt-in — see `src/services/lsp/config.ts` in the leaked source). The agent functions without it. We replace that layer with **tree-sitter + a workspace symbol index** that stays cheap across 6 projects and 3–4 concurrent tasks.

---

## Tier P0 — Reliability fixes (do first) — ✅ COMPLETED 2026-05-14

All 9 items shipped. See audit notes at the bottom of this section.

High value, low effort, mostly bug-class fixes in the agent loop.

**Priority order within P0:** P0.5 → P0.6 → P0.7 (R.2-derived fixes — biggest measurable productivity wins) → P0.9 (harness prompts vanishing — blocks daily harness-mode use) → P0.8 (prerequisite for P0.4 to work in harness mode) → P0.1 → P0.2 → P0.3 → P0.4. R.2 evidence in [docs/perf_findings.md](docs/perf_findings.md) shows that shipping just P0.5 + P0.6 + P0.7 (~3.5 days) is projected to make Rustic 2.8× cheaper and 2× faster on the canonical task set.

### P0.1 — Stream-stall recovery with incremental retry

**What.** When an Anthropic (or any provider) stream goes silent for >30s, retry the request. Up to **3 attempts total** with widening backoff: immediate retry, then 30s wait, then 60s wait. Total max wait ≈ 90s. After the third failure, surface the error to the user. **No fallback model** — the user picked their model, we don't silently switch it.

**Why.** Today the executor errors out the moment a stream stalls. With 3–4 concurrent tasks this is a multiple-times-per-day annoyance. Incremental backoff handles transient network blips and provider overloads without changing model behaviour.

**Where.** [crates/rustic-agent/src/task/executor.rs:184](crates/rustic-agent/src/task/executor.rs#L184) — the main `loop` in `run_turn`. Wrap the provider streaming call with a deadline tracker. On timeout or transient error, retry with the backoff schedule. Emit `TaskEvent::StreamRetry { attempt, waiting_ms }` so the UI can show "retrying in 30s" instead of looking frozen.

**Effort.** ~1 day.

---

### P0.2 — `ask_user` tool (multi-question, single + multi-choice)

**What.** A structured tool the agent calls when it needs answers mid-turn. Supports **multiple questions in a single call**, each with its own option type:

```json
{
  "questions": [
    { "id": "framework", "text": "Which framework?", "kind": "single",
      "options": ["React", "Vue", "Svelte"] },
    { "id": "features", "text": "Pick features to include", "kind": "multi",
      "options": ["auth", "billing", "analytics"] },
    { "id": "notes", "text": "Anything else?", "kind": "free_text" }
  ]
}
```

Returns the user's answers keyed by `id`. The Tauri UI renders it as a proper question panel — radio buttons for `single`, checkboxes for `multi`, textarea for `free_text`.

**Reference.** Claude Code's `AskUserQuestionTool` ([src/tools/AskUserQuestionTool/](references/claude_code_structure/claude-code-main/claude-code-main/src/tools/AskUserQuestionTool)) does this — we should port its schema shape and prompt wording for consistency.

**Where.** New file `crates/rustic-agent/src/tools/ask_user.rs`. Wire dispatch into [tools/mod.rs:402](crates/rustic-agent/src/tools/mod.rs#L402). Add a Tauri event `agent.ask_user_question` and a matching dialog component on the frontend.

**Effort.** ~1.5 days (tool + Tauri command + UI component).

---

### P0.3 — Plan mode (refactor from existing orchestrator read-only gating)

**What.** A per-task toggle the user can flip from the UI: in plan mode, `create_file`, `edit_file`, `apply_patch`, `run_command`, `kill_terminal`, and any MCP write-tools are blocked. Read tools, search, and analysis tools still work. `web_search` and `web_fetch` are allowed (read-only externally). User exits plan mode to apply changes.

**Why this is mostly already built.** The Global orchestrator at [system_prompt.rs:554](crates/rustic-agent/src/system_prompt.rs#L554) is already read-only-by-policy, and the tool filter exists at [executor.rs:912](crates/rustic-agent/src/task/executor.rs#L912) (`BuiltinTools::is_read_only(name)`). We just need to:

1. Extract the read-only flag from "is this the orchestrator?" into a per-task `is_plan_mode` boolean
2. Plumb it through `ToolContext` and the executor's tool-partition step
3. Add a system-prompt addendum when plan mode is active ("You are in plan mode — only investigate and propose; you cannot write or execute")
4. Add a UI toggle on the task panel

**Where.** [task/permissions.rs](crates/rustic-agent/src/task/permissions.rs), [system_prompt.rs](crates/rustic-agent/src/system_prompt.rs), Tauri command for the toggle.

**Effort.** ~half day (it's a refactor, not a new build).

---

### P0.4 — Global concurrency cap + global cost ceiling in Settings

**What.** Two cross-task budgets, both exposed in the Settings UI:
1. **Max concurrent provider streams** (default 6) — gates how many tasks can be calling Anthropic/OpenAI/Gemini in parallel. `Arc<Semaphore>` acquired before the provider call.
2. **Daily cost ceiling** (USD) — `Arc<AtomicU64>` of total cents spent today, reset at midnight. Checked before each new turn; if exceeded, task pauses with a UI prompt to raise the ceiling or stop.

**Why.** With our concurrent-task USP we will get rate-limited or surprise-billed without this. CLAURST has per-loop budget but not a global cross-task ceiling — easy win over them.

**Where.** New `crates/rustic-agent/src/budget.rs`. Hook into Settings UI (`src/components/settings/`). Wire into [task/executor.rs:184](crates/rustic-agent/src/task/executor.rs#L184).

**Effort.** ~half day.

---

### P0.5 — `edit_file` STALE_READ rename + whitespace-tolerant match (R.2 F1)

**What.** Two related changes to [tools/file_ops.rs](crates/rustic-agent/src/tools/file_ops.rs) `execute_edit_file`:

1. **Rename the error.** `STALE_READ: The file has changed since you last read it` is misleading when the actual cause is byte-mismatch on `old_string`. Reserve `STALE_READ` for actual mtime-based external changes; introduce `EDIT_NO_MATCH` for the case where `old_string` doesn't byte-match the file. The current branding makes the agent retry as "file changed, let me re-read" instead of "my match string had wrong whitespace."
2. **Whitespace-tolerant matching as a fallback.** Try exact match first; if it fails, normalize whitespace (strip per-line trailing, collapse line-ending differences) on both sides and retry. If the fallback succeeds, emit a warning so we still catch genuinely-wrong match strings.
3. **Better error context on failure.** When the match truly fails, surface the top 3 candidate lines closest to the agent's `old_string` instead of dumping ±150 lines.

**Why.** R.2 evidence: in T1, the agent's first 3 `edit_file` attempts all failed with this misleading error on the same multi-line doc comment. Claude CLI on the same prompt hit zero failed edits. This is the single most repeatable productivity gap.

**Where.** [crates/rustic-agent/src/tools/file_ops.rs](crates/rustic-agent/src/tools/file_ops.rs) — `execute_edit_file` and the `STALE_READ` formatter.

**Effort.** ~1 day.

---

### P0.6 — Prompt prefix cache stability (R.2 F3)

**What.** Investigate and fix whatever is causing Rustic's prompt prefix to invalidate the cache every turn. R.2 measured **~12K new cached tokens per turn in Rustic vs ~2.5K in Claude CLI** — the resulting 15× cache-creation tax is the single biggest cost driver across all three test tasks.

**Investigation step (~1 day):**
- Capture two consecutive turn prompts to disk during a representative run (a 5-turn task in the experiment sandbox is enough)
- Diff them byte-by-byte
- Identify what changes turn-to-turn (likely candidates per R.2 hypotheses: file-tree snapshot regenerated, todo state injected, tool defs reordered, embedded timestamps, MCP schemas live-fetched)

**Fix step (~1 day):**
- Hoist per-turn-variable content **out of the system prompt** into the most recent user message or a tool result (mirrors what Anthropic's own `--exclude-dynamic-system-prompt-sections` flag does for the same reason)
- Mark stable sections with `cache_control: ephemeral`; leave the tail without `cache_control` so only the changing portion gets rewritten
- For sources of non-determinism (HashMap iteration order, parallel-fetched MCP defs), enforce stable sort

**Why.** R.2 projected impact: cutting cache creation from ~12K/turn to ~3K/turn closes ~80% of the cost gap on its own. Cheapest possible "make Rustic dramatically less expensive" lever.

**Where.** Investigation in [system_prompt.rs](crates/rustic-agent/src/system_prompt.rs) and [task/executor.rs](crates/rustic-agent/src/task/executor.rs) prompt-assembly. Fix locations depend on what the diff finds.

**Effort.** ~2 days (~1 investigation + ~1 fix).

---

### P0.7 — `read_file` range guidance + shell-read soft-warn (R.2 F2)

**What.** Push the agent toward `read_file` with `start_line`/`end_line` instead of shell `Get-Content`/`sed`/`head`/`tail` for reading file ranges.

1. **System prompt update.** Add an explicit "PREFER `read_file` with start_line/end_line over shell read commands — it's faster, more reliable, and doesn't burn shell context" line near the top of the tool guidance section in [system_prompt.rs](crates/rustic-agent/src/system_prompt.rs).
2. **`run_command` description hint.** When detecting shell read commands (`sed -n`, `head`, `tail`, `Get-Content`, `cat`), prepend a soft-warn to the tool result: "Note: for reading specific lines, `read_file` with start_line/end_line is preferred."
3. **Optional v2 (skip unless v1 doesn't help):** Hard-reject shell read commands with an error pointing at `read_file`.

**Why.** R.2 evidence: T3 had ~25 PowerShell `Get-Content` calls (and 4 failed `sed`/`wc`/`findstr` attempts) where Claude CLI used native `Read` calls. T2 hit the same pattern. The agent has the right capability available but isn't picking it up from the current prompt.

**Where.** [system_prompt.rs](crates/rustic-agent/src/system_prompt.rs), [tools/terminal.rs](crates/rustic-agent/src/tools/terminal.rs).

**Effort.** ~half day.

---

### P0.8 — Reliable cost tracking in harness mode (Claude Code + Codex)

**What.** Fix the three concrete gaps that make harness-mode cost reporting unreliable today. The plumbing exists ([harness_runtime.rs:818-862](src-tauri/src/commands/agent/harness_runtime.rs#L818-L862) and [event_map.rs:319-341](crates/rustic-agent/src/harness/event_map.rs#L319-L341)); these are targeted fixes, not a from-scratch build.

**Three bugs to close:**

1. **Model name often missing → cost = $0.** The runtime falls back to $0 when `prep.model` is empty ([harness_runtime.rs:819](src-tauri/src/commands/agent/harness_runtime.rs#L819)). Claude Code emits the model in `system:init` (`"model": "claude-opus-4-7[1m]"`) and on every `assistant.message.model`. Codex emits it on session init too. Capture it from the first init event and stash in session state — don't depend on the caller passing it through.
2. **Local recompute instead of trusting CLI's `total_cost_usd`.** Claude Code's `result` event ships its own `total_cost_usd` (and `modelUsage` breakdown — see the smoke-test output we captured during R.2). When present, use it directly. Only fall back to `calculate_cost(model, &usage)` if the CLI didn't provide one. Same for Codex's `thread/tokenUsage` if it carries a cost field.
3. **Multi-model cost ignored.** Claude Code's `result.modelUsage` has per-model entries (e.g. Opus 8806 output tokens AND Haiku 547 input tokens for the same task — the Haiku usage comes from auto-mode background work). Currently we attribute everything to one model. Sum across all entries.

**UI clarity addition:**
4. **Tag cost as "estimated" or "billed" depending on auth mode.** When Claude Code is on a subscription (OAuth/Pro/Team), the actual user-visible charge is $0 because their plan covers it — what we report is an API-equivalent estimate. When on raw `ANTHROPIC_API_KEY`, it's the real charge. Surface this distinction in the cost panel with a small tag (`≈$0.45 (sub estimate)` vs `$0.45 (API)`). Read `apiKeySource` from Claude Code's `system:init` event — it already tells us which auth mode is active (we saw `"apiKeySource":"ANTHROPIC_API_KEY"` in the smoke-test output).

**Why.** Today users see `$0` for many harness runs even when the tokens are being tracked, because the model name pathway is shaky. Once P0.4 (global daily cost ceiling) ships, that ceiling won't work for harness tasks if their cost reads $0. P0.8 is a prerequisite for P0.4 functioning end-to-end. Also closes the only remaining blind spot in cost reporting between native and harness modes.

**Where.**
- [crates/rustic-agent/src/harness/event_map.rs](crates/rustic-agent/src/harness/event_map.rs) — extend `HarnessEvent::Usage` to carry the CLI-reported cost and the auth-mode tag (or add a new variant `HarnessEvent::CostUpdate`). Capture `system:init.model` and `system:init.apiKeySource` into a new `HarnessEvent::SessionInit` if not already present.
- [crates/rustic-agent/src/harness/event_map_codex.rs](crates/rustic-agent/src/harness/event_map_codex.rs) — equivalent extraction for Codex's session-start envelope.
- [src-tauri/src/commands/agent/harness_runtime.rs](src-tauri/src/commands/agent/harness_runtime.rs) — at the Usage event, prefer the CLI-reported cost over local recompute; sum `modelUsage` entries; thread auth-mode tag through to the `agent-cost-update` event.
- Frontend cost panel — show `(sub estimate)` / `(API)` tag.

**Effort.** ~1.5 days.

---

### P0.9 — Full prompt-forwarding from harness CLIs to Rustic UI

**What.** Translate every Claude Code / Codex envelope that requests user interaction into a UI prompt the user actually sees. Today only two envelope types are handled (`can_use_tool` permission requests and `user_question` control requests). Anything else — `AskUserQuestionTool` calls, `ExitPlanMode` approval requests, MCP elicitation, plan-mode confirmations — currently lands in [event_map.rs:60-61](crates/rustic-agent/src/harness/event_map.rs#L60-L61) as `tracing::debug!(envelope_type = other, "ignoring envelope")` and is silently dropped. Symptom: the agent says "please confirm in the UI" but no popup ever appears, then waits forever or errors.

**Scope:**

1. **Audit the full envelope surface** that Claude Code emits in `--print --input-format stream-json --output-format stream-json` mode. The leaked source at [references/claude_code_structure/.../src/tools/](references/claude_code_structure/claude-code-main/claude-code-main/src/tools/) lists every interactive tool — at minimum `AskUserQuestionTool`, `ExitPlanModeTool`. For each, figure out what envelope shape it emits to the host.
2. **Same audit for Codex.** Codex emits `request_user_input`, MCP elicitation, and similar — [codex.rs:578](crates/rustic-agent/src/harness/codex.rs#L578) already references "MCP elicitation, free-form question, etc." as something the existing path doesn't handle.
3. **Extend `HarnessEvent` enum** with whatever new variants are needed. Likely additions:
   - `HarnessEvent::ApprovalRequest { request_id, kind: "exit_plan_mode" | "tool_run" | "...", payload }` for approval-required tool calls
   - `HarnessEvent::McpElicit { request_id, message, schema }` for MCP-driven prompts
   - Generic fallback `HarnessEvent::UnknownPrompt { envelope_json }` so future envelopes don't get silently dropped — surfaces them as a "the agent is asking something we don't know how to render — here's the raw text" dialog while we add proper support.
4. **Wire each new event type** through [harness_runtime.rs](src-tauri/src/commands/agent/harness_runtime.rs) into a Tauri event the frontend renders.
5. **Frontend dialog audit.** Make sure the chat view subscribes to ALL these events and renders the right dialog. The permission card and question dialog exist; we likely need new components for approval (exit-plan-mode), MCP elicit, and the generic fallback.
6. **Convert "unknown envelope" from silent drop to visible warning.** Replace the `tracing::debug!` at [event_map.rs:60](crates/rustic-agent/src/harness/event_map.rs#L60) with a `tracing::warn!` that also emits a Tauri event the user sees as "harness emitted an unhandled message: …" — so future regressions surface fast instead of silently.

**Why.** Today the agent in harness mode can issue prompts that vanish. The user sees "please confirm in the UI" in chat with nowhere to click. With P0.2 (`ask_user` for native) we add this capability for native mode; P0.9 makes the same UX work in harness mode. Without P0.9, harness mode has a strictly worse interactive story than native.

**Where.**
- [crates/rustic-agent/src/harness/event_map.rs](crates/rustic-agent/src/harness/event_map.rs) — new translators for each envelope type. Audit drives the list.
- [crates/rustic-agent/src/harness/event_map_codex.rs](crates/rustic-agent/src/harness/event_map_codex.rs) — equivalents for Codex.
- [crates/rustic-agent/src/harness/mod.rs](crates/rustic-agent/src/harness/mod.rs) — extend `HarnessEvent` enum + `PermissionDecision` (or a new response type for non-permission prompts).
- [src-tauri/src/commands/agent/harness_runtime.rs](src-tauri/src/commands/agent/harness_runtime.rs) — emit new Tauri events; add response handlers (`respond_to_approval`, `respond_to_elicit`, etc.).
- Frontend dialog components — extend / add as needed.

**Effort.** ~2 days (1 day audit + envelope wiring + 1 day frontend dialogs + end-to-end test of at least 3 scenarios: permission, AskUserQuestion, exit_plan_mode).

---

### P0 audit notes (2026-05-14)

Full audit run against the working tree. Every item passes verification:

- **P0.1** — [executor.rs:713-993](crates/rustic-agent/src/task/executor.rs#L713-L993). `MAX_STREAM_ATTEMPTS=4`, backoffs `[0, 30_000, 60_000]`, 30s stall watchdog, `TaskEvent::StreamRetry { attempt, waiting_ms }` emission. No fallback model.
- **P0.2** — Backend tool + broker ([ask_user.rs](crates/rustic-agent/src/tools/ask_user.rs), [ask_user_broker.rs](crates/rustic-agent/src/task/ask_user_broker.rs)) and frontend tabbed dialog ([chat-view.js:5718-6731](src/components/agent/chat-view.js#L5718-L6731)) wired end-to-end.
- **P0.3** — `is_plan_mode` boolean threaded through `ToolContext` ([tools/mod.rs:224](crates/rustic-agent/src/tools/mod.rs#L224)), executor tool-partition ([executor.rs:1332-1372](crates/rustic-agent/src/task/executor.rs#L1332-L1372)), system-prompt addendum at [system_prompt.rs:776](crates/rustic-agent/src/system_prompt.rs#L776), Tauri command + UI toggle present.
- **P0.4** — [budget.rs](crates/rustic-agent/src/budget.rs) (Arc<Semaphore> for streams, AtomicU64 for spend, midnight rollover) + [ceiling_broker.rs](crates/rustic-agent/src/task/ceiling_broker.rs) (`TaskEvent::CeilingBreached` → UI pause flow) + [budget-settings.js](src/components/settings/budget-settings.js).
- **P0.5** — `EDIT_NO_MATCH` distinct from `STALE_READ` ([file_ops.rs:840-853](crates/rustic-agent/src/tools/file_ops.rs#L840-L853)), whitespace-tolerant fallback with `WHITESPACE_NORMALIZED` warning, top-3 candidates with 8KB cap. Tests in `p0_5_match_tests`.
- **P0.6** — File-tree hoisted out of system prompt into a first-message `<project_structure>` block ([system_prompt.rs:481-515](crates/rustic-agent/src/system_prompt.rs#L481-L515)), `cache_control: ephemeral` on stable prefix breakpoints ([provider/claude.rs:769-825](crates/rustic-agent/src/provider/claude.rs#L769-L825)). Investigation captured in [docs/perf_findings.md](docs/perf_findings.md).
- **P0.7** — `PREFER read_file` line in [system_prompt.rs:200-207](crates/rustic-agent/src/system_prompt.rs#L200-L207); shell-read detection + soft-warn prepend in [terminal.rs:227-304](crates/rustic-agent/src/tools/terminal.rs#L227-L304). Tests in `p0_7_shell_read_detector`.
- **P0.8** — `SessionReady { model, auth_mode }` capture from `system:init` ([event_map.rs:300-314](crates/rustic-agent/src/harness/event_map.rs#L300-L314)); cost-resolution priority CLI→recompute-CLI-model→recompute-user-model ([harness_runtime.rs:910-925](src-tauri/src/commands/agent/harness_runtime.rs#L910-L925)); `sum_model_usage_cost` ([event_map.rs:486](crates/rustic-agent/src/harness/event_map.rs#L486)) sums `result.modelUsage[*].costUSD` when `total_cost_usd` is absent (test in `result_envelope_falls_back_to_modelusage_sum_when_total_cost_missing`); `auth_mode` forwarded to frontend ([harness_runtime.rs:971](src-tauri/src/commands/agent/harness_runtime.rs#L971) → [agent.js:429](src/state/agent.js#L429) as `costAuthMode`).
- **P0.9** — `HarnessEvent::{ApprovalRequest, McpElicit, UnknownPrompt}` variants in [harness/mod.rs:244-289](crates/rustic-agent/src/harness/mod.rs#L244-L289); unknown-envelope catch-all replaces silent `debug!` drop in both [event_map.rs](crates/rustic-agent/src/harness/event_map.rs) and [event_map_codex.rs](crates/rustic-agent/src/harness/event_map_codex.rs); three matching frontend dialogs in chat-view.js with response paths through `respond_to_permission` / `respondToUnknownPrompt`.

**Known non-blocking residuals (carry forward, not P0 blockers):**

1. Codex `SessionReady` always emits `model: None, auth_mode: None` — Codex's stream-json protocol doesn't expose those the way Claude Code's `system:init` does, so cost falls back to the user-picked model. Acceptable; revisit if Codex adds the fields.
2. Codex-specific `request_user_input` / MCP-elicitation envelopes route through the generic `UnknownPrompt` fallback rather than typed variants. Functional (user still sees the dialog) but not optimal — add typed handlers when we next touch Codex envelope translation.

---

## Tier B — Live UX bugs and fixes (from current daily use) — ✅ COMPLETED 2026-05-14

All 6 items shipped. Audit notes at the bottom of this section.

Things that hurt every day. Small effort each, ship alongside P0.

### B.1 — Terminal: scroll to bottom on open + cap scrollback

**What.** Two related fixes:
1. When opening any terminal panel (especially one with a running server), scroll position must land at the **bottom** (latest output), not the top.
2. Scrollback buffer is currently unbounded — long-running servers cause UI lag. Cap at **N lines** (start with 10,000) with FIFO eviction of oldest lines.

**Where.** Terminal UI component in `src/components/` and the PTY buffer in [rustic-terminal/src/pty.rs](crates/rustic-terminal/src/pty.rs) — likely needs a ring-buffer or `VecDeque<Line>` with a hard cap.

**Effort.** ~half day.

---

### B.2 — Task panel: open at the last message, not the first

**What.** When the user clicks into any task (running or not), the scroll position should be at the **last message**, not the first. Currently it always starts at the top.

**Where.** Task view component in `src/components/`. Scroll the message list to `scrollHeight` on mount and on new-message events.

**Effort.** ~quarter day.

---

### B.3 — Checkpoint revert restores attachments alongside text

**What.** When the user reverts to a checkpoint, the input box gets the text back but **attachments are dropped**. Both need to be restored.

**Where.** Wherever the revert-to-checkpoint logic lives (likely a Tauri command in `src-tauri/src/commands/agent/`). Need to (a) include attachments in the checkpoint snapshot and (b) re-attach them when restoring.

**Effort.** ~half day.

---

### B.4 — Remove "pasted text as attachment" behaviour

**What.** Right now pasting a large text block converts it to a "Pasted text" attachment chip. Remove that — paste inline as normal text. The existing "show more / show less" feature already handles long text gracefully in chat bubbles, so no information is lost.

**Why.** This behaviour misbehaves with our clients and adds friction. Plain paste is what users expect.

**Where.** Input box paste handler in `src/components/`.

**Effort.** ~quarter day.

---

### B.5 — Harness mode (Claude Code): slash commands don't fire

**What.** In Claude Code harness mode, typing `/` shows the slash-command popup correctly, but pressing Enter sends the input as **normal text** instead of forwarding it as a slash command to the spawned `claude` binary. Fix the input handler to detect slash-command mode and route to the right path in the harness stdin protocol.

**Where.** [crates/rustic-agent/src/harness/claude_code.rs](crates/rustic-agent/src/harness/claude_code.rs) for the stdin protocol; input handler in the frontend chat component for the Enter routing.

**Investigation needed.** Confirm how the spawned `claude` CLI expects slash commands over its `stream-json` stdin — they may need a different envelope type than user messages.

**Effort.** ~1 day (half day investigation + half day fix).

---

### B.6 — Terminal: paste duplicates

**What.** Pasting any text into a terminal pane pastes it twice. Likely two event handlers (browser-level + custom) both consuming the paste event. Dedupe.

**Where.** Terminal component paste handler. Inspect event listeners and find which one is the duplicate.

**Effort.** ~half day.

---

### Tier B audit notes (2026-05-14)

- **B.1** — [terminal-pane.js:140](src/components/terminal/terminal-pane.js#L140) sets `scrollback: 10000` on the xterm `Terminal` constructor; [terminal-pane.js:335](src/components/terminal/terminal-pane.js#L335) and [terminal-pane.js:345](src/components/terminal/terminal-pane.js#L345) call `terminal.scrollToBottom()` after both first-open replay/fit and on re-show. PTY-side byte buffer was already capped at 128 KB in [pty.rs:16](crates/rustic-terminal/src/pty.rs#L16); no change there.
- **B.2** — [chat-view.js:6642-6646](src/components/agent/chat-view.js#L6642-L6646) now scrolls cached fragments to `scrollHeight` unconditionally. The uncached path was already handled by `pendingTaskSwitchScroll = 'bottom'` flowing through `renderMessages`. The dead `switchingTask` local was removed.
- **B.3** — [chat-view.js:1018](src/components/agent/chat-view.js#L1018) — `handlePerMessageRevertClick` now accepts the full `msg`. After the text mirror, image blocks (`{ type: 'image', media_type, data }`) get mapped into the composer's `attachedFiles` shape and `renderAttachmentPills()` is called. Workflow + pasted-text chips already round-trip via the message body parsing path.
- **B.4** — [chat-view.js:3538](src/components/agent/chat-view.js#L3538) — paste handler dropped the 800-char chip branch. Plain text paste now flows through the default `<textarea>` behavior; image paste still routes to `attachedFiles`. Historical messages containing `<pasted-text id="...">` markers still render via the existing extractor at [chat-view.js:5025](src/components/agent/chat-view.js#L5025).
- **B.5** — Investigation: Claude Code's `stream-json` headless mode does not process slash commands at any envelope type (REPL-only). Fix path chosen: client-side expansion. New Tauri command [`get_claude_code_slash_command_body`](src-tauri/src/commands/agent/harness_slash.rs) reads the markdown body (with YAML frontmatter stripped) for `User` and `Project` commands. Frontend filters `Builtin` commands out of the picker ([chat-view.js:3611](src/components/agent/chat-view.js#L3611)) and inlines the body on selection ([chat-view.js:3797](src/components/agent/chat-view.js#L3797)). Built-ins like `/clear` / `/compact` / `/model` are no longer offered in harness mode because the host has no way to fire them.
- **B.6** — [terminal-pane.js:153](src/components/terminal/terminal-pane.js#L153) — removed the custom `Ctrl+V` keydown handler. xterm's native paste listener on its hidden textarea already routes paste through `onData → api.writeTerminal`; the extra handler was double-writing. The right-click "Paste" path is untouched (no `paste` event, no duplication).

---

## Tier P1 — High value, medium effort

These deliver real new capability and need design work but are bounded.

### P1.1 — Tree-sitter integration

**What.** Embed `tree-sitter` plus per-language grammar crates. Parse every code file in every open project into an AST. Trees live in a shared LRU cache; invalidated on file save and on file-watcher events.

**Stack.**
- `tree-sitter = "0.22"`
- Grammars: `tree-sitter-rust`, `tree-sitter-typescript`, `tree-sitter-go`, `tree-sitter-python`, `tree-sitter-javascript`, `tree-sitter-html`, `tree-sitter-css`, `tree-sitter-bash`, `tree-sitter-markdown`
- Parser pool — `Arc<Mutex<HashMap<Language, Vec<Parser>>>>` so we don't re-create parsers per file
- Tree cache — `Arc<DashMap<PathBuf, (mtime, Tree)>>` with LRU eviction at ~500 cached trees

**Why.** Tree-sitter is what gives Rustic structured code intelligence without LSP's RAM cost. Powers file outlines (UI panel), symbol locations (for the index in P1.2), exact-bound reads of function bodies (for `batch_edit`), and language-aware syntax highlighting in the editor panes. Cost: ~80 MB total for 9 grammars loaded; trees are cheap.

**Where.** New crate `crates/rustic-treesitter/` (cleanly separable, may be useful to other crates). Consumed by `WorkspaceServices` (P1.3).

**Effort.** ~2 days.

---

### P1.2 — Workspace symbol index

**What.** A `HashMap<SymbolName, Vec<SymbolEntry>>` per project, built from tree-sitter queries. Entries: `(file, line, col, kind, scope)`. Built at project open by running symbol queries across all source files. Refreshed incrementally on file save via the `notify` crate.

**Tools exposed to the agent (new):**
- `find_symbol(name, kind?)` — workspace-wide symbol lookup, returns candidates
- `goto_definition(file, line, col)` — name-resolution-only (tells agent it's not type-aware in the description)
- `find_references(name)` — name-match, returns candidate list with caveat
- `outline(file)` — full file structure: classes, functions, methods, nesting
- `call_sites(name)` — every call expression with that callee identifier

**Storage decision.** **In-memory only for now.** Re-indexed on every Rustic startup. 30–90 second warm-up across 6 projects is tolerable. If usage data later shows users opening + closing Rustic often, add SQLite persistence (P2.7) as a non-breaking change.

**Vendoring.** Query files (`.scm`) borrowed from [nvim-treesitter](https://github.com/nvim-treesitter/nvim-treesitter) (MIT licensed) instead of writing from scratch. Saves ~1 day per language.

**Where.** New module `crates/rustic-agent/src/index/`. Tool definitions in `crates/rustic-agent/src/tools/code_intel.rs`.

**Effort.** ~3–4 days.

---

### P1.3 — `WorkspaceServices` abstraction

**What.** A per-opened-project `Arc<WorkspaceServices>` that owns tree-sitter parsers, the symbol index, the file watcher, and (later) any other cross-task per-project state. Tasks borrow `Arc<WorkspaceServices>` instead of holding private copies.

**Why.** This is the architectural slot that makes our concurrent-task USP work without 4× RAM. With 3–4 tasks in the same project, one `WorkspaceServices` is shared. With tasks in different projects, each gets its own. CLAURST doesn't have this because it's single-task.

**Where.** New `crates/rustic-agent/src/workspace.rs`. Thread `Arc<WorkspaceServices>` through `ToolContext` ([tools/mod.rs:241](crates/rustic-agent/src/tools/mod.rs#L241)). Each `TaskExecutor` gets handed one on construction.

**Effort.** ~1 day for the skeleton + refactor; P1.1 and P1.2 then plug into it.

---

### P1.4 — Worktree tool (`enter_worktree` / `exit_worktree`)

**What.** A tool that creates a git worktree (separate working directory pointing at the same `.git`) for a sub-task. Lets parallel tasks edit the same project without stepping on each other.

**Why.** With concurrent tasks as our USP, file conflicts between tasks are the lurking bug. Worktrees are the proven escape hatch. We already have `rustic-git` so the plumbing exists. Each worktree mounts its own child `WorkspaceServices`.

**Where.** New tool in `crates/rustic-agent/src/tools/worktree.rs`. Uses [`rustic-git`](crates/rustic-git/) to invoke `git worktree add/remove`. UI shows worktree status in the task panel.

**Effort.** ~2 days.

---

### P1.5 — Batch edit (extend existing edit_file first)

**What.** Apply N edits across M files in a single tool call, atomically (all or nothing). Input shape:

```json
{
  "edits": [
    { "file_path": "src/a.rs", "old_string": "...", "new_string": "..." },
    { "file_path": "src/b.rs", "old_string": "...", "new_string": "..." }
  ]
}
```

**Approach.** First try to **extend the existing `edit_file` tool** to accept either the current single-edit shape *or* an `edits: []` array. Backwards-compatible, no new tool name. If schema constraints in any provider make that ugly, fall back to a separate `batch_edit` tool.

**Why.** Today an 8-file rename = 8 round-trips to the model. Each round-trip is hundreds of milliseconds + tokens. One tool call = one round-trip, dramatically cheaper.

**Where.** [crates/rustic-agent/src/tools/file_ops.rs](crates/rustic-agent/src/tools/file_ops.rs). Rollback uses the existing `file_history` blob store to revert if any edit in the batch fails validation.

**Effort.** ~1 day.

---

### P1.6 — Sub-agent observation + control

**What.** Round out the orchestrator/sub-agent system with four new tools the parent agent can call:

- `send_message(subagent_id, content)` — push a message into the sub-agent's inbox; the sub-agent consumes it on next turn boundary
- `list_subagents()` — current status, last action, turn count, cumulative cost, **model in use** (see P1.10)
- `stop_subagent(subagent_id, reason)` — graceful cancellation
- `nudge_subagent(subagent_id, hint)` — inject a system-level steering message mid-execution (e.g. "stop reading files, just summarize")

**Where.** Extend [task/subagent.rs](crates/rustic-agent/src/task/subagent.rs) and [task/orchestrator_host.rs](crates/rustic-agent/src/task/orchestrator_host.rs) with an inbox per sub-agent, a status snapshot reader, and a cancellation token registry. Tools in `crates/rustic-agent/src/tools/subagent_tools.rs` (extend the existing one).

**Effort.** ~2 days.

---

### P1.7 — Tool search (deferred tool schemas)

**What.** Keep a small core tool set always visible in the system prompt (the 8–10 most-used: `read_file`, `edit_file`, `grep_search`, `glob`, `run_command`, `web_search`, `todo_write`, `ask_user`). Every other tool's schema is **deferred** — its name and one-line description are listed in the system prompt, but the full JSON schema is fetched on demand via a `tool_search` meta-tool.

**Why.** As we add P1 tools (worktree, batch_edit, sub-agent controls, code-intel x5, document reading, formatter, etc.), the prompt prefix grows by thousands of tokens that get cache-written on every call. Tool search keeps the prefix lean. Claude Code and CLAURST both do this.

**Where.** Change how tool definitions are built in [executor.rs:55](crates/rustic-agent/src/task/executor.rs#L55) — split into "always-on" and "deferred" pools. Add `tool_search` as a built-in.

**Effort.** ~2 days.

---

### P1.8 — Goal loop (`/goal`)

**What.** A multi-turn objective mode. User sets a goal ("get the test suite green", "implement feature X"); agent keeps running turns until it calls `goal_complete` tool or hits a max-iteration cap. Different from a normal turn which ends on `end_turn`.

**Why.** Perfect fit for our multi-task workflow — kick off 3 goal-loops in 3 projects, come back in 20 minutes.

**Where.** New `crates/rustic-agent/src/task/goal_loop.rs`. Wraps `TaskExecutor::run_turn` with a continuation predicate. New `goal_complete` tool. UI gets a "goal mode" task type with progress indicator.

**Effort.** ~2 days.

---

### P1.9 — Async sub-agent completion (remove `wait_for_subagents`)

**What.** Replace the synchronous `wait_for_subagents` tool with an event-driven flow:

1. `spawn_subagent` returns immediately with a subagent_id. No blocking.
2. Main agent continues working on whatever else it wants.
3. **When a sub-agent completes**, the framework prepends a system-style message to the main agent's next turn: `[Sub-agent <id> finished. Result: ...]` — automatic, no tool call needed.
4. If the main agent has nothing else to do and calls `end_turn` while sub-agents are still running, the task is **parked** (not ended) and resumed automatically as soon as the next sub-agent completes.
5. Main agent can always proactively check via `list_subagents` (P1.6) but doesn't have to.

**Why.** The current `wait_for_subagents` tool forces the main agent to block on a fixed wait, wasting tokens and turns. Async notification lets the main agent run other useful work in parallel and resume only when there's something new to react to.

**System prompt change.** Add to main-agent prompt: *"Sub-agents run asynchronously. After spawning, continue with other work; you'll be notified automatically when each completes. If you have nothing useful to do until then, just end your turn — you'll be woken when results arrive."*

**Where.**
- Remove `wait_for_subagents` from [tools/subagent_tools.rs](crates/rustic-agent/src/tools/subagent_tools.rs)
- Modify [task/executor.rs](crates/rustic-agent/src/task/executor.rs) to add a "parked, awaiting subagent completion" state when `end_turn` happens with live sub-agents
- Wire sub-agent completion event ([task/subagent.rs](crates/rustic-agent/src/task/subagent.rs) — `SubagentCompletionEvent` exists) to inject a synthetic system message and resume the parked task
- Update system prompts in [system_prompt.rs](crates/rustic-agent/src/system_prompt.rs)

**Effort.** ~2 days.

---

### P1.10 — Sub-agent model badge in UI

**What.** When the main agent spawns a sub-agent, the UI shows which model the sub-agent is using — either the "cheap" tier or the "intelligent" tier, and ideally the exact model name (e.g. "Haiku 4.5" or "Sonnet 4.6"). Shown as a small badge on the sub-agent's task card.

**Why.** Today it's invisible which tier is running. Useful for debugging cost and quality issues.

**Where.** Sub-agent task event emission in [task/subagent.rs](crates/rustic-agent/src/task/subagent.rs) — add the model name to `SubagentResult` and the in-flight status. UI: small badge in the sub-agent card component.

**Effort.** ~quarter day.

---

### P1.11 — Redesign `read_file` around Claude Code's architecture + format expansion

**Framing.** This is a **redesign**, not an extension. We rebuild `read_file` on Claude Code's cap architecture (format-agnostic byte + token caps, throw-don't-truncate on explicit-range overflow, discriminated-union output type), then layer our own wins on top, and PDF/DOCX/XLSX/notebook support falls out as a consequence. Claude Code's design is the only one of the three that's been **empirically validated** ([limits.ts:9-13](references/claude_code_structure/claude-code-main/claude-code-main/src/tools/FileReadTool/limits.ts#L9-L13) documents an A/B test of truncate-vs-throw and the revert).

**Positioning.** PDF + notebook reads = **parity with Claude Code** (they have them fully implemented at [src/tools/FileReadTool/](references/claude_code_structure/claude-code-main/claude-code-main/src/tools/FileReadTool/)). DOCX + XLSX = **ahead of both Claude Code and CLAURST** — neither reads Word or Excel files (grep across both: zero parser hits).

**Adopt from Claude Code:**

1. **Two-layer cap (byte + token), format-agnostic:**
   - `maxSizeBytes = 256 KB` — pre-read stat check. Replaces our line-cap as the first gate. Works for any format.
   - `maxTokens = 25,000` — post-read token count. The actual cost gate.
2. **Throw-don't-truncate on explicit-range overflow.** If the agent passes an explicit range that exceeds the cap, return a ~100-byte error pointing to a smaller range. Their A/B test showed truncation silently spent 25K tokens per overflow vs ~100 bytes for the error — we don't need to re-run the experiment.
3. **Discriminated-union output type** (`text` | `image` | `notebook` | `pdf` | `parts`). Replaces our current text-only string return. Needed anyway once binary formats land; doing it now avoids a second refactor.
4. **Param names `offset` / `limit`** — switch from `start_line` / `end_line`. Their names work for any format (line offset for text, byte offset for binary). Cross-tool consistency.
5. **Env var override:** `RUSTIC_FILE_READ_MAX_OUTPUT_TOKENS` for power users (matches Claude Code's `CLAUDE_CODE_FILE_READ_MAX_OUTPUT_TOKENS`).
6. **PDF size-threshold fallback:** `PDF_EXTRACT_SIZE_THRESHOLD = 3 MB`. Below: send as base64 document block (Anthropic API handles natively). Above: **extract each page as an image** and send those — critical for scanned PDFs where text extraction yields garbage. Hard ceiling `PDF_MAX_EXTRACT_SIZE = 100 MB`, reject above with a clean error.
7. **`pages` param format** — `"1-5"`, `"3"`, `"10-20"`. Use Claude Code's `PDF_MAX_PAGES_PER_READ = 20` cap.
8. **Multimodal placeholder for images** — `"[Image file: X. The image content has been captured for visual analysis.]"`. Also borrowed from CLAURST. Signals to multimodal models that bytes are attached.

**Keep from our existing design (Claude Code lacks these):**

- **`FILE_UNCHANGED` stub on re-reads.** Hash `(path, mtime, offset, limit)` and short-circuit. Layer on top of the two-cap system. **Better than Claude Code** — keep it and extend the hash key to include format-specific range params for PDF/DOCX/XLSX.
- **`STALE_READ` recovery context** when an `edit_file` fails because content changed (±150 lines around hint, ≤8 KB). Claude Code handles edit recovery via its own `FileEditTool` flow; ours lives in the read path. Check for overlap during the refactor; probably keep both.

**Per-format defaults (after the two-cap system applies):**

| Format | Range param | Format-specific cap (applies before global 25K-token cap) |
|---|---|---|
| **Text** | `offset` / `limit` | none — relies on 256 KB / 25K-token caps |
| **PDF** | `pages` (e.g. `"1-5"`) | `PDF_MAX_PAGES_PER_READ = 20` |
| **DOCX** | `paragraph_range` (e.g. `"1-200"`) | 2000 paragraphs |
| **XLSX** | `sheet` (name/index), `rows` (e.g. `"1-1000"`) | first sheet, 500 rows |
| **Notebook** | `cells` (e.g. `"1-10"`) | none — relies on token cap |
| **Image** | — | base64 with media type + dimensions |

**Skip:** Legacy `.doc` / `.xls` (binary OLE) — return clean error: "binary OLE format unsupported, convert to .docx/.xlsx first."

**Breaking change.** The param rename `start_line` / `end_line` → `offset` / `limit` is a tool-surface change. New turns get the new schema automatically (fresh tool defs per turn). Historic tool calls in saved task transcripts keep the old names — they're frozen records, not re-executed. No backwards-compat shim needed.

**Where.** [crates/rustic-agent/src/tools/file_ops.rs](crates/rustic-agent/src/tools/file_ops.rs) — substantial rewrite of `execute_read_file` and `read_file` tool definition. Detect file type by extension first, magic-bytes second.

**Effort.** ~4–5 days:
- Cap architecture (line → bytes + tokens): ~0.5 day
- Discriminated-union output type: ~0.5 day
- Param rename + system-prompt update: ~0.5 day
- Throw-don't-truncate path: ~0.25 day
- PDF (text extraction + image-fallback for big/scanned): ~1 day
- Notebook (.ipynb structured cells): ~0.5 day
- DOCX: ~0.5 day
- XLSX: ~0.5 day
- Preserve `FILE_UNCHANGED` + `STALE_READ` on top: ~0.5 day
- Tests + dogfood validation: ~0.5 day

---

### P1.12 — Terminal "+" project picker

**What.** When the user clicks the "+" icon to open a new terminal and **more than one project is open**, show a small popover/menu listing project names. Click a project → new terminal opens with cwd set to that project's root. If only one project is open, behaviour is unchanged (just opens a new terminal there).

**Where.** Terminal panel "+" handler in `src/components/`. Reuse project list from the existing project sidebar.

**Effort.** ~half day.

---

## Tier R — Research tasks (must do before committing big effort)

These need investigation before we know the right implementation.

### R.1 — Shadow-git vs. our `file_history` — **DECIDED 2026-05-13**

**Deliverable:** [docs/file_tracking_decision.md](docs/file_tracking_decision.md) — full comparison and rationale.

**Decision: Option C — hybrid.** Replace the storage core with **libgit2-backed shadow trees**, keep our SQLite layer of `(task_id, message_id) → tree_hash` metadata on top. Best of both: git's battle-tested storage internals (delta-compressed packfiles, symlinks/CRLF/case handling, gitignore, reachability-based GC) with our agent-aware per-task/per-message scoping preserved.

**Why hybrid over pure shadow-git or pure custom:**
- Most current bugs likely live in core mechanics (symlinks, blob/index consistency, GC) — these disappear by construction with libgit2.
- Disk usage drops ~10–50× for typical edit patterns (a 1 MB file edited 50× goes from ~50 MB to ~1.5 MB).
- libgit2 (vs CLAURST's `git` subprocess) keeps ops in-process — function calls, not 10–50 ms spawns.
- Per-task/per-message metadata is the part of our system that *isn't* broken — keep it.

**Locked-in design choices:**

- **Storage location:** `{configDir}/file-history/shadow/<project_hash>/` (bare libgit2-managed repo, no commits, just tree objects).
- **Migration:** None — clean break. Pre-1.0, no production users.
- **Default excludes for non-git projects:** `target/`, `node_modules/`, `dist/`, `.next/`, `build/`, `out/`, `__pycache__/`, `.venv/`, `.cargo/`, `.gradle/`.
- **Tiered capture by file size:**

  | File size | Capture path | Latency the agent sees |
  |---|---|---|
  | ≤ 5 MiB (most edits) | Synchronous pre-write capture | ~5 ms (current behavior) |
  | 5–50 MiB (lockfiles, large bundles) | **Async** fire-and-forget on blocking thread | **~0 ms** — agent continues immediately |
  | > 50 MiB | Recorded as `TooLarge`, not tracked | ~0 ms — error returned |

  The shadow repo's Mutex serialises queued captures so subsequent ops never read stale state.

- **Retention caps (eviction triggered on new task creation):**
  - **Per-project: 5 tasks' worth of file history.** When task #6 starts in a project, oldest *closed* task in that project is pruned.
  - **Global: 100 tasks' worth of file history.** When task #101 starts, oldest *closed* task across all projects is pruned.
  - **Within a task: 100 snapshots** (existing `max_snapshots`).
  - **Active tasks are never pruned.**
  - **Disk safety net: 10 GB total cap** — packfile compression means we'll rarely hit this, but it catches pathological cases.

**Implementation effort.** ~1.5 weeks (8 working days). Day-by-day plan in `file_tracking_decision.md` §7.

**Slots into:** Week 8 of the build order (replaces the "Shadow-git outcome" placeholder).

---

### R.2 — Why is Claude Code (via our harness) faster than our native agent?

**The observation.** When using Rustic in harness mode (driving the `claude` CLI), tasks complete noticeably faster than with our native agent on the same prompt. Need to find out *exactly* why.

**Investigation plan.**

1. Pick 3 representative tasks (one small bug fix, one medium feature add, one cross-file refactor). Run each in both modes (native vs harness), instrument:
   - Total wall-clock time
   - Number of turns
   - Tokens per turn (prompt + completion)
   - Cache hit rate on the prompt prefix
   - Number of tool calls
   - Time per tool call

2. Capture the system prompt and tool definitions from both. Compare lengths and structure.

3. Look at what Claude Code does differently in their query loop ([references/claude_code_structure/claude-code-main/claude-code-main/src/query.ts](references/claude_code_structure/claude-code-main/claude-code-main/src/query.ts) and [src/QueryEngine.ts](references/claude_code_structure/claude-code-main/claude-code-main/src/QueryEngine.ts)) — particularly around:
   - Prompt caching strategy (which blocks get `cache_control: ephemeral`)
   - Tool call batching / parallelism
   - Model selection per sub-task
   - Context window management / auto-compact timing
   - System prompt length and structure

**Hypotheses to validate.**
- They cache more aggressively → fewer prompt rebuild costs
- Their system prompt is leaner → faster first-token latency
- Their tool schemas are tighter → fewer prompt tokens per turn
- They parallelise tool calls more → fewer serial round-trips
- They default to Sonnet, we default to Opus → straight model speed delta
- They have specialised flows for common patterns (search → edit → test) that compress turns

**Deliverable.** A short doc (`docs/perf_findings.md`) with the comparison, the biggest 2–3 deltas, and concrete change recommendations.

**Effort.** ~3 days.

**Follow-on.** Apply whichever wins are cheap (system prompt trim, caching strategy, parallelism) before tree-sitter work; defer architectural ones until they're justified.

---

## Tier P2 — Nice to have

Real value but not blockers. Demand-gated.

### P2.1 — Cron / scheduled tasks
CLAURST has `cron` for "run this prompt every 2h". **Effort:** ~2 days.

### P2.2 — Sleep / monitor tools
`sleep N` and `monitor <stream>` for tasks that wait on long-running external state. **Effort:** ~half day each.

### P2.3 — Notebook edit tool
Edit `.ipynb` cells structurally. **Effort:** ~1 day.

### P2.4 — REPL tool
Persistent language REPL (Python, Node) the agent can drive across calls. **Effort:** ~2 days.

### P2.5 — Formatter tool (language-aware)
Dispatches to the right binary per language: `rustfmt` for Rust, `prettier` (or `biome`) for TS/JS/CSS/HTML/JSON/MD, `ruff format` (or `black`) for Python, `gofmt` for Go, `shfmt` for shell. Returns clean error with install hint if binary missing. **Effort:** ~1 day.

### P2.6 — Managed-agents (Manager-Executor)
CLAURST's cost-reduction pattern: a cheap manager model plans, an expensive executor only runs when needed. **Effort:** ~1 week. Defer; our orchestrator + sub-agent system already covers the same need.

### P2.7 — Index persistence (SQLite)
Persist the workspace symbol index to `rustic-db` so the 30–90s startup re-index becomes ~50–200ms. Non-breaking change to P1.2. **Effort:** ~2 days. Decide based on usage telemetry.

---

## Build order (next 6–8 weeks)

**Week 1 — R.2 fixes + harness UX + cost + most-pressing reliability.**
First: **P0.5 → P0.6 → P0.7** (the R.2-derived fixes, ~3.5 days, biggest measurable productivity wins per [docs/perf_findings.md](docs/perf_findings.md)). Then **P0.9** (harness prompts vanishing, ~2 days — blocks daily harness-mode use). Then **P0.8** (harness cost tracking, ~1.5 days — prerequisite for P0.4 to actually enforce budgets in harness mode). That's the full week (~7 days of focused work). Remaining P0 items (P0.1 stream-stall, P0.2 ask_user, P0.3 plan mode, P0.4 budgets) and Tier B spill to Week 2. End of week: agent's cost/turn-count tax is gone, harness mode is fully wired (prompts + cost), R.2 fixes shipped.

**Week 2 — finish Tier B + remaining P0 spillover.**
B.3 checkpoint attachments, B.5 harness slash commands, plus any P0 that didn't fit in week 1. Research (R.1, R.2) is **already complete** — see [docs/file_tracking_decision.md](docs/file_tracking_decision.md) and [docs/perf_findings.md](docs/perf_findings.md).

**Week 3 — Tree-sitter foundation.**
P1.3 (`WorkspaceServices`), P1.1 (tree-sitter parsers + tree cache). Tree-sitter loaded and parsing every file but no tools use it yet.

**Week 4 — Symbol index + code-intel tools.**
P1.2. Agent has `find_symbol`, `goto_definition`, `find_references`, `outline`, `call_sites` — the LSP-replacement is live.

**Week 5 — Sub-agent system overhaul + concurrent-task safety.**
P1.6 (observation + control), P1.9 (async completion, remove wait_for_subagents), P1.10 (model badge), P1.4 (worktrees), P1.5 (batch edit). End of week: 3–4 tasks run safely, orchestrator can manage them actively, no wasted-token waits.

**Week 6 — Prompt hygiene + small capability wins.**
P1.7 (tool search), P1.8 (goal loop), P1.12 (terminal project picker). End of week: tool count can grow safely; long-running unattended tasks shipped; small UX win on the terminal.

**Week 7 — `read_file` redesign + format expansion.**
P1.11 in full. ~4–5 days of focused work because this became a redesign on Claude Code's architecture, not a small extension. PDF + notebook ships parity with Claude Code; DOCX + XLSX ships ahead of both Claude Code and CLAURST.

**Week 8 — Shadow-git outcome + P2 polish + buffer.**
Depending on R.1's recommendation: implement migration (3–5 days), build hybrid (2–3 days), or skip. Whatever's left in the week goes to P2 polish (formatter, sleep/monitor, index persistence) and accumulated bugs.

---

## Resolved decisions (2026-05-13)

All 5 prior open questions are now locked in.

1. **Tree-sitter query maintenance — VENDOR.** Use `.scm` query files from `nvim-treesitter` (MIT licensed), pinned to a specific commit. Write our own only when a language's nvim-treesitter queries don't cover what we need. Saves ~1 day per language and we get free upstream updates by bumping the pin.

2. **Tool search prompt structure — NAME + ONE-LINE DESCRIPTION.** Deferred tools list as `tool_name — short description` in the system prompt (~30 char descriptions). Names alone leave the agent guessing what's available; full schemas defeat the deferral purpose.

3. **Sub-agent inbox semantics — TURN-BOUNDARY DELIVERY.** `send_message` is consumed by the sub-agent at its next natural turn boundary, not as an interrupt. `nudge_subagent` is the explicit "interrupt now" escape hatch for cases that need immediacy.

4. **Async sub-agent parking timeout — 30-MINUTE CAP.** If the main agent `end_turn`s while sub-agents are still running, the task is parked for up to 30 minutes. On timeout, surface a "still waiting on N sub-agents — keep waiting or stop?" UI prompt rather than silently dropping.

5. **Document reading size limits — DEFAULTS WITH EXPLICIT OVERRIDE.** PDF: 20 pages cap (matches Claude Code's `PDF_MAX_PAGES_PER_READ`). DOCX: 2000 paragraphs / ~50KB cap. XLSX: 500 rows of first sheet. Larger reads require explicit `range`/`pages`/`rows` param. Tool description warns the agent that big documents need range-scoping.

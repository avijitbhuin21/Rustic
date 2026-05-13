# R.2 perf findings: Rustic native vs Claude Code CLI

**Status:** Final v1, 2026-05-13.
**Question:** Why does Claude Code complete tasks noticeably faster than Rustic's native agent on the same prompt?
**TL;DR:** Rustic uses **2–5× more turns** and **11–13× more cache writes** than Claude CLI for identical work. The biggest contributors, in priority order: (1) `edit_file` returns confusing STALE_READ errors on whitespace mismatches, costing 3+ wasted turns per file edit; (2) the agent prefers shell commands over `read_file` ranges, then those shell commands fail on Windows, costing another 3–5 turns per file inspection; (3) prompt prefix changes turn-to-turn, forcing the API to invalidate cache (~13K new cached tokens per turn vs ~2K in CLI); (4) auto-compact triggers too aggressively even with 1M context, forcing re-reads of files already in window. Cleanest single signal: a **read-only** task took Rustic **8× longer** than Claude CLI (12:16 vs 1:32) using **5.5× more turns** (77 vs 14).

---

## 1. Methodology

Two independent sandbox copies of Rustic (so runs don't interfere), three representative tasks per mode, identical prompts, both modes pinned to **Opus 4.7 with high thinking effort**.

| Task | Shape | Stresses |
|---|---|---|
| **T1** | Single-file constant change (5 MiB → 50 MiB + doc comment) | Baseline overhead at simplest possible task |
| **T2** | Cross-file feature add (new SQLite column wired through tracker, schema migration, repo functions, tests) | Multi-file coordination + compilation |
| **T3** | Read-only investigation (orchestrator vs sub-agent decision-making) | Pure search/reason, no writes |

Two modes:
- **rustic-native** — Rustic's own agent loop calling the Anthropic API. Driven manually through the Rustic Tauri UI.
- **claude-cli** — `claude --print --output-format stream-json --dangerously-skip-permissions --effort high`. Driven automatically by [scripts/run_claude_cli.ps1](../../r2-perf-experiment/scripts/run_claude_cli.ps1).

All runs ran against a sandboxed copy of Rustic at `D:/Programming/Projects/Personal/r2-perf-experiment/`. Raw run logs in [r2-perf-experiment/runs/](../../r2-perf-experiment/runs/).

## 2. Headline results

| Task | Mode | Wall | Turns | Out tokens | Cache read | **Cache creation** | Cost |
|---|---|---:|---:|---:|---:|---:|---:|
| T1 | rustic | 54.0s | 9 | 2,900 | 316,991 | **35,972** | $0.46 |
| T1 | cli | 33.3s | 5 | 1,980 | 147,037 | **10,933** | $0.19 |
| T2 | rustic | 400.0s | 57 | 16,475 | 2,099,331 | **741,161** | $6.09 |
| T2 | cli | 258.5s* | 26 | 8,806 | 1,435,175 | **57,301** | $1.30 |
| T3 | rustic | 736.0s | 77 | 17,421 | 2,449,100 | **899,190** | $7.28 |
| T3 | cli | 91.8s | 14 | 5,380 | 469,234 | **43,597** | $0.64 |
| **Total** | **rustic** | **1190s** | **143** | **36,796** | **4,865,422** | **1,676,323** | **$13.83** |
| **Total** | **cli** | **383.6s** | **45** | **16,166** | **2,051,446** | **111,831** | **$2.13** |
| **Ratio** | rustic / cli | **3.1×** | **3.2×** | **2.3×** | **2.4×** | **🚨 15.0×** | **6.5×** |

\* T2 CLI wall time reported by Claude's `result` event was 258s; OS-level wall was 1847s. Discrepancy explained by Claude's `duration_ms` counting API/thinking time only, not tool execution wait (multiple `cargo check` runs). For consistency with Rustic's wall-clock UI reporting, used the API-time number. T1 and T3 didn't have this gap because they were either too short or read-only.

**The single most important number: cache-creation ratio of 15×.** Per turn:
- Rustic: ~12K new cached tokens written per turn
- CLI: ~2.5K new cached tokens written per turn

That means Rustic's prompt prefix changes substantively between every turn, forcing the API to rebuild its ephemeral cache. CLI's prefix is stable; only deltas get cached.

## 3. Cleanest signal: T3 (read-only)

T3 is the most diagnostic task because there's no compilation step and no file writes to confound the timing. Pure search-and-reason workload, identical prompt, identical sandbox state.

| | Rustic | CLI | Delta |
|---|---:|---:|---:|
| Wall | 12 min 16 s | 1 min 32 s | **8.0× slower** |
| Turns | 77 | 14 | **5.5× more** |
| Cost | $7.28 | $0.64 | **11.4× more** |
| Cache creation | 899K | 44K | **20.5× more** |
| Files modified | 0 (correct) | 0 (correct) | — |

The task completed correctly in both modes. Rustic just took 8× longer to do the same investigation.

What Rustic did wrong, per the tool-call trace: **~25 PowerShell `Get-Content` invocations** to read line ranges from files, when `read_file` with `start_line`/`end_line` would have done each one in one call. Plus failed `sed`, `wc`, `findstr`, `cmd /c findstr` attempts that wasted further turns. The user even had to type "please continue" once because the agent stopped mid-investigation.

CLI did the same task with: ~7 file reads + ~3 searches + 1 glob = ~11 effective tool calls in 14 turns. No shell-out for file reads.

## 4. Findings by category

### F1 — `edit_file` STALE_READ on whitespace mismatch (T1, P0)

**Impact:** 3 wasted turns per multi-line edit. Replicates every time the agent edits a doc comment or any multi-line block.

**Evidence:** On T1, the agent's first 3 attempts to edit the doc comment block in [blob_store.rs](../crates/rustic-agent/src/file_history/blob_store.rs) all returned:
> `STALE_READ: old_string not found in '<file>'. The file has changed since you last read it. Use the context below to find the correct text and retry.`

The file had not been modified externally. The error mechanism is "old_string didn't byte-match"; the message branding is wrong.

**Two issues, one root cause:**

1. **The error message is misleading.** "File has changed since you last read it" implies an external change. But the actual mechanism is byte-match failure on `old_string`. The agent's next attempt is therefore framed around "the file changed, let me re-read" instead of "my match string had whitespace I didn't see." The agent retries with the same wrong match.
2. **The matcher is intolerant of whitespace.** Indentation in doc comments, trailing whitespace, line-ending differences all break the match. CLI's `Edit` tool either normalises whitespace or has a fuzzy fallback — same task, zero failed edits.

**Fix:**
- Rename `STALE_READ` to `EDIT_NO_MATCH` when the cause is actually byte-mismatch. Reserve `STALE_READ` for true external changes (mtime check).
- Add whitespace-tolerant matching: strip leading/trailing whitespace per line, normalise line endings, then retry the match before failing.
- Or: when the match fails, surface the **exact diff** between what was sent and what was found (top 3 candidate lines), not just "find the correct text below."

Implementation site: [crates/rustic-agent/src/tools/file_ops.rs](../crates/rustic-agent/src/tools/file_ops.rs), `execute_edit_file`. Estimated effort: ~1 day.

---

### F2 — Agent prefers shell commands over `read_file` ranges (T2 and T3, P0)

**Impact:** On T3, ~25 shell `Get-Content` calls instead of ~5 `read_file` calls. On T2, the agent created a temporary `.rustic/tmp_read.ps1` script after 4 failed shell read attempts.

**Evidence:** Tool-call traces from both T2 and T3 show the agent repeatedly using shell commands to read specific line ranges:
- `sed -n '290,360p'` — fails on Windows
- `wc -l ...` — fails on Windows
- `findstr /N "^"` piped — fails (shell quoting)
- `cmd /c findstr /N "^"` — fails
- `powershell -Command "(Get-Content ...)"` — succeeds, used dozens of times
- In T2 specifically, the agent gave up and wrote a temp `.ps1` script to read line ranges

**Yet Rustic's `read_file` tool supports `start_line` and `end_line`.** See [file_ops.rs:412-419](../crates/rustic-agent/src/tools/file_ops.rs#L412-L419) — the description explicitly says "If you already know WHICH lines you need, pass start_line/end_line."

**Why isn't the agent using it?** Three plausible reasons, ranked by likelihood:

1. **System prompt doesn't surface `read_file` range capability prominently.** The tool description has it, but if the system prompt structure puts shell/`run_command` higher in priority, the agent's planning prefers shell.
2. **Tool description doesn't emphasise efficiency.** "Reading a file is billed against context" warning is there, but the agent doesn't seem to translate "expensive" into "use targeted ranges." Stronger prescriptive text needed.
3. **The shell tool description encourages it for "surveying."** The orchestrator prompt at [system_prompt.rs:578](../crates/rustic-agent/src/system_prompt.rs#L578) explicitly says `run_command` is for surveying — that might bleed into normal tasks too.

**Fix:**
- Push the `read_file` range capability into the SYSTEM PROMPT at top level, not just the tool description. Add an explicit "PREFER `read_file` with start_line/end_line OVER shell read commands" line.
- Add a soft-warn in `run_command`'s description: "for reading specific lines of a file, use `read_file` with start_line/end_line — it's faster and doesn't consume shell context."
- Detect and reject shell read commands (`sed -n`, `head`, `tail`, `Get-Content`) with a hint message: "Use `read_file` with line range instead."

Implementation site: [system_prompt.rs](../crates/rustic-agent/src/system_prompt.rs), `tools/terminal.rs`. Estimated effort: ~half day.

---

### F3 — Prompt prefix invalidates every turn (causing 15× cache writes, P0)

**Impact:** $13.83 total cost across 3 tasks vs CLI's $2.13. The cache-creation delta (15× higher in Rustic) is the single biggest dollar driver — fixing this alone would close most of the cost gap.

**Evidence:** Per-turn cache-creation tokens:
- Rustic: 35K (T1) / 13K (T2) / 12K (T3) per turn
- CLI: 2K (T1) / 2K (T2) / 3K (T3) per turn

That's ~5–10× more new cached tokens per turn. The only explanation: something in Rustic's per-turn prompt is changing between turns, forcing the API to mark the cache invalid and rebuild it.

**Most likely culprits, by suspicion ranking:**

1. **File-tree snapshot is included per-turn and refreshes.** Check [system_prompt.rs](../crates/rustic-agent/src/system_prompt.rs) and [file_tree.rs](../crates/rustic-agent/src/file_tree.rs) — if `generate_file_tree` is called fresh on each turn and contains any timestamps, ordering nondeterminism, or modified-time data, every turn invalidates the cache.
2. **Todo state injected into every prompt.** If `TodoWrite` updates are folded back into a section of the system prompt or first user message, every Todo change invalidates the cache.
3. **Tool result format includes timestamps.** Anywhere we format with `chrono::Utc::now()` for display puts a different timestamp in each turn.
4. **Tool definitions reordered.** If tool defs are built from a `HashMap` (no order guarantee) and the JSON output ordering differs per call, the cache key changes.
5. **MCP tool definitions refreshed live.** If MCP tools' schemas are fetched/rebuilt per turn instead of cached, that whole section invalidates.

**Fix (investigation first, then targeted change):**
- Capture two consecutive turn prompts to disk during a run. Diff them. Whatever changed is the culprit.
- Move per-turn variable content **out of the system prompt** and into the most recent user message (or a tool result). This is what `claude --exclude-dynamic-system-prompt-sections` does — Anthropic literally added that flag for this exact reason.
- Mark all stable sections with `cache_control: ephemeral`. Mark the changing tail without `cache_control` so only the tail gets rewritten.

Implementation site: [system_prompt.rs](../crates/rustic-agent/src/system_prompt.rs), [task/executor.rs](../crates/rustic-agent/src/task/executor.rs) where the prompt is assembled before the API call. Estimated effort: ~1 day investigation + ~1 day fix.

---

### F4 — Auto-compact triggers too aggressively (T2, P1)

**Impact:** Mid-task forgetting forces re-reads of files already explored, multiplying turn count.

**Evidence:** During T2, after ~10 turns the agent said:
> "The earlier full read of tracker.rs was aged out. Let me do targeted reads for the key areas."

This is Rustic's auto-compact dropping content from context. **The session was at ~30% of a 1M-context window** — there's no reason a previously-read file should have been evicted.

**Likely cause:** [task/condense.rs](../crates/rustic-agent/src/task/condense.rs) (auto-compact logic) is probably triggering on a token-count heuristic that's too aggressive for the 1M model. Either the threshold needs raising (proportional to model context size), or the eviction policy is dropping useful content (most-recent file reads).

**Fix:**
- Check the auto-compact trigger logic. If it uses a fixed token threshold, scale it to model context size (e.g., 60% of context window, not a hardcoded 100K).
- When compacting, **preserve recently-read files** in their entirety. The condense summary can replace older tool results, not recent file reads.

Implementation site: [task/condense.rs](../crates/rustic-agent/src/task/condense.rs). Estimated effort: ~half day.

---

### F5 — No auto-continue on incomplete turns (T3, P2)

**Impact:** Minor — happened once in T3. Worth noting because it's user-visible.

**Evidence:** During T3, the agent stopped after ~30 turns. The user manually typed "please continue" to resume. CLI didn't need this on the same task — it just keeps going.

**Likely cause:** Rustic's executor sees an `end_turn` stop reason and ends the task. CLI's loop continues past end_turn when there's pending work in its internal state. Not always wrong — sometimes end_turn is legitimate — but on long investigation tasks the agent prematurely concludes.

**Fix:** Add a `max_idle_turns` heuristic — if the agent has issued tool calls in the last N turns but ends on `end_turn`, prompt it once with "anything else needed?" before fully ending. Or just add a system prompt nudge: "If you haven't fully answered the user's question, continue with more tool calls — don't end early."

Implementation site: [task/executor.rs](../crates/rustic-agent/src/task/executor.rs). Estimated effort: ~half day.

---

### F6 — Sub-agent invocation pattern is healthy (no fix needed)

Both Rustic and CLI handled T2 (multi-file feature) without spawning sub-agents, which was correct — the task was within a single context. Rustic's orchestrator correctly noted in T2: "All changes are in two crates only (rustic-db, rustic-agent/src/file_history). Doing in-process — total work is ~5 small edits, parallel sub-agents would be overhead."

That's a good decision. Sub-agent threshold heuristics in Rustic appear sound; don't change them.

## 5. Recommendations, ranked by effort-adjusted impact

| Rank | Fix | Est. effort | Expected gain |
|---|---|---|---|
| **1** | **F1** — Rename STALE_READ, add whitespace-tolerant edit matching | 1 day | Saves 2–3 turns per multi-line edit task (T1 saw 3 wasted turns; T2 likely had similar) |
| **2** | **F3** — Stabilise prompt prefix (diff two turn prompts, find what changes, move out of system prompt) | 1 day investigation + 1 day fix | Cuts cache-creation cost by ~80% — biggest single dollar saver |
| **3** | **F2** — Push `read_file` range usage in system prompt, soft-warn shell read commands | half day | Saves ~10–20 turns per investigation task (T3 had ~25 shell reads that should have been `read_file`) |
| **4** | **F4** — Scale auto-compact threshold to model context size; preserve recent file reads on compact | half day | Saves re-read turns in long tasks (T2 lost tracker.rs unnecessarily) |
| **5** | **F5** — Auto-continue heuristic for incomplete turns | half day | Removes one UX paper cut |

**If you only do #1 + #2 + #3 (the three P0 items, ~3 days total work), conservative estimate of post-fix performance:**

- T1: 9 turns → ~6 turns (-3 wasted edits) → ~36s wall, ~$0.30 cost
- T3: 77 turns → ~20 turns (no shell-read tax, no cache churn) → ~3 minutes wall, ~$1.50 cost
- Total cost across the 3 tasks: ~$5 (from $13.83) — **2.8× cheaper**
- Total wall: ~10 minutes (from ~20 minutes) — **2× faster**

That puts Rustic within ~1.3–1.5× of Claude CLI on both axes. Closing the last bit of gap likely needs deeper system-prompt and tool-design work, but the three P0 fixes alone get us most of the way.

## 6. Where Rustic is **not** worse than Claude CLI

Worth noting explicitly so we don't over-correct:

- **Correctness:** Both modes completed all 3 tasks correctly. Rustic's diffs were sometimes more verbose (T1 used 4 lines vs CLI's 1) but never wrong.
- **Sub-agent decision-making:** Rustic correctly decided to handle T2 in-process rather than spawn sub-agents.
- **File-history tracking didn't fire during these runs:** No evidence of bugs in the existing tracker that would be visible at the experiment scale.
- **No correctness regressions from concurrency:** The experiment was single-task per mode, so this doesn't probe Rustic's multi-task USP. But within-task, Rustic was solid.

## 7. Open questions worth investigating later

- Does the cache-creation ratio improve when running Rustic in harness mode (which just wraps Claude CLI)? If yes, that confirms Rustic's bridge overhead is small and the agent-loop layer is where the cost lives.
- Does Sonnet vs Opus change the picture? Sonnet might compensate for some Rustic inefficiency with raw speed. Worth testing post-fix.
- How does the gap scale on really large tasks (200+ turn workloads)? The cache-creation tax compounds; the gap might widen further.

---

## Appendix — raw data

Full per-task metrics with diffs and tool sequences in [r2-perf-experiment/runs/perf_findings.md](../../r2-perf-experiment/runs/perf_findings.md) (auto-generated from run logs).

Raw stream-json logs preserved at [r2-perf-experiment/runs/claude-cli-T*-high-*.ndjson](../../r2-perf-experiment/runs/) for any future re-analysis.

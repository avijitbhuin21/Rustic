# Rustic — TODO (Features / Research)

> Deferred until the bug list is fully implemented. Every item below must apply to
> **both** the Rustic native application **and** Rustic server.

## Features / Research

### 2. [Research] Improve orchestration

Current orchestration doesn't feel intelligent enough. Research how other similar
tools approach orchestration and what's worth borrowing.

- **Constraint:** keep it simple _for the model_ — not complex — so the agent clearly
  understands what's happening and how to proceed.
- **Goal:** make orchestration easier and more reliable for **longer-running sessions**.

**RESEARCH DONE (2026-06-10) — no changes made yet. Findings:**

_Where we're EXCELLING (keep as-is):_ context engine is already best-in-class —
`condense.rs` routes summarization to a cheaper model, preserves head+tail verbatim,
summarizes only the middle, keeps recent file reads, and dedups file-reads before
truncating (protects prompt cache). Also good: deferred tools via `tool_search`,
sub-agent depth cap, prompt-cache-aware ordering. Context management is NOT our problem.

_Where we're LACKING:_

1. **Root cause of confusion = "parallelize-first" framing.** `system_prompt.rs:76-119`
   pushes multi-agent parallelism as THE default. Field evidence (Cognition
   "Don't Build Multi-Agents") says for _coding_ (interdependent decisions) a
   single-threaded agent is more reliable; parallel sub-agents making decisions
   confuse each other. We're on the wrong side of this for edit-heavy work.
   → Fix direction: flip default to single-threaded; reserve sub-agents for
   read-only summary-returning _exploration_ (Claude Code "Explore" pattern).
   NOTE: this partially reverses the deliberate May "all-in parallelization" call.
2. **Todo list isn't a durable anchor.** `todo_write` is advisory only — not reinjected
   on a cadence, not preserved through condense. Best-in-class (Cline Focus Chain)
   reinjects the todo every N msgs and keeps it through summarization = the stable
   "what am I doing / what's next" we lack. Biggest fix for long-session drift.
3. **Invisible state changes** — opaque `[Old tool result content cleared]` stubs +
   silent condense fallback → model can hallucinate over dropped context. Add visible
   breadcrumbs.
4. **6 always-on orchestration tools** (spawn/list/check/send/nudge/stop) enlarge the
   action space; collapse to ~2 once sub-agents are read-only.

Full writeup: memory `project_orchestration_research.md`. All fixes SIMPLIFY for the
model (per constraint).

**IMPLEMENTED (2026-06-10)** — all changes live in `rustic-agent`, so native app and
server get them identically:

1. ✅ Prompt reframed to single-threaded-by-default (`system_prompt.rs`): workflow
   step "plan for parallelism" removed; new "## Sub-agents" section frames sub-agents
   as context-offloading (read-only exploration/research returning summaries, rare
   self-contained chunks). `spawn_subagent` tool description reframed to match.
   Verification + shared-resource rails kept.
2. ✅ Todo list is now a durable anchor: `ToolContext.current_todos` write-through
   slot; executor reinjects the list every 6 provider calls without a `todo_write`
   (`TODO_ANCHOR_EVERY`, Cline Focus Chain cadence); preserved VERBATIM through both
   `condense_context` and `sliding_window_fallback`; rehydrated from history on task
   resume after restart.
3. ◐ Soft collapse only: the 6 orchestration tools were already deferred behind
   `tool_search` (not in context); prompt now de-emphasizes supervision. Hard
   removal (delete send/nudge/stop + make sub-agents read-only) deliberately NOT
   done — destroys write-scope machinery; revisit only if confusion persists.
4. ✅ Cleared tool-result stubs now carry a breadcrumb ("[Old read_file result for
   'src/x.rs' cleared — superseded by a newer call…]"), still deterministic per
   result so prompt-cache stability is preserved. Condense fallback already had a
   visible notice.

Bonus fixes found during implementation: `dedup_key_for_tool` matched `"grep"` but
the tool is `grep_search` (dedup never fired for grep results); condense routed to
deprecated `gemini-1.5-flash` / missed gpt-5.x entirely (now `gemini-3-flash` /
`gpt-5.4-mini`).

### 3. Video generation tool upgrades

(Image generation is fine as-is — no changes needed.)

- **First + last frame support** — Gemini video gen supports both first and last frame;
  we currently only support the first frame. Add last-frame support.
- **Gemini Omni video editing** — Gemini Omni can edit video (feed it a video + edit
  instructions). Add support for this video-edit capability.
- Both should follow a web-search/research step, then implementation.

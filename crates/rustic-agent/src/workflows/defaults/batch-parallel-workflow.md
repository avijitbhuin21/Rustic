---
name: batch-parallel-workflow
description: Research and plan a large, mechanical change (migration, refactor, bulk rename), then execute it across many parallel sub-agents — each in an isolated git worktree or a tight write scope — and integrate the results. Use when a change decomposes into independent units across many files.
---

# Batch: parallel work orchestration

You are orchestrating a large, parallelizable change across this codebase. Do
NOT start editing immediately — follow the phases in order.

## Phase 1 — Research

1. Read the relevant parts of the codebase (prefer `outline` / `grep_search` /
   sub-agent exploration for bulk reading) until you can enumerate every place
   the change applies.
2. Decompose the change into 5–30 **work units**. Every unit MUST:
   - Be independently implementable: no unit depends on another unit's edits.
   - Touch a file set disjoint from every sibling unit.
   - Be describable in a self-contained prompt (a sub-agent sees only what you
     write plus inherited context).
3. If the change cannot be decomposed into independent units (shared types,
   cascading interfaces), STOP and tell the user this is a sequential change —
   do it yourself instead.

## Phase 2 — Plan approval

Present the unit list (name, scope, files) to the user with `ask_user` and get
explicit approval before spawning anything.

## Phase 3 — Execute in parallel

Spawn one sub-agent per unit using `spawn_subagent` batch mode (`agents: [...]`)
so they run concurrently. Pick the isolation model per unit:

- **Tight, known file set** → declare `writes` with the exact paths. The unit
  runs in the parent tree; file locks + write scopes prevent collisions, and
  everything lands in this task's worktree and auto-merges normally.
- **Broad or unpredictable file set** (codemods, formatters, generated code) →
  set `isolation: "worktree"`. The child works in a throwaway copy of the repo
  forked from HEAD; it cannot touch the parent's files.

Rules:
- Never give two units overlapping `writes` paths.
- Give each unit a `fast` model tier only when the edit is truly mechanical.
- While children run, monitor with `list_subagents` / `check_subagent`; nudge
  or stop children that drift off their unit's scope.

## Phase 4 — Integrate and verify

1. For each `isolation: "worktree"` child whose completion notice reports a
   KEPT worktree: review its diff against the fork commit, integrate the
   changes into the parent tree (copy files or `git diff | git apply` /
   cherry-pick), then delete the worktree directory.
2. Run the project's build / typecheck / tests once ALL units are integrated —
   not per-unit; cross-unit breakage only shows up in the combined tree.
3. Fix integration fallout yourself (imports, formatting, lockfiles). If one
   unit's output is unsalvageable, re-spawn just that unit with a tighter
   prompt.
4. Report per-unit outcomes (done / re-spawned / failed) in the final summary.

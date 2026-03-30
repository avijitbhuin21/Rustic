# Deletion Conflict Handling
> Edge case: target content deleted by another agent
> Date: 2026-03-30

---

## The Scenario

```
t=0   All three agents read file.rs
      Agent A, B, C all see: fn replace_one(x: &str) { ... }  at line 350

t=5   Agent A deletes fn replace_one entirely
      Reasoning: "this function is obsolete, removing it"

t=8   Agent B calls:
      edit_file(old="fn replace_one(x: &str)", new="fn replace_one(x: &String)")
      → old_string not found anywhere in file
      → this is NOT the same as "content changed" — the function is GONE

t=9   Agent C calls:
      edit_file(old="fn replace_one(x: &str) {", new="fn replace_one(x: &str) {\n    log::info!(\"...\");")
      → same problem — completely gone
```

Unlike the "stale content" case, there's no modified version to show. The target doesn't exist at all.

---

## Why This Situation Occurs

This is fundamentally a **task assignment error by the main orchestrator**. If Agent A is deleting a function, and Agents B and C are modifying that same function, the tasks were not properly independent. The main model should have either:

1. Made Agent A run first, then decided whether B and C were still needed
2. Not assigned B and C to modify a function A was going to delete
3. Assigned B and C to different work

So the **primary fix is prevention**: the system prompt tells the main model to analyze function-level dependencies before assigning parallel tasks.

But we still need graceful failure handling for when it happens anyway.

---

## Two Distinct Error Cases (Must Be Told Apart)

The model must know WHY the edit failed — the response action is completely different:

| Failure type | Cause | Correct response |
|---|---|---|
| **Content changed** | Function exists but was modified — wrong old_string | Re-read that section, regenerate edit with correct old_string |
| **Content deleted** | Function no longer exists anywhere in file | Do NOT retry the edit. Escalate to main orchestrator. Task may be moot. |

If the tool just says "not found", the model might keep retrying an edit for something that no longer exists — wasting turns and tokens.

---

## Detection: How to Tell the Difference

When `edit_file` fails:

```
Step 1: Search for old_string exactly → not found

Step 2: Fuzzy search — take first significant line of old_string,
        search entire file for closest match

        Case A: Found a close match (e.g., "fn replace_one" still exists but
                signature changed) → "CONTENT CHANGED" error → show ±150 lines

        Case B: No close match anywhere → "CONTENT DELETED" error
                → show what's near the last known line (if provided)
                → show nearby symbols that DO exist
```

The `hint_line` optional parameter becomes important here:

```json
{
  "name": "edit_file",
  "parameters": {
    "path": "src/file.rs",
    "old_string": "fn replace_one(x: &str)",
    "new_string": "fn replace_one(x: &String)",
    "hint_line": 350
  }
}
```

`hint_line` = the line the model last saw this content at. Used only for error reporting — the edit itself always uses string matching.

---

## Error Response for Deleted Content

```
edit_file FAILED — content appears to have been DELETED

'fn replace_one(x: &str)' was not found anywhere in src/file.rs.
Fuzzy search found no close match — the function may have been removed by another agent.

Last known location was around line 350.
Current content at lines 320–380:

320: fn replace_all(items: &[&str]) -> Vec<String> {
321:     items.iter().map(|s| s.to_string()).collect()
322: }
323:
324: // replace_one removed — see commit for rationale   ← Agent A may have left a comment
325:
326: fn replace_many(items: &[&str], count: usize) -> Vec<String> {
327:     ...
328: }

Functions currently in this file near that area:
  line 310: fn replace_all
  line 326: fn replace_many
  line 345: fn replace_with_pattern

ACTION REQUIRED: The function you were asked to modify no longer exists.
Do not retry this edit. Report this situation to the orchestrator.
Your task cannot be completed as originally specified.
```

This gives the sub-agent:
1. Clear statement: function is gone, don't retry
2. What's actually there now (so it can reason about whether its task is moot)
3. Explicit instruction to escalate, not retry

---

## Sub-Agent Response to the Orchestrator

The sub-agent's final result (injected into the main model via reactive completion):

```
[Sub-agent 'update-replace-one' completed — FAILED]

The function fn replace_one was not found in src/file.rs.
It appears to have been deleted by another agent (Agent A was assigned to refactor
the same file). The task cannot be completed as originally specified.

Current state of src/file.rs around line 350:
- fn replace_all (line 310)
- fn replace_many (line 326)  ← replace_one appears replaced by replace_many

Suggested actions for orchestrator:
1. If replace_one was intentionally removed — my task is moot, no action needed
2. If replace_one was accidentally removed — Agent A should restore it
3. If the functionality moved to replace_many — I can modify replace_many instead
```

The main orchestrator now has all the information it needs to decide what to do.

---

## Main Orchestrator's Recovery Options

When the main model receives this completion:

**Option 1: Task is moot** — Agent A correctly deleted the function, B and C's tasks are no longer needed. Main model acknowledges, continues.

**Option 2: Fix the assignment** — Main model spawns a new sub-agent: "Agent A deleted replace_one but Agent B still needs to modify it. Please check if Agent A's deletion was intentional by reading the area around line 324 in file.rs and the overall context."

**Option 3: Redirect the work** — "The functionality moved to replace_many. Please apply the same change (add logging) to replace_many instead."

The main model has enough context from the error report to make this decision intelligently.

---

## Prevention in the System Prompt

The most important part — stop this from happening in the first place:

```
## Task Assignment Rules for Parallel Sub-Agents

Before spawning sub-agents that modify the same file:

1. FUNCTION-LEVEL ANALYSIS: Identify which functions each agent will touch.
   Two agents must never be assigned to the same function.

2. DELETION CONFLICTS: If one agent might DELETE a function/class/section,
   do not assign another agent to MODIFY that same function/class/section.
   Run the deletion agent first, then reassign the remaining agents
   based on what still exists.

3. SEQUENTIAL DEPENDENCY: If Agent B's work depends on Agent A NOT deleting
   something, run Agent A first and wait for its result before spawning Agent B.

4. WHEN IN DOUBT: Run agents sequentially rather than in parallel.
   Parallel execution is only for genuinely independent work.
```

---

## The `hint_line` Parameter — Worth Adding?

**Yes, always include it.** Cost: zero. Benefit: when content is deleted or heavily moved, the model can report what's at the last known location rather than having to search blindly.

The model naturally knows the line number because it just used `grep -n` or `awk` to read the content before editing. Including it in the edit call is a 3-token overhead.

```bash
# Model workflow
grep -n "fn replace_one" src/file.rs
# → 350: fn replace_one(x: &str) {

# Now model knows line 350 — includes it in the edit
edit_file(
  path="src/file.rs",
  old="fn replace_one(x: &str) {",
  new="fn replace_one(x: &String) {",
  hint_line=350   # from the grep output above
)
```

---

## Summary

| Scenario | Detection | Model Action |
|---|---|---|
| Content modified (stale old_string) | Fuzzy match found elsewhere in file | Re-read ±150 lines, retry edit with corrected old_string |
| Content deleted entirely | Fuzzy match finds nothing close | Do NOT retry. Report to orchestrator with context of what's nearby |
| Line drift (insert/delete above) | `hint_line` content doesn't match | Re-grep for target, use new line number, retry |

The key distinction is: **modified = retry with corrected content. Deleted = escalate, don't retry.**

Primary fix is always task assignment — the main model should not put two agents on the same function where one might delete it. Error handling is the safety net, not the plan.

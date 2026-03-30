# Sub-Agent Async Design — Reactive Injection Pattern
> Replaces the wait_for_agents blocking approach in subagent-plan.md
> Date: 2026-03-30

---

## The Problem with `wait_for_agents`

The previous design had `wait_for_agents(['*'])` as a blocking tool call — the main model calls it, the executor pauses, and nothing happens until ALL listed agents complete. That wastes time:

```
Agent A completes at t=5s  → main model sits idle, waiting for B and C
Agent B completes at t=8s  → still idle
Agent C completes at t=12s → NOW main model wakes up, 7 wasted seconds
```

If the main model could have processed Agent A's result immediately at t=5s and started the next step, we'd save 7 seconds.

---

## The Solution: Reactive Injection

Instead of the main model waiting, **the executor loop wakes the main model automatically** whenever any sub-agent completes. The main model never blocks — it either has tool calls to make (in which case it loops normally) or it has no tool calls (in which case the loop waits for the next sub-agent completion and injects it as a new message).

```
t=0s  Main model: spawns agents A, B, C → ends turn with no tool calls
t=0s  Executor: sees active sub-agents → enters "reactive wait"
t=5s  Agent A finishes → executor injects completion notification → re-invokes main model
t=5s  Main model: processes A's result, starts step 2 (tool calls) → ends turn
t=5s  Executor: still has B and C active → back to reactive wait
t=8s  Agent B finishes → inject → re-invoke main model
t=8s  Main model: processes B's result, does more work
t=12s Agent C finishes → inject → re-invoke main model
t=12s Main model: synthesizes all results, returns final answer → no sub-agents left → done
```

Total: 12s (same as sequential finish time), but intermediate results processed as they arrive.

---

## Updated Executor Loop

The key change is in `run_turn` in `executor.rs`. The loop now has a third condition beyond "has tool calls" and "no tool calls":

```rust
pub async fn run_turn(
    &self,
    task_id: &str,
    messages: &mut Vec<Message>,
    context: &ToolContext,
    event_tx: &mpsc::UnboundedSender<TaskEvent>,
) -> Result<()> {
    loop {
        // ── Phase 1: Call the provider ──────────────────────────────────────
        let tool_defs = self.get_tool_definitions(context); // depth-aware
        let response = self.provider.chat(messages.clone(), tool_defs, &self.config).await?;

        let assistant_msg = response_to_message(&response);
        messages.push(assistant_msg.clone());

        // Emit text events to UI
        emit_text_events(&response, task_id, event_tx);

        // ── Phase 2: Handle tool calls (if any) ─────────────────────────────
        let tool_uses = extract_tool_uses(&response);
        if !tool_uses.is_empty() {
            // Execute tools (spawn_subagent returns immediately, others execute normally)
            execute_tools(&tool_uses, messages, context, task_id, event_tx).await?;
            continue; // loop back for next provider call
        }

        // ── Phase 3: No tool calls — check for active sub-agents ────────────
        let active = context.subagent_registry.active_for_task(task_id);
        if active.is_empty() {
            // No tool calls, no active sub-agents — turn is truly complete
            break;
        }

        // ── Phase 4: Reactive wait — block until ANY sub-agent completes ────
        // This is NOT a busy-wait. tokio::select! suspends the task and resumes
        // only when one of the notify handles fires.
        let completed = context.subagent_registry.wait_for_any(task_id).await;

        let remaining = context.subagent_registry.active_for_task(task_id);
        let remaining_names: Vec<_> = remaining.iter().map(|a| a.agent_id.as_str()).collect();

        // Inject completion as a new user message — re-triggers the main model
        let notification = build_completion_notification(&completed, &remaining_names);
        messages.push(Message {
            role: Role::User,
            content: vec![ContentBlock::Text { text: notification }],
        });

        let _ = event_tx.send(TaskEvent::SubagentCompleted {
            task_id: task_id.to_string(),
            agent_id: completed.agent_id.clone(),
            result: completed.result.clone().unwrap_or_default(),
        });

        // Loop back — main model will process the completion and decide next steps
    }

    Ok(())
}

fn build_completion_notification(completed: &SubagentResult, remaining: &[&str]) -> String {
    let remaining_msg = if remaining.is_empty() {
        "All sub-agents have now completed.".to_string()
    } else {
        format!(
            "{} sub-agent(s) still running: {}",
            remaining.len(),
            remaining.join(", ")
        )
    };

    format!(
        "[Sub-agent '{}' completed — model: {}]\n\n\
         {}\n\n\
         {}\n\n\
         Process this result and continue with whatever makes sense. \
         You do not need to wait for remaining agents unless your next step depends on them.",
        completed.agent_id,
        completed.model,
        completed.result.as_deref().unwrap_or("(no output)"),
        remaining_msg
    )
}
```

---

## Updated `SubagentRegistry`

The registry needs a `wait_for_any` that returns on the FIRST completion for a given parent task:

```rust
pub struct SubagentRegistry {
    agents: Mutex<HashMap<String, SubagentEntry>>,
    // Per parent_task_id: a broadcast channel that fires on any completion
    completion_tx: Mutex<HashMap<String, broadcast::Sender<SubagentResult>>>,
}

impl SubagentRegistry {
    /// Called by the spawned tokio task when a sub-agent finishes
    pub fn complete(&self, parent_task_id: &str, agent_id: &str, result: String) {
        let mut agents = self.agents.lock().unwrap();
        if let Some(entry) = agents.get_mut(agent_id) {
            entry.status = SubagentStatus::Completed;
            entry.result = Some(result.clone());
        }

        // Fire the broadcast — any waiting `wait_for_any` call wakes up
        let txs = self.completion_tx.lock().unwrap();
        if let Some(tx) = txs.get(parent_task_id) {
            let _ = tx.send(SubagentResult {
                agent_id: agent_id.to_string(),
                status: SubagentStatus::Completed,
                result: Some(result),
                model: agents.get(agent_id).map(|e| e.model.clone()).unwrap_or_default(),
            });
        }
    }

    /// Suspends until ANY sub-agent belonging to parent_task_id completes.
    /// Uses tokio broadcast channel — zero CPU while waiting.
    pub async fn wait_for_any(&self, parent_task_id: &str) -> SubagentResult {
        let mut rx = {
            let txs = self.completion_tx.lock().unwrap();
            txs.get(parent_task_id)
                .expect("No completion channel for task")
                .subscribe()
        };
        // Blocks here (yields to tokio scheduler) until a completion fires
        rx.recv().await.expect("Completion channel closed unexpectedly")
    }

    pub fn active_for_task(&self, parent_task_id: &str) -> Vec<SubagentEntry> {
        self.agents.lock().unwrap()
            .values()
            .filter(|e| e.parent_task_id == parent_task_id
                     && e.status == SubagentStatus::Running)
            .cloned()
            .collect()
    }
}
```

**Why `broadcast::channel`?**
- `tokio::sync::broadcast` is perfect here: multiple senders (sub-agents), one receiver per parent task.
- Zero-cost when idle — the executor is suspended by tokio, not spinning.
- Any completion wakes exactly one `wait_for_any` call, then the loop re-evaluates.

---

## Revised Tool Set

With reactive injection, `wait_for_agents` (blocking) is no longer needed as a primary pattern. But we keep explicit control tools for cases where the main model knows it must have everything before proceeding:

| Tool | Behavior | When to use |
|---|---|---|
| `spawn_subagent(id, task, model, files)` | Spawn and return immediately | Always — non-blocking |
| `wait_for_all_agents([ids])` | Block until ALL listed agents complete | When next step strictly requires ALL results (e.g. merging outputs) |
| `list_active_agents()` | Returns current status of all sub-agents | Checking progress mid-turn |
| `cancel_agent(id)` | Cancel a running sub-agent | Abandoning unneeded work |

`wait_for_all_agents` is implemented as a simple shorthand — it calls `wait_for_any` in a loop until all listed IDs have completed status.

**The model uses them like this:**

```
// Pattern 1: Fully reactive (most common)
spawn_subagent("analyze-auth", ..., files=["src/auth/"])
spawn_subagent("analyze-api", ..., files=["src/api/"])
spawn_subagent("run-tests", ..., files=[])
// → End turn. Executor handles the rest. Model wakes up as each finishes.

// Pattern 2: Explicit wait-all when strictly needed
spawn_subagent("fetch-data-a", ...)
spawn_subagent("fetch-data-b", ...)
wait_for_all_agents(["fetch-data-a", "fetch-data-b"])
// → Now merge the data (model knows it needs both before merging)

// Pattern 3: Hybrid — do independent work while waiting
spawn_subagent("slow-analysis", ...)
write_file("docs/plan.md", "...")  // ← do this while slow-analysis runs
// → End turn. When slow-analysis finishes, executor injects it and model continues.
```

---

## Parallel Tool Execution Within a Turn

There's a second level of parallelism: when the model calls multiple `spawn_subagent` tools in a single response (multiple tool_use blocks), those spawns themselves should run concurrently rather than waiting for each spawn call to finish before starting the next.

Since `spawn_subagent` is non-blocking (it just calls `tokio::spawn` and returns), running them sequentially in the tool loop is fine in practice — each spawn takes microseconds. But to be explicit, we can group spawn calls:

```rust
async fn execute_tools(tool_uses: &[(id, name, input)], ...) {
    // Separate spawn calls from all other tools
    let (spawns, others): (Vec<_>, Vec<_>) = tool_uses
        .iter()
        .partition(|(_, name, _)| name == "spawn_subagent");

    // Fire all spawns concurrently (they're already non-blocking, but explicit is clear)
    futures::future::join_all(
        spawns.iter().map(|(id, name, input)| async {
            execute_one_tool(id, name, input, context).await
        })
    ).await;

    // Execute other tools sequentially (may have dependencies between them)
    for (id, name, input) in &others {
        execute_one_tool(id, name, input, context).await;
    }
}
```

---

## System Prompt Instructions for the Main Model

The system prompt section on sub-agents:

```
## Sub-Agent Orchestration

You can spawn sub-agents to work in parallel. Key rules:

1. Spawn multiple agents when tasks are independent (different files, no data dependencies).
2. Use spawn_subagent() and end your turn — you will be automatically notified when each
   completes. You do not need to call wait_for_any explicitly.
3. When notified of a completion, process that result immediately. Do not wait for other
   agents unless your next action strictly requires their output.
4. Only use wait_for_all_agents() when you genuinely cannot proceed without all results
   (e.g., merging outputs from multiple sources).
5. Assign specific files to each agent via the `files` parameter to prevent conflicts.
   Two agents should never write to the same file.
6. Sub-agents cannot spawn their own sub-agents.
7. Available models for sub-agents: {subagent_models_list}
   Use cheaper/faster models for simple tasks (reading, analysis, search).
   Use more capable models for complex tasks (code generation, refactoring).
```

---

## Full Async Flow Example

**Task:** "Refactor the auth module, update all tests that use auth, and update the docs."

```
t=0s  Main model analysis:
      - Refactoring auth (src/auth/) and updating tests (tests/) are independent
        IF we do the refactor first and pass the new API to the test-updater.
      - Docs update (docs/) is independent of both.
      - Decision: refactor auth first, spawn test-updater + docs-updater in parallel.

t=0s  Main model calls:
      spawn_subagent("refactor-auth", "Refactor auth module...", "claude-haiku", ["src/auth/"])
      → End turn.

t=0s  Executor: 1 active sub-agent, enters reactive wait.

t=8s  "refactor-auth" completes → executor injects:
      "[Sub-agent 'refactor-auth' completed]
       Result: Refactored auth.rs — new API: AuthClient::new(config), removed old token() method.
       All sub-agents have now completed."

t=8s  Main model wakes, sees the new API, calls:
      spawn_subagent("update-tests", "Update tests to use AuthClient::new...", "gpt-4o-mini", ["tests/"])
      spawn_subagent("update-docs", "Update auth docs...", "gemini-flash", ["docs/"])
      → End turn.

t=8s  Executor: 2 active sub-agents, enters reactive wait.

t=11s "update-docs" completes (faster) → executor injects notification.

t=11s Main model wakes:
      "update-tests still running. Docs look good. Let me verify the docs output..."
      read_file("docs/auth.md")
      → End turn.

t=11s Executor: 1 active sub-agent still running, reactive wait.

t=14s "update-tests" completes → executor injects notification.

t=14s Main model wakes, synthesizes all results, writes summary → no more tool calls → DONE.

Total: 14s (vs ~30s sequential, vs ~14s if we'd waited for all at t=8s)
```

---

## Summary of Changes from Previous Plan

| Aspect | Previous (blocking) | Updated (reactive) |
|---|---|---|
| `wait_for_agents` | Blocks until ALL complete | Removed as primary pattern |
| `wait_for_all_agents` | N/A | Kept for explicit all-wait |
| Main model wakeup | Explicit tool call | Automatic injection by executor |
| Sub-agent completion handling | After all done | As each finishes |
| Time efficiency | Wastes time waiting | Processes results immediately |
| Executor complexity | Simple loop | Needs broadcast channel + reactive wait phase |
| Model instruction | "Call wait_for_agents" | "Spawn and end turn — you'll be notified" |

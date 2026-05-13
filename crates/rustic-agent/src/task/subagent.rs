use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

// The sub-agent concurrency cap moved to `BudgetSettings.max_concurrent_subagents`
// so users can raise / disable it from the Settings → Budget panel. The
// historical hard-cap constant lives at
// `crate::budget::DEFAULT_MAX_CONCURRENT_SUBAGENTS` and is read as a
// fallback when the field is missing from a persisted config.

/// Returns true if two declared-write paths overlap: identical, or one is a
/// directory ancestor of the other. Uses simple string-prefix matching on
/// normalized forward-slash paths — not bulletproof (symlinks, case-insensitive
/// FS quirks) but sufficient for the "don't let two agents edit the same file"
/// contract. Also used at write time by `check_write_scope` in file_ops.rs to
/// reject writes outside a sub-agent's declared scope.
pub fn paths_overlap(a: &str, b: &str) -> bool {
    let norm = |p: &str| p.replace('\\', "/").trim_end_matches('/').to_string();
    let a = norm(a);
    let b = norm(b);
    if a == b {
        return true;
    }
    let a_prefix = format!("{}/", a);
    let b_prefix = format!("{}/", b);
    b.starts_with(&a_prefix) || a.starts_with(&b_prefix)
}

/// A write the sub-agent needed to make but was blocked from by its declared
/// write scope. Returned to the orchestrator so it can decide whether to do
/// the write itself, spawn a follow-up sub-agent, or expand the scope.
#[derive(Debug, Clone)]
pub struct BlockedWrite {
    pub path: String,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub struct SubagentResult {
    pub agent_id: String,
    pub model: String,
    pub summary: String,
    pub notes: Option<String>,
    /// Writes the sub-agent wanted to make but couldn't because they were
    /// outside its declared `writes` scope. Populated via the
    /// `report_blocked_write` tool.
    pub blocked_on: Vec<BlockedWrite>,
}

impl SubagentResult {
    /// Format the "[Sub-agent 'X' completed]" block that gets injected back into
    /// the orchestrator's context. Includes the summary and, when non-empty, a
    /// structured tail listing blocked writes so the orchestrator can decide
    /// what to do with them. Shared across the three injection paths
    /// (`wait_for_subagents` tool output + executor's `wait_for_any` + executor's
    /// `drain_pending`) so they stay in sync.
    pub fn format_completion_block(&self) -> String {
        let mut out = format!("[Sub-agent '{}' completed]\n{}", self.agent_id, self.summary);
        if !self.blocked_on.is_empty() {
            out.push_str(&format!(
                "\n\n[Sub-agent '{}' blocked on {} write(s)]",
                self.agent_id,
                self.blocked_on.len()
            ));
            for bw in &self.blocked_on {
                out.push_str(&format!("\n- {} — {}", bw.path, bw.reason));
            }
            out.push_str(
                "\nYou (the orchestrator) decide: do these writes yourself, spawn a follow-up \
                 sub-agent with narrower scope, or re-dispatch the task with expanded writes.",
            );
        }
        out
    }
}

#[derive(Debug, Clone)]
pub struct SubagentEntry {
    pub agent_id: String,
    pub model: String,
    pub status: SubagentStatus,
    /// File paths this sub-agent declared it will write to. Used by the
    /// spawn-time collision check to serialize agents that target overlapping
    /// files. Empty list = reads-only (safe to run alongside anyone).
    pub writes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SubagentStatus {
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Clone)]
pub enum SubagentCompletionEvent {
    Completed(SubagentResult),
    Failed { agent_id: String, error: String },
}

pub struct SubagentRegistry {
    /// parent_task_id -> agent_id -> SubagentEntry
    agents: Mutex<HashMap<String, HashMap<String, SubagentEntry>>>,
    /// parent_task_id -> queue of unconsumed completion events
    pending: Mutex<HashMap<String, VecDeque<SubagentCompletionEvent>>>,
    /// parent_task_id -> Notify (wakes wait_for_any on completion)
    notifies: Mutex<HashMap<String, Arc<Notify>>>,
}

impl SubagentRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            agents: Mutex::new(HashMap::new()),
            pending: Mutex::new(HashMap::new()),
            notifies: Mutex::new(HashMap::new()),
        })
    }

    /// Register a new sub-agent under a parent task.
    pub fn register(&self, parent_task_id: &str, agent_id: &str, model: &str, writes: Vec<String>) {
        let mut agents = self.agents.lock().unwrap();
        let task_agents = agents.entry(parent_task_id.to_string()).or_default();
        task_agents.insert(
            agent_id.to_string(),
            SubagentEntry {
                agent_id: agent_id.to_string(),
                model: model.to_string(),
                status: SubagentStatus::Running,
                writes,
            },
        );
        // Ensure a Notify exists for this parent task
        let mut notifies = self.notifies.lock().unwrap();
        notifies
            .entry(parent_task_id.to_string())
            .or_insert_with(|| Arc::new(Notify::new()));
    }

    /// Returns the id of a currently-running sub-agent whose declared writes
    /// overlap with `candidate_writes`. Returns None if no collision. Used by
    /// spawn_subagent to reject spawns that would race on the same file.
    pub fn find_write_collision(
        &self,
        parent_task_id: &str,
        candidate_writes: &[String],
    ) -> Option<String> {
        if candidate_writes.is_empty() {
            return None;
        }
        let agents = self.agents.lock().unwrap();
        let task_agents = match agents.get(parent_task_id) {
            Some(m) => m,
            None => return None,
        };
        for entry in task_agents.values() {
            if entry.status != SubagentStatus::Running {
                continue;
            }
            for w in &entry.writes {
                if candidate_writes.iter().any(|c| paths_overlap(c, w)) {
                    return Some(entry.agent_id.clone());
                }
            }
        }
        None
    }

    /// Count currently-running sub-agents for a parent task.
    pub fn running_count(&self, parent_task_id: &str) -> usize {
        let agents = self.agents.lock().unwrap();
        agents
            .get(parent_task_id)
            .map(|m| m.values().filter(|e| e.status == SubagentStatus::Running).count())
            .unwrap_or(0)
    }

    /// Mark a sub-agent as completed and wake any waiting executor.
    pub fn complete(&self, parent_task_id: &str, result: SubagentResult) {
        {
            let mut agents = self.agents.lock().unwrap();
            if let Some(task_agents) = agents.get_mut(parent_task_id) {
                if let Some(entry) = task_agents.get_mut(&result.agent_id) {
                    entry.status = SubagentStatus::Completed;
                }
            }
        }
        {
            let mut pending = self.pending.lock().unwrap();
            pending
                .entry(parent_task_id.to_string())
                .or_default()
                .push_back(SubagentCompletionEvent::Completed(result));
        }
        let notifies = self.notifies.lock().unwrap();
        if let Some(notify) = notifies.get(parent_task_id) {
            notify.notify_one();
        }
    }

    /// Mark a sub-agent as failed and wake any waiting executor.
    pub fn fail(&self, parent_task_id: &str, agent_id: &str, error: String) {
        {
            let mut agents = self.agents.lock().unwrap();
            if let Some(task_agents) = agents.get_mut(parent_task_id) {
                if let Some(entry) = task_agents.get_mut(agent_id) {
                    entry.status = SubagentStatus::Failed;
                }
            }
        }
        {
            let mut pending = self.pending.lock().unwrap();
            pending
                .entry(parent_task_id.to_string())
                .or_default()
                .push_back(SubagentCompletionEvent::Failed {
                    agent_id: agent_id.to_string(),
                    error,
                });
        }
        let notifies = self.notifies.lock().unwrap();
        if let Some(notify) = notifies.get(parent_task_id) {
            notify.notify_one();
        }
    }

    /// Returns all Running sub-agents for the given parent task.
    pub fn active_for_task(&self, parent_task_id: &str) -> Vec<SubagentEntry> {
        let agents = self.agents.lock().unwrap();
        agents
            .get(parent_task_id)
            .map(|m| {
                m.values()
                    .filter(|e| e.status == SubagentStatus::Running)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Returns all sub-agents (any status) for the given parent task.
    pub fn all_for_task(&self, parent_task_id: &str) -> Vec<SubagentEntry> {
        let agents = self.agents.lock().unwrap();
        agents
            .get(parent_task_id)
            .map(|m| m.values().cloned().collect())
            .unwrap_or_default()
    }

    /// Drain all pending (already-completed) events without blocking.
    /// Returns events for sub-agents that finished while the model was generating or tools were executing.
    pub fn drain_pending(&self, parent_task_id: &str) -> Vec<SubagentCompletionEvent> {
        let mut pending = self.pending.lock().unwrap();
        if let Some(queue) = pending.get_mut(parent_task_id) {
            queue.drain(..).collect()
        } else {
            Vec::new()
        }
    }

    /// Wait asynchronously for any sub-agent to complete or fail.
    /// Returns None if there are no active agents.
    pub async fn wait_for_any(&self, parent_task_id: &str) -> Option<SubagentCompletionEvent> {
        loop {
            // Check the pending queue first
            {
                let mut pending = self.pending.lock().unwrap();
                if let Some(queue) = pending.get_mut(parent_task_id) {
                    if let Some(event) = queue.pop_front() {
                        return Some(event);
                    }
                }
            }

            // If there are no running agents (and no pending events), we're done
            if self.active_for_task(parent_task_id).is_empty() {
                return None;
            }

            // Get the Notify for this parent task
            let notify = {
                let notifies = self.notifies.lock().unwrap();
                notifies.get(parent_task_id)?.clone()
            };

            // Wait for a notification (notify_one() stores a permit if called before we await)
            notify.notified().await;
            // Loop back to check the queue
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_overlap_identical() {
        assert!(paths_overlap("src/foo.rs", "src/foo.rs"));
    }

    #[test]
    fn paths_overlap_directory_ancestor() {
        // One path is a directory ancestor of the other.
        assert!(paths_overlap("src/foo", "src/foo/bar.rs"));
        assert!(paths_overlap("src/foo/bar.rs", "src/foo"));
    }

    #[test]
    fn paths_overlap_sibling_no_overlap() {
        assert!(!paths_overlap("src/foo.rs", "src/bar.rs"));
        assert!(!paths_overlap("src/foo", "src/foobar"));
    }

    #[test]
    fn paths_overlap_normalizes_backslashes() {
        // Windows-style paths should match their forward-slash counterparts.
        assert!(paths_overlap("src\\foo.rs", "src/foo.rs"));
        assert!(paths_overlap("src\\foo", "src/foo/bar.rs"));
    }

    #[test]
    fn paths_overlap_trailing_slash_tolerance() {
        // Trailing slashes on directory scope entries shouldn't matter.
        assert!(paths_overlap("src/foo/", "src/foo/bar.rs"));
    }
}

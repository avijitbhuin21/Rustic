use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

use crate::checkpoint::TaskDiff;

#[derive(Debug, Clone)]
pub struct SubagentResult {
    pub agent_id: String,
    pub model: String,
    pub summary: String,
    pub notes: Option<String>,
    pub diff: TaskDiff,
}

#[derive(Debug, Clone)]
pub struct SubagentEntry {
    pub agent_id: String,
    pub model: String,
    pub status: SubagentStatus,
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
    pub fn register(&self, parent_task_id: &str, agent_id: &str, model: &str) {
        let mut agents = self.agents.lock().unwrap();
        let task_agents = agents.entry(parent_task_id.to_string()).or_default();
        task_agents.insert(
            agent_id.to_string(),
            SubagentEntry {
                agent_id: agent_id.to_string(),
                model: model.to_string(),
                status: SubagentStatus::Running,
            },
        );
        // Ensure a Notify exists for this parent task
        let mut notifies = self.notifies.lock().unwrap();
        notifies
            .entry(parent_task_id.to_string())
            .or_insert_with(|| Arc::new(Notify::new()));
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

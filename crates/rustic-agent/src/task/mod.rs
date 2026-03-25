pub mod executor;
pub mod permissions;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TaskStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskInfo {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub status: TaskStatus,
    pub provider_type: String,
    pub model: String,
}

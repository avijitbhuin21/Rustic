use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum PermissionLevel {
    /// Read-only. No file writes or command execution.
    Chat,
    /// File writes and commands require per-operation user approval. (Default)
    ManualEdit,
    /// File writes auto-allowed. Command execution requires user approval.
    AutoEdit,
    /// File writes and commands all auto-allowed. No approval prompts.
    FullAuto,
}

impl Default for PermissionLevel {
    fn default() -> Self {
        Self::ManualEdit
    }
}

#[derive(Debug, Clone)]
pub enum Action {
    Read,
    Write,
    Execute,
}

/// Shared, mutable permission state that can be updated mid-conversation.
/// Both the Tauri command layer (set_task_permissions) and the running executor
/// read from/write to this same Arc.
#[derive(Debug, Clone)]
pub struct SharedPermissions {
    inner: Arc<Mutex<SharedPermissionsInner>>,
}

#[derive(Debug)]
struct SharedPermissionsInner {
    level: PermissionLevel,
    sensitive_files_allowed: bool,
}

impl SharedPermissions {
    pub fn new(level: PermissionLevel, sensitive_files_allowed: bool) -> Self {
        Self {
            inner: Arc::new(Mutex::new(SharedPermissionsInner {
                level,
                sensitive_files_allowed,
            })),
        }
    }

    pub fn level(&self) -> PermissionLevel {
        self.inner.lock().unwrap().level.clone()
    }

    pub fn sensitive_files_allowed(&self) -> bool {
        self.inner.lock().unwrap().sensitive_files_allowed
    }

    pub fn set_level(&self, level: PermissionLevel) {
        self.inner.lock().unwrap().level = level;
    }

    pub fn set_sensitive_files_allowed(&self, allowed: bool) {
        self.inner.lock().unwrap().sensitive_files_allowed = allowed;
    }
}

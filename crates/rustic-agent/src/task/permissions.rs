use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum PermissionLevel {
    Admin,     // Bypass all permission checks
    ReadWrite, // Read + write + commands (may need UI confirmation)
    ReadOnly,  // Only read operations allowed
}

impl Default for PermissionLevel {
    fn default() -> Self {
        Self::ReadWrite
    }
}

#[derive(Debug, Clone)]
pub enum Action {
    Read,
    Write,
    Execute,
}

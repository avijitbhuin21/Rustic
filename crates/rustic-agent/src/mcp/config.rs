use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Which config file an MCP server was loaded from.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum McpScope {
    /// Global, in the app data dir (`mcp.json`). Shared across projects.
    User,
    /// Per-project `.mcp.json` in the project root. Committed to source control.
    Project,
}

impl Default for McpScope {
    fn default() -> Self {
        McpScope::User
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub id: String,
    pub name: String,
    pub transport: McpTransport,
    pub enabled: bool,
    #[serde(default)]
    pub scope: McpScope,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum McpTransport {
    #[serde(rename = "stdio")]
    Stdio {
        command: String,
        args: Vec<String>,
        #[serde(default)]
        env: HashMap<String, String>,
    },
    #[serde(rename = "sse")]
    Sse {
        url: String,
        #[serde(default)]
        headers: HashMap<String, String>,
    },
}

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Where an MCP server definition came from.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum McpSource {
    /// Manually added via the UI.
    Manual,
    /// Loaded automatically from a `.mcp.json` file in the project root.
    Json,
}

impl Default for McpSource {
    fn default() -> Self {
        McpSource::Manual
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub id: String,
    pub name: String,
    pub transport: McpTransport,
    pub enabled: bool,
    /// Where this server definition came from.
    #[serde(default)]
    pub source: McpSource,
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

pub mod client;
pub mod config;

use crate::provider::ToolDef;
use anyhow::Result;
use client::McpClient;
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;

pub use config::{McpServerConfig as ServerConfig, McpSource, McpTransport};
// Private import for the original name (re-exported only as ServerConfig above)
use config::McpServerConfig;

/// Manages multiple MCP server connections.
pub struct McpManager {
    configs: Vec<McpServerConfig>,
    clients: HashMap<String, McpClient>,
}

impl McpManager {
    pub fn new() -> Self {
        Self {
            configs: Vec::new(),
            clients: HashMap::new(),
        }
    }

    pub fn add_server(&mut self, config: McpServerConfig) {
        // Remove existing with same id
        self.configs.retain(|c| c.id != config.id);
        self.configs.push(config);
    }

    pub fn remove_server(&mut self, id: &str) {
        self.configs.retain(|c| c.id != id);
        if let Some(mut client) = self.clients.remove(id) {
            client.disconnect();
        }
    }

    /// Remove all servers whose source is Json (called before re-loading .mcp.json).
    pub fn remove_json_servers(&mut self) {
        let to_remove: Vec<String> = self.configs
            .iter()
            .filter(|c| c.source == McpSource::Json)
            .map(|c| c.id.clone())
            .collect();
        for id in to_remove {
            self.remove_server(&id);
        }
    }

    pub fn list_servers(&self) -> Vec<McpServerConfig> {
        self.configs.clone()
    }

    /// Load servers from a `.mcp.json` file (Claude Code format).
    ///
    /// Replaces all previously Json-sourced servers with the new set.
    /// Returns the number of servers loaded.
    ///
    /// Format:
    /// ```json
    /// {
    ///   "mcpServers": {
    ///     "server-name": {
    ///       "command": "npx",
    ///       "args": ["-y", "@something/mcp"],
    ///       "env": {}
    ///     }
    ///   }
    /// }
    /// ```
    pub fn load_from_json_file(&mut self, path: &Path) -> Result<usize> {
        let text = std::fs::read_to_string(path)?;
        let json: serde_json::Value = serde_json::from_str(&text)?;

        let servers_map = json
            .get("mcpServers")
            .and_then(|v| v.as_object())
            .ok_or_else(|| anyhow::anyhow!(".mcp.json must have a \"mcpServers\" object"))?;

        // Drop all previously json-sourced servers
        self.remove_json_servers();

        let mut count = 0;
        for (name, def) in servers_map {
            let transport = if let Some(url) = def.get("url").and_then(|v| v.as_str()) {
                // SSE transport
                let headers = def
                    .get("headers")
                    .and_then(|v| v.as_object())
                    .map(|obj| {
                        obj.iter()
                            .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                            .collect()
                    })
                    .unwrap_or_default();
                McpTransport::Sse {
                    url: url.to_string(),
                    headers,
                }
            } else {
                // Stdio transport
                let command = def
                    .get("command")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let args = def
                    .get("args")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default();
                let env = def
                    .get("env")
                    .and_then(|v| v.as_object())
                    .map(|obj| {
                        obj.iter()
                            .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                            .collect()
                    })
                    .unwrap_or_default();
                McpTransport::Stdio { command, args, env }
            };

            let id = format!("json-{}", name);
            self.configs.push(McpServerConfig {
                id,
                name: name.clone(),
                transport,
                enabled: true,
                source: McpSource::Json,
            });
            count += 1;
        }

        Ok(count)
    }

    /// Connect to a server and list its tools (for testing).
    pub fn test_server(&mut self, id: &str) -> Result<Vec<ToolDef>> {
        let config = self
            .configs
            .iter()
            .find(|c| c.id == id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Server not found: {}", id))?;

        let mut client = McpClient::connect(config)?;
        let tools = client.list_tools()?;
        client.disconnect();
        Ok(tools)
    }

    /// Connect to all enabled servers and aggregate their tools.
    pub fn connect_all(&mut self) -> Result<()> {
        for config in &self.configs {
            if !config.enabled {
                continue;
            }
            if self.clients.contains_key(&config.id) {
                continue;
            }
            match McpClient::connect(config.clone()) {
                Ok(client) => {
                    self.clients.insert(config.id.clone(), client);
                }
                Err(e) => {
                    eprintln!("Failed to connect MCP server {}: {}", config.name, e);
                }
            }
        }
        Ok(())
    }

    /// Get tool definitions from all connected servers.
    pub fn all_tools(&mut self) -> Vec<ToolDef> {
        let mut tools = Vec::new();
        for (_, client) in &mut self.clients {
            if let Ok(t) = client.list_tools() {
                tools.extend(t);
            }
        }
        tools
    }

    /// Call a tool on the appropriate server.
    pub fn call_tool(&mut self, name: &str, arguments: Value) -> Result<Value> {
        // Try each connected client until one has the tool
        for (_, client) in &mut self.clients {
            if let Ok(tools) = client.list_tools() {
                if tools.iter().any(|t| t.name == name) {
                    return client.call_tool(name, arguments);
                }
            }
        }
        Err(anyhow::anyhow!("No MCP server provides tool: {}", name))
    }

    pub fn disconnect_all(&mut self) {
        for (_, mut client) in self.clients.drain() {
            client.disconnect();
        }
    }
}

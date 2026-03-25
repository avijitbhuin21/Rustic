pub mod client;
pub mod config;

use crate::provider::ToolDef;
use anyhow::Result;
use client::McpClient;
use config::McpServerConfig;
use serde_json::Value;
use std::collections::HashMap;

pub use config::{McpServerConfig as ServerConfig, McpTransport};

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

    pub fn list_servers(&self) -> Vec<McpServerConfig> {
        self.configs.clone()
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

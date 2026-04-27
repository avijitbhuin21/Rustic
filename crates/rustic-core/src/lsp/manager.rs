use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::client::LspClient;

/// Notification from LSP server to frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LspNotification {
    Diagnostics {
        uri: String,
        diagnostics: Vec<DiagnosticInfo>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticInfo {
    pub range_start_line: u32,
    pub range_start_col: u32,
    pub range_end_line: u32,
    pub range_end_col: u32,
    pub severity: String, // "error", "warning", "info", "hint"
    pub message: String,
    pub source: Option<String>,
}

/// Configuration for a language server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspServerConfig {
    pub language_id: String,
    pub command: String,
    pub args: Vec<String>,
    pub file_extensions: Vec<String>,
}

impl LspServerConfig {
    /// Default server configs for common languages.
    pub fn defaults() -> Vec<Self> {
        vec![
            Self {
                language_id: "rust".into(),
                command: "rust-analyzer".into(),
                args: vec![],
                file_extensions: vec!["rs".into()],
            },
            Self {
                language_id: "typescript".into(),
                command: "typescript-language-server".into(),
                args: vec!["--stdio".into()],
                file_extensions: vec!["ts".into(), "tsx".into(), "js".into(), "jsx".into()],
            },
            Self {
                language_id: "python".into(),
                command: "pylsp".into(),
                args: vec![],
                file_extensions: vec!["py".into()],
            },
            Self {
                language_id: "go".into(),
                command: "gopls".into(),
                args: vec![],
                file_extensions: vec!["go".into()],
            },
            Self {
                language_id: "c".into(),
                command: "clangd".into(),
                args: vec![],
                file_extensions: vec!["c".into(), "h".into(), "cpp".into(), "hpp".into(), "cc".into()],
            },
            Self {
                language_id: "json".into(),
                command: "vscode-json-language-server".into(),
                args: vec!["--stdio".into()],
                file_extensions: vec!["json".into()],
            },
            Self {
                language_id: "css".into(),
                command: "vscode-css-language-server".into(),
                args: vec!["--stdio".into()],
                file_extensions: vec!["css".into(), "scss".into(), "less".into()],
            },
            Self {
                language_id: "html".into(),
                command: "vscode-html-language-server".into(),
                args: vec!["--stdio".into()],
                file_extensions: vec!["html".into(), "htm".into()],
            },
        ]
    }
}

/// Manages LSP clients: one per (project_root, language_id).
pub struct LspManager {
    /// Map of (project_root, language_id) -> LspClient
    clients: HashMap<(String, String), LspClient>,
    /// Available server configurations
    configs: Vec<LspServerConfig>,
}

impl LspManager {
    pub fn new() -> Self {
        Self {
            clients: HashMap::new(),
            configs: LspServerConfig::defaults(),
        }
    }

    pub fn set_configs(&mut self, configs: Vec<LspServerConfig>) {
        self.configs = configs;
    }

    pub fn configs(&self) -> &[LspServerConfig] {
        &self.configs
    }

    /// Find a server config for a given file extension.
    pub fn config_for_extension(&self, ext: &str) -> Option<&LspServerConfig> {
        self.configs.iter().find(|c| c.file_extensions.iter().any(|e| e == ext))
    }

    /// Get or start an LSP client for the given project root and file extension.
    pub fn get_or_start(
        &mut self,
        project_root: &str,
        file_extension: &str,
    ) -> Result<Option<&LspClient>> {
        let config = match self.config_for_extension(file_extension) {
            Some(c) => c.clone(),
            None => return Ok(None),
        };

        let key = (project_root.to_string(), config.language_id.clone());

        if !self.clients.contains_key(&key) {
            // Convert project root to a file URI
            let root_uri = path_to_uri(project_root);

            match LspClient::start(&config.command, &config.args, &root_uri, &config.language_id) {
                Ok(client) => {
                    self.clients.insert(key.clone(), client);
                }
                Err(e) => {
                    // Server not installed — not an error, just unavailable
                    tracing::warn!("LSP server '{}' not available: {}", config.command, e);
                    return Ok(None);
                }
            }
        }

        Ok(self.clients.get(&key))
    }

    /// Get an existing client (no auto-start).
    pub fn get_client(&self, project_root: &str, language_id: &str) -> Option<&LspClient> {
        let key = (project_root.to_string(), language_id.to_string());
        self.clients.get(&key)
    }

    /// Stop a specific client.
    pub fn stop(&mut self, project_root: &str, language_id: &str) {
        let key = (project_root.to_string(), language_id.to_string());
        if let Some(client) = self.clients.remove(&key) {
            let _ = client.shutdown();
        }
    }

    /// Stop all clients for a project.
    pub fn stop_project(&mut self, project_root: &str) {
        let keys: Vec<_> = self.clients.keys()
            .filter(|(root, _)| root == project_root)
            .cloned()
            .collect();
        for key in keys {
            if let Some(client) = self.clients.remove(&key) {
                let _ = client.shutdown();
            }
        }
    }

    /// Stop all clients.
    pub fn stop_all(&mut self) {
        for (_, client) in self.clients.drain() {
            let _ = client.shutdown();
        }
    }
}

/// Convert a filesystem path to a file:// URI.
pub fn path_to_uri(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    if normalized.starts_with('/') {
        format!("file://{}", normalized)
    } else {
        format!("file:///{}", normalized)
    }
}

/// Convert a file:// URI back to a filesystem path.
pub fn uri_to_path(uri: &str) -> String {
    let path = uri
        .strip_prefix("file:///")
        .or_else(|| uri.strip_prefix("file://"))
        .unwrap_or(uri);

    // On Windows, the path might start with a drive letter like C:/
    #[cfg(target_os = "windows")]
    {
        path.to_string()
    }
    #[cfg(not(target_os = "windows"))]
    {
        format!("/{}", path)
    }
}

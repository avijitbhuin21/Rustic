use anyhow::Result;
use lsp_types::*;
use lsp_types::{HoverContents, MarkedString};
use serde_json::json;

use super::transport::StdioTransport;

/// A client for communicating with a single language server.
pub struct LspClient {
    transport: StdioTransport,
    server_capabilities: Option<ServerCapabilities>,
    language_id: String,
}

impl LspClient {
    pub fn start(
        command: &str,
        args: &[String],
        root_uri: &str,
        language_id: &str,
    ) -> Result<Self> {
        let transport = StdioTransport::start(command, args)?;
        let mut client = Self {
            transport,
            server_capabilities: None,
            language_id: language_id.to_string(),
        };
        client.initialize(root_uri)?;
        Ok(client)
    }

    fn initialize(&mut self, root_uri: &str) -> Result<()> {
        let params = json!({
            "processId": std::process::id(),
            "rootUri": root_uri,
            "capabilities": {
                "textDocument": {
                    "synchronization": {
                        "dynamicRegistration": false,
                        "willSave": false,
                        "willSaveWaitUntil": false,
                        "didSave": true
                    },
                    "completion": {
                        "completionItem": {
                            "snippetSupport": false,
                            "documentationFormat": ["plaintext", "markdown"]
                        }
                    },
                    "hover": {
                        "contentFormat": ["plaintext", "markdown"]
                    },
                    "definition": {
                        "dynamicRegistration": false
                    },
                    "publishDiagnostics": {
                        "relatedInformation": true
                    },
                    "formatting": {
                        "dynamicRegistration": false
                    }
                }
            }
        });

        let result = self.transport.send_request("initialize", params)?;
        if let Ok(init_result) = serde_json::from_value::<InitializeResult>(result) {
            self.server_capabilities = Some(init_result.capabilities);
        }

        // Send initialized notification
        self.transport.send_notification("initialized", json!({}))?;

        Ok(())
    }

    pub fn language_id(&self) -> &str {
        &self.language_id
    }

    pub fn capabilities(&self) -> Option<&ServerCapabilities> {
        self.server_capabilities.as_ref()
    }

    // --- Text synchronization ---

    pub fn did_open(&self, uri: &str, language_id: &str, version: i32, text: &str) -> Result<()> {
        self.transport.send_notification("textDocument/didOpen", json!({
            "textDocument": {
                "uri": uri,
                "languageId": language_id,
                "version": version,
                "text": text
            }
        }))
    }

    pub fn did_change(&self, uri: &str, version: i32, text: &str) -> Result<()> {
        // Full document sync (simplest approach)
        self.transport.send_notification("textDocument/didChange", json!({
            "textDocument": { "uri": uri, "version": version },
            "contentChanges": [{ "text": text }]
        }))
    }

    pub fn did_save(&self, uri: &str, text: Option<&str>) -> Result<()> {
        let mut params = json!({ "textDocument": { "uri": uri } });
        if let Some(t) = text {
            params["text"] = json!(t);
        }
        self.transport.send_notification("textDocument/didSave", params)
    }

    pub fn did_close(&self, uri: &str) -> Result<()> {
        self.transport.send_notification("textDocument/didClose", json!({
            "textDocument": { "uri": uri }
        }))
    }

    // --- Completions ---

    pub fn completion(&self, uri: &str, line: u32, character: u32) -> Result<Vec<CompletionItem>> {
        let result = self.transport.send_request("textDocument/completion", json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": character }
        }))?;

        // Result can be CompletionList or Vec<CompletionItem>
        if let Ok(list) = serde_json::from_value::<CompletionList>(result.clone()) {
            return Ok(list.items);
        }
        if let Ok(items) = serde_json::from_value::<Vec<CompletionItem>>(result) {
            return Ok(items);
        }
        Ok(Vec::new())
    }

    // --- Hover ---

    pub fn hover(&self, uri: &str, line: u32, character: u32) -> Result<Option<Hover>> {
        let result = self.transport.send_request("textDocument/hover", json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": character }
        }))?;

        if result.is_null() {
            return Ok(None);
        }
        Ok(serde_json::from_value(result).ok())
    }

    // --- Go to definition ---

    pub fn goto_definition(&self, uri: &str, line: u32, character: u32) -> Result<Vec<Location>> {
        let result = self.transport.send_request("textDocument/definition", json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": character }
        }))?;

        if result.is_null() {
            return Ok(Vec::new());
        }
        // Can be Location, Vec<Location>, or Vec<LocationLink>
        if let Ok(loc) = serde_json::from_value::<Location>(result.clone()) {
            return Ok(vec![loc]);
        }
        if let Ok(locs) = serde_json::from_value::<Vec<Location>>(result) {
            return Ok(locs);
        }
        Ok(Vec::new())
    }

    // --- Formatting ---

    pub fn format(&self, uri: &str, tab_size: u32, insert_spaces: bool) -> Result<Vec<TextEdit>> {
        let result = self.transport.send_request("textDocument/formatting", json!({
            "textDocument": { "uri": uri },
            "options": {
                "tabSize": tab_size,
                "insertSpaces": insert_spaces
            }
        }))?;

        if result.is_null() {
            return Ok(Vec::new());
        }
        Ok(serde_json::from_value(result).unwrap_or_default())
    }

    // --- Simplified helpers (no lsp_types exposure) ---

    /// Get hover info as a plain string.
    pub fn hover_string(&self, uri: &str, line: u32, character: u32) -> Result<Option<String>> {
        let hover = self.hover(uri, line, character)?;
        Ok(hover.map(|h| {
            match h.contents {
                HoverContents::Scalar(s) => match s {
                    MarkedString::String(s) => s,
                    MarkedString::LanguageString(ls) => ls.value,
                },
                HoverContents::Array(arr) => arr
                    .into_iter()
                    .map(|s| match s {
                        MarkedString::String(s) => s,
                        MarkedString::LanguageString(ls) => {
                            format!("```{}\n{}\n```", ls.language, ls.value)
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n"),
                HoverContents::Markup(m) => m.value,
            }
        }))
    }

    /// Completion items as simplified tuples: (label, kind_str, detail, insert_text).
    pub fn completion_simple(
        &self, uri: &str, line: u32, character: u32,
    ) -> Result<Vec<(String, String, Option<String>, Option<String>)>> {
        let items = self.completion(uri, line, character)?;
        Ok(items.into_iter().map(|item| {
            let kind = item.kind.map(|k| format!("{:?}", k)).unwrap_or_else(|| "Text".into());
            (item.label, kind, item.detail, item.insert_text)
        }).collect())
    }

    /// Go to definition returning simplified tuples: (uri_string, line, character).
    pub fn goto_definition_simple(
        &self, uri: &str, line: u32, character: u32,
    ) -> Result<Vec<(String, u32, u32)>> {
        let locs = self.goto_definition(uri, line, character)?;
        Ok(locs.into_iter().map(|loc| {
            (loc.uri.to_string(), loc.range.start.line, loc.range.start.character)
        }).collect())
    }

    /// Format returning simplified tuples: (start_line, start_col, end_line, end_col, new_text).
    pub fn format_simple(
        &self, uri: &str, tab_size: u32, insert_spaces: bool,
    ) -> Result<Vec<(u32, u32, u32, u32, String)>> {
        let edits = self.format(uri, tab_size, insert_spaces)?;
        Ok(edits.into_iter().map(|e| {
            (e.range.start.line, e.range.start.character,
             e.range.end.line, e.range.end.character, e.new_text)
        }).collect())
    }

    // --- Shutdown ---

    pub fn shutdown(&self) -> Result<()> {
        let _ = self.transport.send_request("shutdown", json!(null));
        let _ = self.transport.send_notification("exit", json!(null));
        Ok(())
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

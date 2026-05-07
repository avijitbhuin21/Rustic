use super::config::{McpServerConfig, McpTransport};
use crate::provider::ToolDef;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

static REQUEST_ID: AtomicU64 = AtomicU64::new(1);

/// MCP client that communicates with an MCP server via JSON-RPC 2.0.
/// Currently supports stdio transport.
pub struct McpClient {
    #[allow(dead_code)]
    config: McpServerConfig,
    child: Option<Child>,
    stdin: Option<Box<dyn Write + Send>>,
    reader: Option<BufReader<Box<dyn std::io::Read + Send>>>,
}

#[derive(Serialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    id: u64,
    method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<Value>,
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct JsonRpcResponse {
    jsonrpc: String,
    id: Option<u64>,
    result: Option<Value>,
    error: Option<JsonRpcError>,
}

#[derive(Deserialize)]
struct JsonRpcError {
    code: i64,
    message: String,
}

impl McpClient {
    pub fn connect(config: McpServerConfig) -> Result<Self> {
        match &config.transport {
            McpTransport::Stdio { command, args, env } => {
                let mut cmd = Command::new(command);
                cmd.args(args)
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null());
                for (k, v) in env {
                    cmd.env(k, v);
                }
                // Suppress the console-window flash on Windows. GUI Tauri
                // processes have no console, so child stdio servers (npx, uvx,
                // node, …) briefly pop one open without this.
                #[cfg(windows)]
                {
                    use std::os::windows::process::CommandExt;
                    cmd.creation_flags(0x0800_0000);
                }

                let mut child = cmd.spawn()?;
                let stdin = child.stdin.take().map(|s| Box::new(s) as Box<dyn Write + Send>);
                let stdout = child.stdout.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>);
                let reader = stdout.map(BufReader::new);

                let mut client = Self {
                    config,
                    child: Some(child),
                    stdin,
                    reader,
                };

                // Send initialize
                client.initialize()?;
                Ok(client)
            }
            McpTransport::Sse { url: _, headers: _ } => {
                // SSE transport is more complex — defer to a basic stub
                Err(anyhow::anyhow!("SSE transport not yet implemented"))
            }
        }
    }

    fn initialize(&mut self) -> Result<()> {
        let _resp = self.send_request(
            "initialize",
            Some(json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {
                    "name": "Rustic",
                    "version": "0.1.0"
                }
            })),
        )?;

        // Send initialized notification
        self.send_notification("notifications/initialized", None)?;
        Ok(())
    }

    pub fn list_tools(&mut self) -> Result<Vec<ToolDef>> {
        let resp = self.send_request("tools/list", None)?;

        let tools_value = resp
            .get("tools")
            .cloned()
            .unwrap_or(json!([]));

        let tools: Vec<McpTool> = serde_json::from_value(tools_value)?;
        Ok(tools
            .into_iter()
            .map(|t| ToolDef {
                name: t.name,
                description: t.description.unwrap_or_default(),
                parameters: t.input_schema.unwrap_or(json!({"type": "object", "properties": {}})),
            })
            .collect())
    }

    pub fn call_tool(&mut self, name: &str, arguments: Value) -> Result<Value> {
        let resp = self.send_request(
            "tools/call",
            Some(json!({
                "name": name,
                "arguments": arguments,
            })),
        )?;
        Ok(resp)
    }

    pub fn disconnect(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    fn send_request(&mut self, method: &str, params: Option<Value>) -> Result<Value> {
        let id = REQUEST_ID.fetch_add(1, Ordering::Relaxed);
        let request = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id,
            method: method.into(),
            params,
        };

        let msg = serde_json::to_string(&request)? + "\n";

        let stdin = self.stdin.as_mut().ok_or(anyhow::anyhow!("No stdin"))?;
        stdin.write_all(msg.as_bytes())?;
        stdin.flush()?;

        // Read response
        let reader = self.reader.as_mut().ok_or(anyhow::anyhow!("No stdout"))?;
        let mut line = String::new();
        reader.read_line(&mut line)?;

        let resp: JsonRpcResponse = serde_json::from_str(&line)?;

        if let Some(err) = resp.error {
            return Err(anyhow::anyhow!("MCP error {}: {}", err.code, err.message));
        }

        resp.result.ok_or(anyhow::anyhow!("No result in response"))
    }

    fn send_notification(&mut self, method: &str, params: Option<Value>) -> Result<()> {
        let msg = if let Some(p) = params {
            json!({"jsonrpc": "2.0", "method": method, "params": p})
        } else {
            json!({"jsonrpc": "2.0", "method": method})
        };

        let data = serde_json::to_string(&msg)? + "\n";
        let stdin = self.stdin.as_mut().ok_or(anyhow::anyhow!("No stdin"))?;
        stdin.write_all(data.as_bytes())?;
        stdin.flush()?;
        Ok(())
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        self.disconnect();
    }
}

#[derive(Deserialize)]
struct McpTool {
    name: String,
    description: Option<String>,
    #[serde(rename = "inputSchema")]
    input_schema: Option<Value>,
}

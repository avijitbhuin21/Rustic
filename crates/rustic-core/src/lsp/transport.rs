use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Mutex;

/// JSON-RPC transport over stdio to a language server process.
pub struct StdioTransport {
    child: Child,
    writer: Mutex<ChildStdin>,
    reader: Mutex<BufReader<ChildStdout>>,
    next_id: AtomicI64,
}

impl StdioTransport {
    pub fn start(command: &str, args: &[String]) -> Result<Self> {
        let mut child = Command::new(command)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("Failed to start language server: {}", command))?;

        let stdin = child.stdin.take().context("No stdin on child process")?;
        let stdout = child.stdout.take().context("No stdout on child process")?;

        Ok(Self {
            child,
            writer: Mutex::new(stdin),
            reader: Mutex::new(BufReader::new(stdout)),
            next_id: AtomicI64::new(1),
        })
    }

    /// Send a JSON-RPC request and return the response.
    pub fn send_request(&self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });

        self.send_message(&request)?;

        // Read responses until we get one matching our ID
        loop {
            let msg = self.read_message()?;
            if let Some(msg_id) = msg.get("id") {
                if msg_id.as_i64() == Some(id) {
                    if let Some(error) = msg.get("error") {
                        bail!("LSP error: {}", error);
                    }
                    return Ok(msg.get("result").cloned().unwrap_or(Value::Null));
                }
            }
            // It's a notification — skip it for now (handled separately)
        }
    }

    /// Send a JSON-RPC notification (no response expected).
    pub fn send_notification(&self, method: &str, params: Value) -> Result<()> {
        let notification = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        self.send_message(&notification)
    }

    /// Try to read a message from stdout (blocking).
    pub fn read_message(&self) -> Result<Value> {
        let mut reader = self.reader.lock().unwrap();

        // Read headers
        let mut content_length: usize = 0;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line)?;
            let line = line.trim();
            if line.is_empty() {
                break;
            }
            if let Some(len) = line.strip_prefix("Content-Length: ") {
                content_length = len.parse().context("Invalid Content-Length")?;
            }
        }

        if content_length == 0 {
            bail!("Empty Content-Length in LSP message");
        }

        // Read body
        let mut body = vec![0u8; content_length];
        reader.read_exact(&mut body)?;
        let msg: Value = serde_json::from_slice(&body)?;
        Ok(msg)
    }

    fn send_message(&self, msg: &Value) -> Result<()> {
        let body = serde_json::to_string(msg)?;
        let header = format!("Content-Length: {}\r\n\r\n", body.len());

        let mut writer = self.writer.lock().unwrap();
        writer.write_all(header.as_bytes())?;
        writer.write_all(body.as_bytes())?;
        writer.flush()?;
        Ok(())
    }

    pub fn kill(&mut self) -> Result<()> {
        let _ = self.child.kill();
        let _ = self.child.wait();
        Ok(())
    }
}

impl Drop for StdioTransport {
    fn drop(&mut self) {
        let _ = self.kill();
    }
}

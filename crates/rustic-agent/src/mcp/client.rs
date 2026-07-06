use super::config::{McpServerConfig, McpTransport};
use crate::provider::ToolDef;
use anyhow::Result;
use reqwest::blocking::Client as HttpClient;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

/// F-19 (Medium): cap on the size of a single JSON-RPC message from an MCP
/// server. Legitimate MCP responses (tools/list, tool result blocks) sit well
/// under a few hundred KiB; 16 MiB is well above that and well below "the
/// host runs out of RAM." A compromised server emitting a never-ending line
/// without a newline would otherwise grow the buffer unbounded.
const MCP_MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;

/// Per-request deadline applied INSIDE the client (5.4/8.1). Stdio requests
/// wait on the reader-thread channel with this timeout; remote POSTs use it as
/// the reqwest per-request timeout. A deadline that fires here leaves the
/// client in a *known* state: the client marks itself broken so callers stop
/// reusing a desynced stream, and late replies to timed-out request ids are
/// discarded by id-matching instead of being returned for the next call.
pub const MCP_REQUEST_TIMEOUT_SECS: u64 = 300;

/// Remote (Streamable HTTP / legacy SSE) transport timeouts: 30s to establish
/// a TCP+TLS connection, `MCP_REQUEST_TIMEOUT_SECS` for any single JSON-RPC
/// request round-trip.
const REMOTE_CONNECT_TIMEOUT_SECS: u64 = 30;
const REMOTE_REQUEST_TIMEOUT_SECS: u64 = MCP_REQUEST_TIMEOUT_SECS;

static REQUEST_ID: AtomicU64 = AtomicU64::new(1);

/// MCP client that communicates with an MCP server via JSON-RPC 2.0.
/// Supports the stdio transport and the remote HTTP transports (Streamable
/// HTTP per the 2025-03-26 spec, with automatic fallback to the legacy
/// HTTP+SSE transport from the 2024-11-05 spec).
pub struct McpClient {
    #[allow(dead_code)]
    config: McpServerConfig,
    transport: ClientTransport,
    /// Set when a request timed out or the transport stream died/desynced.
    /// A broken client must not be reused — the owner (McpManager) drops it
    /// and reconnects lazily. See `is_broken`.
    broken: bool,
}

enum ClientTransport {
    Stdio {
        child: Option<Child>,
        stdin: Option<Box<dyn Write + Send>>,
        /// Complete JSON-RPC lines from the dedicated stdout reader thread.
        /// Reading through a channel (instead of blocking on the pipe
        /// directly) is what makes a per-request deadline possible: the
        /// request loop waits with `recv_timeout` and, on a late reply, can
        /// discard messages whose id doesn't match the in-flight request.
        rx: Option<std::sync::mpsc::Receiver<Result<String>>>,
    },
    Remote(RemoteTransport),
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
    /// `Value` rather than `u64`: JSON-RPC allows string ids and some remote
    /// servers echo numeric ids back as strings. `id_matches` normalizes.
    id: Option<Value>,
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

                // F-12: do NOT let the MCP child inherit the Tauri parent's
                // environment. A user who launched Rustic from a shell with
                // ANTHROPIC_API_KEY / OPENAI_API_KEY / AWS_SECRET_ACCESS_KEY
                // exported would otherwise hand those secrets to whatever
                // command the .mcp.json names. Start from an empty env and
                // re-add only the small baseline children actually need
                // (PATH so npx/uvx/node can resolve, and a working-dir hint).
                cmd.env_clear();
                for var in [
                    "PATH",
                    "SystemRoot",
                    "USERPROFILE",
                    "HOME",
                    "TEMP",
                    "TMP",
                    "LANG",
                ] {
                    if let Ok(v) = std::env::var(var) {
                        cmd.env(var, v);
                    }
                }
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
                let stdin = child
                    .stdin
                    .take()
                    .map(|s| Box::new(s) as Box<dyn Write + Send>);
                let stdout = child
                    .stdout
                    .take()
                    .map(|s| Box::new(s) as Box<dyn std::io::Read + Send>);
                // Dedicated reader thread: forwards complete lines over a
                // channel so `send_request` can enforce a per-request deadline
                // with `recv_timeout` and discard id-mismatched late replies.
                // The thread exits on EOF/error (child killed on disconnect)
                // or when the receiver is dropped with the client.
                let rx = stdout.map(|s| {
                    let (tx, rx) = std::sync::mpsc::channel::<Result<String>>();
                    let mut reader = BufReader::new(s);
                    std::thread::spawn(move || loop {
                        match read_bounded_line(&mut reader, MCP_MAX_MESSAGE_BYTES) {
                            Ok(line) => {
                                if tx.send(Ok(line)).is_err() {
                                    break; // client dropped
                                }
                            }
                            Err(e) => {
                                let _ = tx.send(Err(e));
                                break;
                            }
                        }
                    });
                    rx
                });

                let mut client = Self {
                    config,
                    transport: ClientTransport::Stdio {
                        child: Some(child),
                        stdin,
                        rx,
                    },
                    broken: false,
                };

                // Send initialize
                client.initialize()?;
                Ok(client)
            }
            McpTransport::Sse { url, headers } => {
                // Remote transport (config types "sse" and "http"). Probes
                // Streamable HTTP (2025-03-26 spec) first; falls back to the
                // legacy HTTP+SSE transport (2024-11-05 spec) when the server
                // rejects the POST probe with a 4xx. The full handshake
                // (initialize + notifications/initialized) runs inside
                // `RemoteTransport::connect`.
                let transport = RemoteTransport::connect(url.clone(), headers.clone())?;
                Ok(Self {
                    config,
                    transport: ClientTransport::Remote(transport),
                    broken: false,
                })
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

        let tools_value = resp.get("tools").cloned().unwrap_or(json!([]));

        let tools: Vec<McpTool> = serde_json::from_value(tools_value)?;
        Ok(tools
            .into_iter()
            .map(|t| ToolDef {
                name: t.name,
                description: t.description.unwrap_or_default(),
                parameters: t
                    .input_schema
                    .unwrap_or(json!({"type": "object", "properties": {}})),
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
        match &mut self.transport {
            ClientTransport::Stdio { child, .. } => {
                if let Some(mut child) = child.take() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
            ClientTransport::Remote(remote) => remote.close(),
        }
    }

    /// True when a previous request timed out or the transport stream
    /// died/desynced. A broken client's request/response pairing can no
    /// longer be trusted (a late reply could otherwise become the answer to
    /// the NEXT request — 8.1), so the owner must drop it and reconnect.
    pub fn is_broken(&self) -> bool {
        self.broken || matches!(&self.transport, ClientTransport::Remote(r) if r.broken)
    }

    fn send_request(&mut self, method: &str, params: Option<Value>) -> Result<Value> {
        if self.is_broken() {
            return Err(anyhow::anyhow!(
                "MCP client is broken (a previous request timed out or the stream desynced) — reconnect the server"
            ));
        }
        match &mut self.transport {
            ClientTransport::Stdio { stdin, rx, .. } => {
                let id = REQUEST_ID.fetch_add(1, Ordering::Relaxed);
                let request = JsonRpcRequest {
                    jsonrpc: "2.0".into(),
                    id,
                    method: method.into(),
                    params,
                };

                let msg = serde_json::to_string(&request)? + "\n";

                let stdin = stdin.as_mut().ok_or(anyhow::anyhow!("No stdin"))?;
                if let Err(e) = stdin.write_all(msg.as_bytes()).and_then(|_| stdin.flush()) {
                    self.broken = true;
                    return Err(e.into());
                }

                // Per-request deadline (5.4/8.1). Wait on the reader-thread
                // channel with a timeout instead of blocking on the pipe:
                // - a timeout marks the client broken (the reply that
                //   eventually arrives must not be paired with the NEXT
                //   request) and returns promptly, releasing the per-client
                //   mutex so the owner can drop/reconnect;
                // - replies are matched by JSON-RPC id, so a late reply to an
                //   earlier timed-out id is discarded, never returned here.
                let rx = rx.as_ref().ok_or(anyhow::anyhow!("No stdout"))?;
                let deadline =
                    std::time::Instant::now() + Duration::from_secs(MCP_REQUEST_TIMEOUT_SECS);
                loop {
                    let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                    if remaining.is_zero() {
                        self.broken = true;
                        return Err(anyhow::anyhow!(
                            "MCP request '{}' timed out after {}s — connection marked broken; it will be re-established",
                            method,
                            MCP_REQUEST_TIMEOUT_SECS
                        ));
                    }
                    match rx.recv_timeout(remaining) {
                        Ok(Ok(line)) => {
                            // F-19 cap is enforced by the reader thread.
                            let Ok(resp) = serde_json::from_str::<JsonRpcResponse>(&line) else {
                                continue; // unparseable / non-response line
                            };
                            if resp.id.is_none() {
                                continue; // server notification — skip
                            }
                            if !id_matches(&resp.id, id) {
                                // Late reply to a previously timed-out
                                // request (or a server-initiated request) —
                                // discard instead of mis-attributing it to
                                // this call (8.1).
                                tracing::debug!(
                                    got = %resp.id.as_ref().map(|v| v.to_string()).unwrap_or_default(),
                                    want = id,
                                    "MCP stdio: discarding id-mismatched message (late reply to a timed-out request?)"
                                );
                                continue;
                            }
                            if resp.result.is_none() && resp.error.is_none() {
                                continue;
                            }
                            return unwrap_rpc(resp);
                        }
                        Ok(Err(e)) => {
                            // Reader thread hit EOF / IO error / F-19 cap —
                            // the pipe is dead.
                            self.broken = true;
                            return Err(e);
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                            self.broken = true;
                            return Err(anyhow::anyhow!(
                                "MCP request '{}' timed out after {}s — connection marked broken; it will be re-established",
                                method,
                                MCP_REQUEST_TIMEOUT_SECS
                            ));
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                            self.broken = true;
                            return Err(anyhow::anyhow!("MCP server closed stdout"));
                        }
                    }
                }
            }
            ClientTransport::Remote(remote) => remote.send_request(method, params),
        }
    }

    fn send_notification(&mut self, method: &str, params: Option<Value>) -> Result<()> {
        let msg = if let Some(p) = params {
            json!({"jsonrpc": "2.0", "method": method, "params": p})
        } else {
            json!({"jsonrpc": "2.0", "method": method})
        };

        match &mut self.transport {
            ClientTransport::Stdio { stdin, .. } => {
                let data = serde_json::to_string(&msg)? + "\n";
                let stdin = stdin.as_mut().ok_or(anyhow::anyhow!("No stdin"))?;
                if let Err(e) = stdin.write_all(data.as_bytes()).and_then(|_| stdin.flush()) {
                    self.broken = true;
                    return Err(e.into());
                }
                Ok(())
            }
            ClientTransport::Remote(remote) => remote.send_notification(&msg),
        }
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

/// Read bytes up to (and including) the next `\n` from `reader`, returning
/// the decoded string. Errors out if `max_bytes` is reached without a newline
/// — guards against an unbounded `BufRead::read_line` allocation when the
/// peer never terminates the message (F-19).
fn read_bounded_line<R: Read>(reader: &mut R, max_bytes: usize) -> Result<String> {
    read_bounded_line_msg(reader, max_bytes, "MCP server closed stdout")
}

/// Same as `read_bounded_line`, with a caller-supplied EOF error message so
/// the remote transport doesn't report "closed stdout" for an HTTP stream.
fn read_bounded_line_msg<R: Read>(
    reader: &mut R,
    max_bytes: usize,
    eof_msg: &str,
) -> Result<String> {
    let mut buf = Vec::with_capacity(1024);
    let mut byte = [0u8; 1];
    loop {
        match reader.read(&mut byte) {
            Ok(0) => {
                // EOF before newline.
                if buf.is_empty() {
                    return Err(anyhow::anyhow!("{}", eof_msg));
                }
                return Ok(String::from_utf8_lossy(&buf).into_owned());
            }
            Ok(_) => {
                buf.push(byte[0]);
                if byte[0] == b'\n' {
                    return Ok(String::from_utf8_lossy(&buf).into_owned());
                }
                if buf.len() >= max_bytes {
                    return Err(anyhow::anyhow!(
                        "MCP message exceeded {} byte cap without terminator (F-19)",
                        max_bytes
                    ));
                }
            }
            Err(e) => return Err(e.into()),
        }
    }
}

// ---------------------------------------------------------------------------
// Remote transport: Streamable HTTP (2025-03-26) + legacy HTTP+SSE (2024-11-05)
// ---------------------------------------------------------------------------

/// Remote MCP transport over HTTP. Two wire modes:
///
/// - **Streamable HTTP** (current spec, 2025-03-26): every JSON-RPC message is
///   POSTed to the server URL; the response is either `application/json` (one
///   response body) or `text/event-stream` (SSE events read until the response
///   with the matching id arrives). The `Mcp-Session-Id` header returned by
///   `initialize` is echoed on all subsequent requests.
/// - **Legacy HTTP+SSE** (2024-11-05): a long-lived GET on the server URL
///   yields an `endpoint` event naming the POST URL; requests are POSTed there
///   and responses arrive on the long-lived stream, matched by id.
///
/// Detection: the Streamable HTTP `initialize` POST is tried first; a 4xx
/// (typically 404/405 from a legacy server that only accepts GET) triggers the
/// legacy fallback. Network-level failures are fatal, not fallback.
struct RemoteTransport {
    http: HttpClient,
    base_url: String,
    /// User-configured headers (auth tokens etc.), sent on every request.
    headers: HashMap<String, String>,
    /// Streamable HTTP session id from the `initialize` response, if issued.
    session_id: Option<String>,
    mode: RemoteMode,
    /// Set when the legacy long-lived SSE stream dies mid-session: responses
    /// for in-flight and future requests can no longer arrive, so the client
    /// must be dropped and reconnected. Streamable-HTTP request failures are
    /// per-request and do NOT set this.
    broken: bool,
}

enum RemoteMode {
    Streamable,
    Legacy {
        post_url: String,
        /// The long-lived GET event stream responses arrive on.
        stream: SseStream<reqwest::blocking::Response>,
    },
}

impl RemoteTransport {
    fn connect(url: String, headers: HashMap<String, String>) -> Result<Self> {
        let http = shared_http_client()?;

        let init_id = REQUEST_ID.fetch_add(1, Ordering::Relaxed);
        let init_req = json!({
            "jsonrpc": "2.0",
            "id": init_id,
            "method": "initialize",
            "params": initialize_params("2025-03-26"),
        });

        match post_json(&http, &url, &headers, None, &init_req) {
            Ok(resp) if resp.status().is_success() => {
                // Streamable HTTP. Session id (if any) arrives on the
                // initialize response headers, before the body is consumed.
                let session_id = resp
                    .headers()
                    .get("mcp-session-id")
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned);
                let rpc = read_jsonrpc_from_response(resp, init_id)?;
                let _ = unwrap_rpc(rpc)?;
                let mut transport = Self {
                    http,
                    base_url: url,
                    headers,
                    session_id,
                    mode: RemoteMode::Streamable,
                    broken: false,
                };
                transport.send_notification(&json!({
                    "jsonrpc": "2.0",
                    "method": "notifications/initialized",
                }))?;
                Ok(transport)
            }
            Ok(resp) if resp.status().is_client_error() => {
                // 4xx on the POST probe → assume a legacy HTTP+SSE server.
                Self::connect_legacy(http, url, headers)
            }
            Ok(resp) => Err(anyhow::anyhow!(
                "MCP remote server '{}' returned HTTP {} during initialize",
                url,
                resp.status()
            )),
            Err(e) => Err(anyhow::anyhow!(
                "MCP remote connect to '{}' failed: {}",
                url,
                e
            )),
        }
    }

    /// Legacy HTTP+SSE (2024-11-05): GET the base URL with
    /// `Accept: text/event-stream`, wait for the `endpoint` event naming the
    /// POST URL, then run the initialize handshake through that pair.
    fn connect_legacy(
        http: HttpClient,
        url: String,
        headers: HashMap<String, String>,
    ) -> Result<Self> {
        let mut builder = http.get(&url).header("Accept", "text/event-stream");
        for (k, v) in &headers {
            builder = builder.header(k.as_str(), v.as_str());
        }
        // No whole-request timeout: this stream lives for the session (the
        // shared client's default timeout is disabled). Silent-peer hangs are
        // bounded one layer up — the executor wraps every MCP call in a 300s
        // tokio timeout.
        let resp = builder
            .send()
            .map_err(|e| anyhow::anyhow!("MCP legacy SSE connect to '{}' failed: {}", url, e))?;
        if !resp.status().is_success() {
            return Err(anyhow::anyhow!(
                "MCP legacy SSE connect to '{}' failed: HTTP {}",
                url,
                resp.status()
            ));
        }

        let mut stream = SseStream::new(resp);
        let post_url = loop {
            let ev = stream.next_event()?;
            if ev.event == "endpoint" {
                break resolve_endpoint(&url, &ev.data)?;
            }
        };

        let mut transport = Self {
            http,
            base_url: url,
            headers,
            session_id: None,
            mode: RemoteMode::Legacy { post_url, stream },
            broken: false,
        };
        let _ = transport.send_request("initialize", Some(initialize_params("2024-11-05")))?;
        transport.send_notification(&json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
        }))?;
        Ok(transport)
    }

    fn send_request(&mut self, method: &str, params: Option<Value>) -> Result<Value> {
        let id = REQUEST_ID.fetch_add(1, Ordering::Relaxed);
        let request = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id,
            method: method.into(),
            params,
        };
        let body = serde_json::to_value(&request)?;

        match &mut self.mode {
            RemoteMode::Streamable => {
                let resp = post_json(
                    &self.http,
                    &self.base_url,
                    &self.headers,
                    self.session_id.as_deref(),
                    &body,
                )?;
                let status = resp.status();
                if !status.is_success() {
                    return Err(anyhow::anyhow!(
                        "MCP remote request '{}' failed: HTTP {}",
                        method,
                        status
                    ));
                }
                unwrap_rpc(read_jsonrpc_from_response(resp, id)?)
            }
            RemoteMode::Legacy { post_url, stream } => {
                let resp = post_json(&self.http, post_url, &self.headers, None, &body)?;
                let status = resp.status();
                if !status.is_success() {
                    return Err(anyhow::anyhow!(
                        "MCP legacy SSE request '{}' failed: HTTP {}",
                        method,
                        status
                    ));
                }
                // The actual response arrives on the long-lived GET stream,
                // matched by request id. Server-initiated notifications and
                // requests interleaved on the stream are skipped. A stream
                // error means no in-flight/future response can ever arrive —
                // mark the transport broken so the client is reconnected
                // rather than reused (8.1).
                loop {
                    let ev = match stream.next_event() {
                        Ok(ev) => ev,
                        Err(e) => {
                            self.broken = true;
                            return Err(e);
                        }
                    };
                    if ev.event != "message" {
                        continue;
                    }
                    if let Ok(rpc) = serde_json::from_str::<JsonRpcResponse>(&ev.data) {
                        if id_matches(&rpc.id, id) && (rpc.result.is_some() || rpc.error.is_some())
                        {
                            return unwrap_rpc(rpc);
                        }
                    }
                }
            }
        }
    }

    fn send_notification(&mut self, msg: &Value) -> Result<()> {
        let (url, session) = match &self.mode {
            RemoteMode::Streamable => (self.base_url.clone(), self.session_id.clone()),
            RemoteMode::Legacy { post_url, .. } => (post_url.clone(), None),
        };
        let resp = post_json(&self.http, &url, &self.headers, session.as_deref(), msg)?;
        if !resp.status().is_success() {
            return Err(anyhow::anyhow!(
                "MCP remote notification failed: HTTP {}",
                resp.status()
            ));
        }
        Ok(())
    }

    /// Best-effort session teardown. Streamable HTTP sessions are explicitly
    /// terminated with an HTTP DELETE carrying the session id (servers MAY
    /// respond 405 if they don't support client-initiated teardown — ignored).
    fn close(&mut self) {
        if !matches!(self.mode, RemoteMode::Streamable) {
            return;
        }
        let Some(sid) = self.session_id.take() else {
            return;
        };
        let mut builder = self
            .http
            .delete(&self.base_url)
            .header("Mcp-Session-Id", sid)
            .timeout(Duration::from_secs(10));
        for (k, v) in &self.headers {
            builder = builder.header(k.as_str(), v.as_str());
        }
        let _ = builder.send();
    }
}

/// One process-wide blocking HTTP client shared by all remote MCP transports.
/// Cheap to clone (Arc inside); never dropped, so its internal runtime thread
/// is never torn down from an awkward context.
fn shared_http_client() -> Result<HttpClient> {
    static CLIENT: OnceLock<HttpClient> = OnceLock::new();
    if let Some(c) = CLIENT.get() {
        return Ok(c.clone());
    }
    let built = HttpClient::builder()
        .connect_timeout(Duration::from_secs(REMOTE_CONNECT_TIMEOUT_SECS))
        // The blocking client applies a 30s whole-request timeout by default,
        // which would kill the legacy long-lived GET event stream. Disable it
        // here; every POST sets its own per-request 300s timeout instead. The
        // legacy stream's silent-server hang is bounded one layer up: the
        // executor wraps every MCP call in a 300s tokio timeout (the blocking
        // reqwest builder does not expose read_timeout).
        .timeout(None::<Duration>)
        .build()?;
    Ok(CLIENT.get_or_init(|| built).clone())
}

/// POST a JSON-RPC message. Sets the spec-required
/// `Accept: application/json, text/event-stream`, echoes the session id when
/// present, and applies the user's configured headers last.
fn post_json(
    http: &HttpClient,
    url: &str,
    extra_headers: &HashMap<String, String>,
    session_id: Option<&str>,
    body: &Value,
) -> Result<reqwest::blocking::Response> {
    let mut builder = http
        .post(url)
        .header("Accept", "application/json, text/event-stream")
        .timeout(Duration::from_secs(REMOTE_REQUEST_TIMEOUT_SECS))
        .json(body);
    if let Some(sid) = session_id {
        builder = builder.header("Mcp-Session-Id", sid);
    }
    for (k, v) in extra_headers {
        builder = builder.header(k.as_str(), v.as_str());
    }
    Ok(builder.send()?)
}

/// Extract the JSON-RPC response for `want_id` from a Streamable HTTP
/// response body — either a single `application/json` body or a
/// `text/event-stream` of JSON-RPC messages read until the matching id.
fn read_jsonrpc_from_response(
    resp: reqwest::blocking::Response,
    want_id: u64,
) -> Result<JsonRpcResponse> {
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();

    if content_type.starts_with("text/event-stream") {
        let mut stream = SseStream::new(resp);
        loop {
            let ev = stream.next_event()?;
            if ev.event != "message" {
                continue;
            }
            if let Ok(rpc) = serde_json::from_str::<JsonRpcResponse>(&ev.data) {
                if id_matches(&rpc.id, want_id) && (rpc.result.is_some() || rpc.error.is_some()) {
                    return Ok(rpc);
                }
            }
            // Anything else (server notifications/requests) is skipped.
        }
    } else {
        let bytes = read_capped_body(resp)?;
        Ok(serde_json::from_slice(&bytes)?)
    }
}

/// Read a whole response body, enforcing the F-19 message cap.
fn read_capped_body(resp: reqwest::blocking::Response) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    let mut limited = resp.take((MCP_MAX_MESSAGE_BYTES + 1) as u64);
    limited.read_to_end(&mut buf)?;
    if buf.len() > MCP_MAX_MESSAGE_BYTES {
        return Err(anyhow::anyhow!(
            "MCP message exceeded {} byte cap (F-19)",
            MCP_MAX_MESSAGE_BYTES
        ));
    }
    Ok(buf)
}

fn initialize_params(protocol_version: &str) -> Value {
    json!({
        "protocolVersion": protocol_version,
        "capabilities": {},
        "clientInfo": {
            "name": "Rustic",
            "version": "0.1.0"
        }
    })
}

fn unwrap_rpc(resp: JsonRpcResponse) -> Result<Value> {
    if let Some(err) = resp.error {
        return Err(anyhow::anyhow!("MCP error {}: {}", err.code, err.message));
    }
    resp.result.ok_or(anyhow::anyhow!("No result in response"))
}

/// True when a JSON-RPC response id equals the u64 id we sent. Tolerates
/// servers that echo numeric ids back as strings.
fn id_matches(id: &Option<Value>, want: u64) -> bool {
    match id {
        Some(Value::Number(n)) => n.as_u64() == Some(want),
        Some(Value::String(s)) => s.parse::<u64>().ok() == Some(want),
        _ => false,
    }
}

/// Resolve a legacy `endpoint` event payload against the stream's base URL.
/// The payload may be an absolute URL, an absolute path (`/messages?...`), or
/// a relative path.
fn resolve_endpoint(base: &str, endpoint: &str) -> Result<String> {
    let base_url = reqwest::Url::parse(base)
        .map_err(|e| anyhow::anyhow!("Invalid MCP base URL '{}': {}", base, e))?;
    let joined = base_url
        .join(endpoint.trim())
        .map_err(|e| anyhow::anyhow!("Invalid MCP endpoint '{}': {}", endpoint, e))?;
    Ok(joined.to_string())
}

// ---------------------------------------------------------------------------
// SSE framing (hand-rolled — WHATWG EventSource line protocol)
// ---------------------------------------------------------------------------

/// A single parsed Server-Sent Event.
#[derive(Debug, PartialEq)]
struct SseEvent {
    /// Resolved event type; an absent `event:` field defaults to "message".
    event: String,
    data: String,
}

/// Incremental SSE event assembler: feed one line at a time (trailing `\r`
/// tolerated for CRLF streams), get `Some(event)` back when a blank line
/// dispatches a completed event. `data:` lines accumulate joined by `\n`;
/// `id:`/`retry:` fields and `:` comment lines (keepalives) are ignored; a
/// blank line with no accumulated data dispatches nothing (per spec).
#[derive(Default)]
struct SseAssembler {
    event_type: String,
    data: String,
    has_data: bool,
}

impl SseAssembler {
    fn feed_line(&mut self, line: &str) -> Option<SseEvent> {
        let line = line.strip_suffix('\r').unwrap_or(line);

        if line.is_empty() {
            // Dispatch point. Empty data buffer → reset and skip (spec).
            if !self.has_data {
                self.event_type.clear();
                return None;
            }
            let event = if self.event_type.is_empty() {
                "message".to_string()
            } else {
                std::mem::take(&mut self.event_type)
            };
            self.event_type.clear();
            self.has_data = false;
            return Some(SseEvent {
                event,
                data: std::mem::take(&mut self.data),
            });
        }

        if line.starts_with(':') {
            return None; // comment / keepalive
        }

        let (field, value) = match line.split_once(':') {
            Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
            None => (line, ""),
        };
        match field {
            "event" => self.event_type = value.to_string(),
            "data" => {
                if self.has_data {
                    self.data.push('\n');
                }
                self.data.push_str(value);
                self.has_data = true;
            }
            _ => {} // id, retry, unknown fields — ignored
        }
        None
    }
}

/// Blocking SSE event reader over any `Read` (a reqwest response body in
/// production, a `Cursor` in tests). Enforces the F-19 cap on both individual
/// lines and the accumulated event payload.
struct SseStream<R: Read> {
    reader: R,
    assembler: SseAssembler,
}

impl<R: Read> SseStream<R> {
    fn new(reader: R) -> Self {
        Self {
            reader,
            assembler: SseAssembler::default(),
        }
    }

    /// Block until the next complete SSE event. Errors on EOF, I/O failure,
    /// or the byte cap.
    fn next_event(&mut self) -> Result<SseEvent> {
        loop {
            let raw = read_bounded_line_msg(
                &mut self.reader,
                MCP_MAX_MESSAGE_BYTES,
                "MCP remote server closed the event stream",
            )?;
            let line = raw.trim_end_matches('\n');
            if let Some(ev) = self.assembler.feed_line(line) {
                return Ok(ev);
            }
            if self.assembler.data.len() > MCP_MAX_MESSAGE_BYTES {
                return Err(anyhow::anyhow!(
                    "MCP SSE event exceeded {} byte cap without dispatch (F-19)",
                    MCP_MAX_MESSAGE_BYTES
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn events_from(input: &str) -> Vec<SseEvent> {
        let mut stream = SseStream::new(Cursor::new(input.as_bytes().to_vec()));
        let mut out = Vec::new();
        while let Ok(ev) = stream.next_event() {
            out.push(ev);
        }
        out
    }

    #[test]
    fn sse_simple_event() {
        let evs = events_from("data: hello\n\n");
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].event, "message");
        assert_eq!(evs[0].data, "hello");
    }

    #[test]
    fn sse_multiline_data_joined_with_newline() {
        let evs = events_from("data: line1\ndata: line2\n\n");
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].data, "line1\nline2");
    }

    #[test]
    fn sse_custom_event_type() {
        let evs = events_from("event: endpoint\ndata: /messages?sessionId=abc\n\n");
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].event, "endpoint");
        assert_eq!(evs[0].data, "/messages?sessionId=abc");
    }

    #[test]
    fn sse_comments_and_unknown_fields_ignored() {
        let evs = events_from(": keepalive\nid: 7\nretry: 100\ndata: x\n\n");
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].data, "x");
    }

    #[test]
    fn sse_crlf_line_endings() {
        let evs = events_from("event: endpoint\r\ndata: /post\r\n\r\n");
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].event, "endpoint");
        assert_eq!(evs[0].data, "/post");
    }

    #[test]
    fn sse_no_space_after_colon() {
        let evs = events_from("data:tight\n\n");
        assert_eq!(evs[0].data, "tight");
    }

    #[test]
    fn sse_blank_line_without_data_dispatches_nothing() {
        // "event: ping" followed by a blank line has no data → no dispatch,
        // and the pending event type must not leak into the next event.
        let evs = events_from("event: ping\n\ndata: real\n\n");
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].event, "message");
        assert_eq!(evs[0].data, "real");
    }

    #[test]
    fn sse_event_type_resets_between_events() {
        let evs = events_from("event: endpoint\ndata: a\n\ndata: b\n\n");
        assert_eq!(evs.len(), 2);
        assert_eq!(evs[0].event, "endpoint");
        assert_eq!(evs[1].event, "message");
    }

    #[test]
    fn sse_multiple_events_in_sequence() {
        let evs = events_from("data: one\n\ndata: two\n\ndata: three\n\n");
        let datas: Vec<&str> = evs.iter().map(|e| e.data.as_str()).collect();
        assert_eq!(datas, vec!["one", "two", "three"]);
    }

    #[test]
    fn sse_jsonrpc_response_matching() {
        // Typical legacy-stream interleaving: a notification (no id), then
        // the response we want.
        let input =
            "data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\",\"params\":{}}\n\n\
                     data: {\"jsonrpc\":\"2.0\",\"id\":42,\"result\":{\"ok\":true}}\n\n";
        let mut stream = SseStream::new(Cursor::new(input.as_bytes().to_vec()));
        let rpc = loop {
            let ev = stream.next_event().unwrap();
            if ev.event != "message" {
                continue;
            }
            if let Ok(rpc) = serde_json::from_str::<JsonRpcResponse>(&ev.data) {
                if id_matches(&rpc.id, 42) && (rpc.result.is_some() || rpc.error.is_some()) {
                    break rpc;
                }
            }
        };
        assert_eq!(rpc.result.unwrap(), json!({"ok": true}));
    }

    #[test]
    fn bounded_line_respects_cap() {
        let long = vec![b'a'; 64];
        let mut cur = Cursor::new(long);
        let err = read_bounded_line(&mut cur, 16).unwrap_err();
        assert!(err.to_string().contains("byte cap"));
    }

    #[test]
    fn bounded_line_custom_eof_message() {
        let mut cur = Cursor::new(Vec::<u8>::new());
        let err = read_bounded_line_msg(&mut cur, 16, "stream gone").unwrap_err();
        assert_eq!(err.to_string(), "stream gone");
    }

    #[test]
    fn resolve_endpoint_absolute_url() {
        let out =
            resolve_endpoint("https://example.com/sse", "https://other.example.com/rpc").unwrap();
        assert_eq!(out, "https://other.example.com/rpc");
    }

    #[test]
    fn resolve_endpoint_absolute_path() {
        let out = resolve_endpoint("https://example.com/sse", "/messages?sessionId=42").unwrap();
        assert_eq!(out, "https://example.com/messages?sessionId=42");
    }

    #[test]
    fn resolve_endpoint_relative_path() {
        let out = resolve_endpoint("https://example.com/mcp/sse", "messages").unwrap();
        assert_eq!(out, "https://example.com/mcp/messages");
    }

    #[test]
    fn id_matches_number_and_string() {
        assert!(id_matches(&Some(json!(7)), 7));
        assert!(id_matches(&Some(json!("7")), 7));
        assert!(!id_matches(&Some(json!(8)), 7));
        assert!(!id_matches(&Some(json!(null)), 7));
        assert!(!id_matches(&None, 7));
    }
}

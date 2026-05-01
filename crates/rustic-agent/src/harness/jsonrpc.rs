//! Generic JSON-RPC 2.0 reader/writer over newline-delimited stdio.
//!
//! Used by the Codex harness (`harness/codex.rs`). Codex's
//! `codex app-server --listen stdio://` mode sends/receives one JSON-RPC
//! message per line — same line-delimited framing as Claude Code's NDJSON,
//! which means we can build on top of `stream_json::NdjsonReader/Writer`.
//!
//! Three message kinds we care about (per the JSONRPCMessage schema):
//! * **Request** — has `id` + `method` + `params`. Expects a Response or
//!   Error response with the same id.
//! * **Notification** — has `method` + `params` but no `id`. Fire-and-
//!   forget; no reply expected.
//! * **Response/Error** — has `id` + (`result` xor `error`). Reply to a
//!   prior outbound Request.
//!
//! `RequestId` per the schema is `string | int64`. We always send strings
//! (UUID-shaped) and accept either form on parse.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{oneshot, Mutex as AsyncMutex};

/// JSON-RPC request id. Schema allows string or int; we use strings on
/// outbound and accept either on inbound.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    String(String),
    Int(i64),
}

impl RequestId {
    pub fn new_uuid() -> Self {
        RequestId::String(uuid::Uuid::new_v4().to_string())
    }
}

impl std::fmt::Display for RequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RequestId::String(s) => write!(f, "{s}"),
            RequestId::Int(i) => write!(f, "{i}"),
        }
    }
}

/// Discriminated union over the four JSON-RPC message kinds. Decoded once
/// off the wire; the harness then dispatches by variant.
pub enum JsonRpcMessage {
    /// Server-initiated request — needs a response.
    Request {
        id: RequestId,
        method: String,
        params: Value,
    },
    /// Server notification — fire-and-forget.
    Notification { method: String, params: Value },
    /// Reply to one of our outbound requests.
    Response { id: RequestId, result: Value },
    /// Error reply to one of our outbound requests.
    Error {
        id: RequestId,
        code: i64,
        message: String,
        data: Option<Value>,
    },
}

/// Decode a single JSON envelope into one of the four message kinds.
/// Returns `None` if the envelope doesn't look like a JSON-RPC message
/// (missing `jsonrpc` marker or required fields).
pub fn decode(envelope: &Value) -> Option<JsonRpcMessage> {
    let obj = envelope.as_object()?;
    let id = obj.get("id").and_then(parse_request_id);
    let method = obj.get("method").and_then(|v| v.as_str()).map(String::from);
    let has_result = obj.get("result").is_some();
    let has_error = obj.get("error").is_some();

    match (id, method, has_result, has_error) {
        // Response: id + result, no method.
        (Some(id), None, true, false) => Some(JsonRpcMessage::Response {
            id,
            result: obj.get("result").cloned().unwrap_or(Value::Null),
        }),
        // Error response: id + error.
        (Some(id), _, _, true) => {
            let err = obj.get("error")?.as_object()?;
            let code = err.get("code").and_then(Value::as_i64).unwrap_or(0);
            let message = err
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let data = err.get("data").cloned();
            Some(JsonRpcMessage::Error {
                id,
                code,
                message,
                data,
            })
        }
        // Server-initiated request: id + method.
        (Some(id), Some(method), false, false) => Some(JsonRpcMessage::Request {
            id,
            method,
            params: obj.get("params").cloned().unwrap_or(Value::Null),
        }),
        // Notification: method, no id.
        (None, Some(method), _, _) => Some(JsonRpcMessage::Notification {
            method,
            params: obj.get("params").cloned().unwrap_or(Value::Null),
        }),
        _ => None,
    }
}

fn parse_request_id(v: &Value) -> Option<RequestId> {
    match v {
        Value::String(s) => Some(RequestId::String(s.clone())),
        Value::Number(n) => n.as_i64().map(RequestId::Int),
        _ => None,
    }
}

/// Build an outbound JSON-RPC request envelope: `{ jsonrpc, id, method, params }`.
pub fn encode_request(id: &RequestId, method: &str, params: Value) -> Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    })
}

/// Build an outbound JSON-RPC response envelope (success): `{ jsonrpc, id, result }`.
pub fn encode_response(id: &RequestId, result: Value) -> Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    })
}

/// Build an outbound JSON-RPC error envelope: `{ jsonrpc, id, error }`.
pub fn encode_error(id: &RequestId, code: i64, message: &str) -> Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message },
    })
}

/// Pending request registry — correlates response ids with their waiting
/// callers. Each outbound request gets a unique id and a oneshot sender;
/// the read loop pops the sender on response and resolves the future.
#[derive(Default)]
pub struct PendingRequests {
    next_id: AtomicU64,
    waiters: AsyncMutex<HashMap<RequestId, oneshot::Sender<Result<Value>>>>,
}

impl PendingRequests {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Allocate a fresh id and register a oneshot receiver for the eventual
    /// response. Returns the id (to embed in the outbound envelope) and the
    /// receiver (to await the result).
    pub async fn allocate(&self) -> (RequestId, oneshot::Receiver<Result<Value>>) {
        let n = self.next_id.fetch_add(1, Ordering::SeqCst);
        // String-shaped id matches the schema's preferred form (and lets
        // us interop cleanly with servers that compare ids as strings).
        let id = RequestId::String(format!("rustic-{n}"));
        let (tx, rx) = oneshot::channel();
        self.waiters.lock().await.insert(id.clone(), tx);
        (id, rx)
    }

    /// Resolve a previously-allocated request with its response payload.
    /// Returns true if a waiter was found and notified.
    pub async fn resolve(&self, id: &RequestId, result: Result<Value>) -> bool {
        let tx = self.waiters.lock().await.remove(id);
        match tx {
            Some(tx) => {
                let _ = tx.send(result);
                true
            }
            None => false,
        }
    }

    /// Drop every pending waiter — used when the underlying connection
    /// closes so callers don't hang forever.
    pub async fn fail_all(&self, reason: &str) {
        let mut g = self.waiters.lock().await;
        for (_, tx) in g.drain() {
            let _ = tx.send(Err(anyhow::anyhow!(reason.to_string())));
        }
    }
}

/// Default error code for a server-initiated request we don't yet handle.
/// Per JSON-RPC 2.0 spec, -32601 is "method not found".
pub const ERROR_METHOD_NOT_FOUND: i64 = -32601;

/// Convenience: drive `PendingRequests` plus an `NdjsonWriter` to send a
/// request and await its response. Bails if the response was an Error or
/// the connection closed before resolution.
pub async fn call<W: tokio::io::AsyncWrite + Unpin + Send>(
    pending: &PendingRequests,
    writer: &crate::harness::stream_json::NdjsonWriter<W>,
    method: &str,
    params: Value,
) -> Result<Value> {
    let (id, rx) = pending.allocate().await;
    let envelope = encode_request(&id, method, params);
    writer
        .write(&envelope)
        .await
        .with_context(|| format!("write of {method} request failed"))?;
    rx.await
        .map_err(|_| anyhow::anyhow!("{method}: connection closed before response"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn decode_notification() {
        let env = json!({
            "jsonrpc": "2.0",
            "method": "thread/started",
            "params": { "thread": { "id": "thr_abc" } }
        });
        match decode(&env) {
            Some(JsonRpcMessage::Notification { method, params }) => {
                assert_eq!(method, "thread/started");
                assert_eq!(params["thread"]["id"], "thr_abc");
            }
            other => panic!("expected Notification, got {other:?}"),
        }
    }

    #[test]
    fn decode_response() {
        let env = json!({
            "jsonrpc": "2.0",
            "id": "rustic-3",
            "result": { "thread": { "id": "thr_xyz" } }
        });
        match decode(&env) {
            Some(JsonRpcMessage::Response { id, result }) => {
                assert_eq!(id, RequestId::String("rustic-3".into()));
                assert_eq!(result["thread"]["id"], "thr_xyz");
            }
            other => panic!("expected Response, got {other:?}"),
        }
    }

    #[test]
    fn decode_error() {
        let env = json!({
            "jsonrpc": "2.0",
            "id": 7,
            "error": { "code": -32602, "message": "invalid params" }
        });
        match decode(&env) {
            Some(JsonRpcMessage::Error { id, code, message, .. }) => {
                assert_eq!(id, RequestId::Int(7));
                assert_eq!(code, -32602);
                assert_eq!(message, "invalid params");
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn decode_server_request() {
        let env = json!({
            "jsonrpc": "2.0",
            "id": "srv-1",
            "method": "applyPatch",
            "params": { "changes": [] }
        });
        match decode(&env) {
            Some(JsonRpcMessage::Request { id, method, params }) => {
                assert_eq!(id, RequestId::String("srv-1".into()));
                assert_eq!(method, "applyPatch");
                assert!(params["changes"].is_array());
            }
            other => panic!("expected Request, got {other:?}"),
        }
    }

    #[test]
    fn decode_rejects_garbage() {
        assert!(decode(&json!({})).is_none());
        assert!(decode(&json!({ "jsonrpc": "2.0" })).is_none());
        assert!(decode(&json!(["not", "an", "object"])).is_none());
    }

    #[test]
    fn encode_request_round_trips() {
        let id = RequestId::String("abc".into());
        let env = encode_request(&id, "turn/start", json!({ "input": "hi" }));
        match decode(&env).expect("encoded request decodes") {
            JsonRpcMessage::Request { id: round, method, params } => {
                assert_eq!(round, id);
                assert_eq!(method, "turn/start");
                assert_eq!(params["input"], "hi");
            }
            other => panic!("expected Request, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn pending_requests_resolve() {
        let pending = PendingRequests::new();
        let (id, rx) = pending.allocate().await;
        let result = serde_json::json!({ "ok": true });
        assert!(pending.resolve(&id, Ok(result.clone())).await);
        let got = rx.await.unwrap().unwrap();
        assert_eq!(got, result);
    }

    #[tokio::test]
    async fn pending_requests_fail_all_unblocks_waiters() {
        let pending = PendingRequests::new();
        let (_id, rx) = pending.allocate().await;
        pending.fail_all("connection closed").await;
        let err = rx.await.unwrap().unwrap_err();
        assert!(err.to_string().contains("connection closed"));
    }
}

// `Debug` for JsonRpcMessage so test panics show clean output.
impl std::fmt::Debug for JsonRpcMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JsonRpcMessage::Request { id, method, .. } => {
                write!(f, "Request{{ id: {id}, method: {method} }}")
            }
            JsonRpcMessage::Notification { method, .. } => {
                write!(f, "Notification{{ method: {method} }}")
            }
            JsonRpcMessage::Response { id, .. } => write!(f, "Response{{ id: {id} }}"),
            JsonRpcMessage::Error {
                id, code, message, ..
            } => write!(f, "Error{{ id: {id}, code: {code}, message: {message} }}"),
        }
    }
}

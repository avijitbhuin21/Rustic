//! `/ws` — the single multiplexed event socket. Each browser tab opens one;
//! the frontend `listen(event, cb)` shim filters the stream by event name.

use std::sync::Arc;

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::Response,
};
use tokio::sync::broadcast::error::RecvError;

use crate::app::Shared;
use rustic_app::context::AppContext;

/// Upgrade handler. Auth is enforced by the middleware layer before we get
/// here (cookie or a one-time `?ticket=`), so by this point the connection is trusted.
pub async fn ws_handler(State(shared): State<Arc<Shared>>, upgrade: WebSocketUpgrade) -> Response {
    upgrade.on_upgrade(move |socket| client_loop(socket, shared))
}

async fn client_loop(mut socket: WebSocket, shared: Arc<Shared>) {
    let mut rx = shared.ctx.hub.subscribe();
    loop {
        tokio::select! {
            // Server → client: forward every published event as JSON text.
            recv = rx.recv() => match recv {
                Ok(msg) => {
                    let Ok(text) = serde_json::to_string(&msg) else { continue };
                    if socket.send(Message::Text(text)).await.is_err() {
                        break; // client went away
                    }
                }
                // A lagging subscriber dropped messages; keep going from the
                // current position rather than tearing down the socket. The
                // frontend resyncs via a fresh fetch on reconnect/refresh.
                Err(RecvError::Lagged(n)) => {
                    tracing::warn!(skipped = n, "ws subscriber lagged; skipping ahead");
                    continue;
                }
                Err(RecvError::Closed) => break,
            },
            // Client → server: terminal keystrokes are pushed up this socket to
            // avoid a fresh HTTP round-trip per character (latency on remote
            // deploys). Everything else (commands) still goes over HTTP.
            from_client = socket.recv() => match from_client {
                Some(Ok(Message::Close(_))) | None => break,
                Some(Ok(Message::Text(txt))) => handle_client_text(&shared, &txt),
                Some(Ok(_)) => continue,
                Some(Err(_)) => break,
            },
        }
    }
}

/// Apply a client→server WS message. Currently only terminal keystrokes; the
/// socket is already authenticated at upgrade, so this carries the same trust
/// as the HTTP `write_terminal` command it mirrors. Best-effort: a malformed
/// frame or dead session is silently dropped (the client also has an HTTP
/// fallback).
fn handle_client_text(shared: &Arc<Shared>, txt: &str) {
    #[derive(serde::Deserialize)]
    #[serde(tag = "t")]
    enum ClientMsg {
        #[serde(rename = "terminal-input", rename_all = "camelCase")]
        TerminalInput { session_id: u64, data: String },
    }

    let Ok(msg) = serde_json::from_str::<ClientMsg>(txt) else {
        return;
    };
    match msg {
        ClientMsg::TerminalInput { session_id, data } => {
            if let Ok(mut manager) = shared.ctx.state().terminal_manager.lock() {
                let _ = manager.write_session(session_id, data.as_bytes());
            }
        }
    }
}

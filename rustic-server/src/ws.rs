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

/// Upgrade handler. Auth is enforced by the middleware layer before we get
/// here (cookie or `?token=`), so by this point the connection is trusted.
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
            // Client → server: we only care about close / pings here. The
            // browser never sends app data up this socket (commands go over
            // HTTP), so anything else is ignored.
            from_client = socket.recv() => match from_client {
                Some(Ok(Message::Close(_))) | None => break,
                Some(Ok(_)) => continue,
                Some(Err(_)) => break,
            },
        }
    }
}

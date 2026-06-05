//! Authed reverse-proxy from the public server port to Chromium's loopback CDP.
//!
//! Two kinds of traffic, both behind the session-token auth middleware (CDP is a
//! full RCE surface — it can read `file://` — so it must never be reachable
//! unauthenticated; Chromium itself is loopback-bound as a second layer):
//!
//! * `GET /ws/browser/cdp?target=<id>` — a bidirectional WebSocket tunnel to the
//!   page's CDP socket. One per consumer (the screencast viewport and the
//!   embedded DevTools frontend each open their own). Drives the socket
//!   ref-count that gates Chromium teardown.
//! * `GET /api/browser/devtools/*` and `/api/browser/json[/*]` — HTTP GETs for
//!   the bundled DevTools frontend assets and the CDP discovery endpoints.

use std::sync::Arc;

use axum::{
    body::Body,
    extract::{
        ws::{Message as AxMessage, WebSocket, WebSocketUpgrade},
        Path as AxumPath, Query, State,
    },
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::json;
use tokio_tungstenite::tungstenite::Message as TMessage;

use crate::app::Shared;

#[derive(Deserialize)]
pub struct CdpQuery {
    /// The CDP `targetId` of the page to attach to.
    target: String,
}

/// `GET /ws/browser/cdp?target=<id>` — upgrade, then pipe frames both ways
/// between this client socket and the page's loopback CDP socket.
pub async fn cdp_ws(
    State(shared): State<Arc<Shared>>,
    Query(q): Query<CdpQuery>,
    upgrade: WebSocketUpgrade,
) -> Response {
    let browser = shared.ctx.browser.clone();
    let upstream_url = browser.page_ws(&q.target);
    upgrade.on_upgrade(move |socket| pipe(socket, upstream_url, browser))
}

/// Bidirectional pump between the client WebSocket and Chromium's CDP socket.
async fn pipe(client: WebSocket, upstream_url: String, browser: Arc<super::BrowserManager>) {
    let upstream = match tokio_tungstenite::connect_async(&upstream_url).await {
        Ok((s, _)) => s,
        Err(e) => {
            tracing::warn!(url = %upstream_url, error = %e, "CDP proxy: upstream connect failed");
            return;
        }
    };

    // Count this socket so the idle watchdog knows the browser is in use.
    browser.socket_opened().await;

    let (mut client_tx, mut client_rx) = client.split();
    let (mut up_tx, mut up_rx) = upstream.split();

    // client → Chromium
    let c2u = async {
        while let Some(Ok(msg)) = client_rx.next().await {
            let out = match msg {
                AxMessage::Text(t) => TMessage::Text(t.into()),
                AxMessage::Binary(b) => TMessage::Binary(b.into()),
                AxMessage::Ping(b) => TMessage::Ping(b.into()),
                AxMessage::Pong(b) => TMessage::Pong(b.into()),
                AxMessage::Close(_) => break,
            };
            if up_tx.send(out).await.is_err() {
                break;
            }
        }
        let _ = up_tx.close().await;
    };

    // Chromium → client
    let u2c = async {
        while let Some(Ok(msg)) = up_rx.next().await {
            let out = match msg {
                TMessage::Text(t) => AxMessage::Text(t.to_string()),
                TMessage::Binary(b) => AxMessage::Binary(b.to_vec()),
                TMessage::Ping(b) => AxMessage::Ping(b.to_vec()),
                TMessage::Pong(b) => AxMessage::Pong(b.to_vec()),
                TMessage::Close(_) => break,
                TMessage::Frame(_) => continue,
            };
            if client_tx.send(out).await.is_err() {
                break;
            }
        }
        let _ = client_tx.close().await;
    };

    // When either side ends, the other future is dropped and its half closed.
    tokio::select! {
        _ = c2u => {}
        _ = u2c => {}
    }

    browser.socket_closed().await;
}

/// `GET /api/browser/devtools/*path` — the bundled DevTools frontend + assets.
pub async fn devtools(State(shared): State<Arc<Shared>>, AxumPath(path): AxumPath<String>) -> Response {
    proxy_http(&shared, &format!("devtools/{path}")).await
}

/// `GET /api/browser/json` — CDP target discovery.
pub async fn json_root(State(shared): State<Arc<Shared>>) -> Response {
    proxy_http(&shared, "json").await
}

/// `GET /api/browser/json/*path` — CDP discovery sub-paths (`/json/version`, …).
pub async fn json_path(State(shared): State<Arc<Shared>>, AxumPath(path): AxumPath<String>) -> Response {
    proxy_http(&shared, &format!("json/{path}")).await
}

/// Forward a GET to Chromium's loopback HTTP server, preserving status +
/// content-type so DevTools JS/CSS is served with the right MIME.
async fn proxy_http(shared: &Arc<Shared>, path: &str) -> Response {
    let port = shared.ctx.browser.port();
    let url = format!("http://127.0.0.1:{port}/{path}");

    let resp = match reqwest::Client::new().get(&url).send().await {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                axum::Json(json!({ "error": format!("browser not reachable: {e}") })),
            )
                .into_response();
        }
    };

    let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let content_type = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();

    let bytes = match resp.bytes().await {
        Ok(b) => b,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                axum::Json(json!({ "error": format!("browser response read failed: {e}") })),
            )
                .into_response();
        }
    };

    (
        status,
        [(header::CONTENT_TYPE, content_type)],
        Body::from(bytes),
    )
        .into_response()
}

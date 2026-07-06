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

    // Messages destined for Chromium: client commands AND our own screencast
    // acks. A single writer task drains this so both sources serialize cleanly.
    let (to_up_tx, mut to_up_rx) = tokio::sync::mpsc::channel::<TMessage>(256);
    // Non-frame messages for the client (command responses, events, pings):
    // delivered reliably, in order.
    let (ctrl_tx, mut ctrl_rx) = tokio::sync::mpsc::channel::<AxMessage>(256);
    // The latest screencast frame for the client. A `watch` keeps only the most
    // recent value, so when the remote link is slower than Chromium's capture
    // rate the intermediate frames are DROPPED (always render the freshest one)
    // rather than piling into an ever-growing, ever-staler backlog.
    let (frame_tx, mut frame_rx) = tokio::sync::watch::channel::<Option<String>>(None);

    // Writer → Chromium (loopback): client commands + locally-generated acks.
    let up_writer = async move {
        while let Some(m) = to_up_rx.recv().await {
            if up_tx.send(m).await.is_err() {
                break;
            }
        }
        let _ = up_tx.close().await;
    };

    // Reader ← client: forward the user's CDP commands to Chromium.
    let to_up_from_client = to_up_tx.clone();
    let c2u = async move {
        while let Some(Ok(msg)) = client_rx.next().await {
            let out = match msg {
                AxMessage::Text(t) => TMessage::Text(t.into()),
                AxMessage::Binary(b) => TMessage::Binary(b.into()),
                AxMessage::Ping(b) => TMessage::Ping(b.into()),
                AxMessage::Pong(b) => TMessage::Pong(b.into()),
                AxMessage::Close(_) => break,
            };
            if to_up_from_client.send(out).await.is_err() {
                break;
            }
        }
    };

    // Reader ← Chromium. Screencast frames are acked HERE, over loopback, the
    // instant they arrive — so Chromium's frame rate is bounded by local capture
    // speed instead of the client's network round-trip (which otherwise caps the
    // stream at ~1/RTT, i.e. a handful of fps over a remote link). The frame
    // itself is handed to the latest-wins slot; every other message passes
    // through the ordered control channel untouched.
    let u2c = async move {
        let mut ack_id: u64 = 1_000_000; // high range never collides with client ids
        while let Some(Ok(msg)) = up_rx.next().await {
            match msg {
                TMessage::Text(t) => {
                    if let Some(sid) = screencast_session_id(t.as_str()) {
                        ack_id += 1;
                        let ack = json!({
                            "id": ack_id,
                            "method": "Page.screencastFrameAck",
                            "params": { "sessionId": sid },
                        })
                        .to_string();
                        if to_up_tx.send(TMessage::Text(ack.into())).await.is_err() {
                            break;
                        }
                        // Latest-wins: replaces any frame the client hasn't sent yet.
                        if frame_tx.send(Some(t.to_string())).is_err() {
                            break;
                        }
                    } else if ctrl_tx.send(AxMessage::Text(t.to_string())).await.is_err() {
                        break;
                    }
                }
                TMessage::Binary(b) => {
                    if ctrl_tx.send(AxMessage::Binary(b.to_vec())).await.is_err() {
                        break;
                    }
                }
                TMessage::Ping(b) => {
                    if ctrl_tx.send(AxMessage::Ping(b.to_vec())).await.is_err() {
                        break;
                    }
                }
                TMessage::Pong(b) => {
                    if ctrl_tx.send(AxMessage::Pong(b.to_vec())).await.is_err() {
                        break;
                    }
                }
                TMessage::Close(_) => break,
                TMessage::Frame(_) => continue,
            }
        }
    };

    // Writer → client: interleave reliable control messages, the freshest frame,
    // and a 20s keepalive ping. A CDP screencast emits nothing while the page is
    // static, so without the ping an idle socket gets dropped by edge proxies
    // (Railway/Cloudflare) — which zeroes the ref-count and lets the idle
    // watchdog reap Chromium out from under the user.
    let client_writer = async move {
        let mut keepalive = tokio::time::interval(std::time::Duration::from_secs(20));
        keepalive.tick().await; // consume the immediate first tick
        loop {
            tokio::select! {
                msg = ctrl_rx.recv() => {
                    match msg {
                        Some(m) => {
                            if client_tx.send(m).await.is_err() {
                                break;
                            }
                        }
                        None => break,
                    }
                }
                changed = frame_rx.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    let frame = frame_rx.borrow_and_update().clone();
                    if let Some(text) = frame {
                        if client_tx.send(AxMessage::Text(text)).await.is_err() {
                            break;
                        }
                    }
                }
                _ = keepalive.tick() => {
                    if client_tx.send(AxMessage::Ping(Vec::new())).await.is_err() {
                        break;
                    }
                }
            }
        }
        let _ = client_tx.close().await;
    };

    // When any pump finishes (a socket closed), the others are dropped and their
    // halves closed.
    tokio::select! {
        _ = up_writer => {}
        _ = c2u => {}
        _ = u2c => {}
        _ = client_writer => {}
    }

    browser.socket_closed().await;
}

/// If `text` is a `Page.screencastFrame` event, return its `sessionId` (needed
/// to ack the frame). Cheap substring scan — avoids fully parsing the large
/// base64 JPEG payload that dominates every frame message.
fn screencast_session_id(text: &str) -> Option<i64> {
    if !text.contains("\"Page.screencastFrame\"") {
        return None;
    }
    const KEY: &str = "\"sessionId\":";
    let start = text.find(KEY)? + KEY.len();
    let rest = text[start..].trim_start();
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

/// `GET /api/browser/devtools/*path` — the bundled DevTools frontend + assets.
pub async fn devtools(
    State(shared): State<Arc<Shared>>,
    AxumPath(path): AxumPath<String>,
) -> Response {
    proxy_http(&shared, &format!("devtools/{path}")).await
}

/// `GET /api/browser/json` — CDP target discovery.
pub async fn json_root(State(shared): State<Arc<Shared>>) -> Response {
    proxy_http(&shared, "json").await
}

/// `GET /api/browser/json/*path` — CDP discovery sub-paths (`/json/version`, …).
pub async fn json_path(
    State(shared): State<Arc<Shared>>,
    AxumPath(path): AxumPath<String>,
) -> Response {
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

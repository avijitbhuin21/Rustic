//! Port-forwarding tunnel: open a VM dev server in the user's OWN browser
//! instead of the embedded headless Chromium. Two modes share the same
//! forwarding core:
//!
//! * **Path mode** (default, zero-config): `ANY /proxy/:port/*path` →
//!   `http://127.0.0.1:<port>/*path`. Works on any host (incl. the bare
//!   `*.up.railway.app`), but apps that load assets from an absolute root path
//!   (`/assets/x.js`) escape the prefix and need a dev-server base path.
//! * **Subdomain mode** (when `RUSTIC_PREVIEW_DOMAIN` is set): a request to
//!   `<port>.<preview-domain>` is forwarded verbatim to `127.0.0.1:<port>`.
//!   Apps work unmodified because they own the whole host. Routing happens in
//!   `app::host_proxy_middleware`; the actual forwarding is [`forward_host`].
//!
//! Both forward plain HTTP and the WebSocket upgrade (dev-server HMR). The WS
//! pump mirrors `browser::proxy::pipe`. `Location` redirects are rewritten back
//! under the `/proxy/<port>` prefix in path mode only.

use std::sync::Arc;

use axum::{
    body::Body,
    extract::{
        ws::{Message as AxMessage, WebSocket, WebSocketUpgrade},
        FromRequestParts, Path as AxumPath, State,
    },
    http::{header, HeaderMap, HeaderValue, Request, StatusCode},
    response::{IntoResponse, Response},
};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::{handshake::client::generate_key, Message as TMessage};

use crate::app::Shared;

/// `ANY /proxy/:port` — path mode, loopback service root.
pub async fn root(
    State(_shared): State<Arc<Shared>>,
    AxumPath(port): AxumPath<u16>,
    ws: Option<WebSocketUpgrade>,
    req: Request<Body>,
) -> Response {
    let target = stripped_target(&req, port);
    forward(port, target, ws, req).await
}

/// `ANY /proxy/:port/*path` — path mode, loopback service sub-path.
pub async fn with_path(
    State(_shared): State<Arc<Shared>>,
    AxumPath((port, _path)): AxumPath<(u16, String)>,
    ws: Option<WebSocketUpgrade>,
    req: Request<Body>,
) -> Response {
    let target = stripped_target(&req, port);
    forward(port, target, ws, req).await
}

/// Subdomain mode: forward the request verbatim (path + query untouched) to the
/// loopback port. Called from the host-routing middleware once auth has passed.
pub async fn forward_host(port: u16, req: Request<Body>) -> Response {
    let target = verbatim_target(&req);

    if wants_websocket(req.headers()) {
        let (mut parts, body) = req.into_parts();
        match WebSocketUpgrade::from_request_parts(&mut parts, &()).await {
            Ok(upgrade) => return do_ws(upgrade, &parts.headers, port, &target),
            Err(rej) => {
                let _ = body;
                return rej.into_response();
            }
        }
    }

    http_proxy(port, &target, req, false).await
}

/// Path-mode target: strip the `/proxy/<port>` prefix off the raw request URI
/// (raw, not the decoded wildcard param, to preserve exact percent-encoding).
fn stripped_target(req: &Request<Body>, port: u16) -> String {
    let prefix = format!("/proxy/{port}");
    let path = req.uri().path();
    let rest = path.strip_prefix(&prefix).unwrap_or("/");
    let rest = if rest.is_empty() { "/" } else { rest };
    match req.uri().query() {
        Some(q) => format!("{rest}?{q}"),
        None => rest.to_string(),
    }
}

/// Subdomain-mode target: the request's own path + query, unchanged.
fn verbatim_target(req: &Request<Body>) -> String {
    let path = req.uri().path();
    match req.uri().query() {
        Some(q) => format!("{path}?{q}"),
        None => path.to_string(),
    }
}

/// True when the request is a WebSocket upgrade handshake.
fn wants_websocket(headers: &HeaderMap) -> bool {
    let has = |name, needle: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.to_ascii_lowercase().contains(needle))
            .unwrap_or(false)
    };
    has(header::UPGRADE, "websocket") && has(header::CONNECTION, "upgrade")
}

/// Path-mode dispatch: WebSocket upgrade (HMR) or plain HTTP.
async fn forward(
    port: u16,
    target: String,
    ws: Option<WebSocketUpgrade>,
    req: Request<Body>,
) -> Response {
    if let Some(upgrade) = ws {
        return do_ws(upgrade, req.headers(), port, &target);
    }
    http_proxy(port, &target, req, true).await
}

/// Accept the client WebSocket and pump frames both ways to the loopback
/// service's WebSocket. Echoes the client's first requested subprotocol.
fn do_ws(upgrade: WebSocketUpgrade, headers: &HeaderMap, port: u16, target: &str) -> Response {
    let proto = headers
        .get(header::SEC_WEBSOCKET_PROTOCOL)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let upstream_url = format!("ws://127.0.0.1:{port}{target}");
    let upgrade = match proto.clone() {
        Some(p) => upgrade.protocols([p]),
        None => upgrade,
    };
    upgrade.on_upgrade(move |socket| ws_pipe(socket, upstream_url, port, proto))
}

/// Forward a plain HTTP request to the loopback service and stream the response.
/// `rewrite` controls path-mode `Location` rewriting (off in subdomain mode).
async fn http_proxy(port: u16, target: &str, req: Request<Body>, rewrite: bool) -> Response {
    let url = format!("http://127.0.0.1:{port}{target}");

    let method = match reqwest::Method::from_bytes(req.method().as_str().as_bytes()) {
        Ok(m) => m,
        Err(_) => return (StatusCode::BAD_REQUEST, "bad method").into_response(),
    };

    let mut headers = req.headers().clone();
    strip_hop_by_hop(&mut headers);
    if let Ok(h) = HeaderValue::from_str(&format!("localhost:{port}")) {
        headers.insert(header::HOST, h);
    }

    let body = reqwest::Body::wrap_stream(req.into_body().into_data_stream());

    let client = match reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("proxy client: {e}"))
                .into_response()
        }
    };

    let upstream = match client.request(method, &url).headers(headers).body(body).send().await {
        Ok(r) => r,
        Err(e) => {
            return (StatusCode::BAD_GATEWAY, format!("no service on port {port}: {e}"))
                .into_response();
        }
    };

    let status = StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut resp_headers = upstream.headers().clone();
    strip_hop_by_hop(&mut resp_headers);
    if rewrite {
        rewrite_location(&mut resp_headers, port);
    }

    let stream = upstream.bytes_stream();
    let mut resp = Response::new(Body::from_stream(stream));
    *resp.status_mut() = status;
    *resp.headers_mut() = resp_headers;
    resp
}

/// Bidirectional pump between the client WebSocket and the loopback service's
/// WebSocket (mirrors `browser::proxy::pipe`).
async fn ws_pipe(client: WebSocket, upstream_url: String, port: u16, proto: Option<String>) {
    let mut builder = Request::builder()
        .uri(&upstream_url)
        .header(header::HOST, format!("localhost:{port}"))
        .header(header::CONNECTION, "Upgrade")
        .header(header::UPGRADE, "websocket")
        .header(header::SEC_WEBSOCKET_VERSION, "13")
        .header(header::SEC_WEBSOCKET_KEY, generate_key());
    if let Some(p) = proto {
        builder = builder.header(header::SEC_WEBSOCKET_PROTOCOL, p);
    }
    let request = match builder.body(()) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "proxy ws: bad upstream request");
            return;
        }
    };

    let upstream = match tokio_tungstenite::connect_async(request).await {
        Ok((s, _)) => s,
        Err(e) => {
            tracing::warn!(url = %upstream_url, error = %e, "proxy ws: upstream connect failed");
            return;
        }
    };

    let (mut client_tx, mut client_rx) = client.split();
    let (mut up_tx, mut up_rx) = upstream.split();

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

    tokio::select! {
        _ = c2u => {}
        _ = u2c => {}
    }
}

/// Remove hop-by-hop headers that must not be forwarded across a proxy.
fn strip_hop_by_hop(headers: &mut HeaderMap) {
    const HOP: [&str; 8] = [
        "connection",
        "keep-alive",
        "proxy-connection",
        "transfer-encoding",
        "te",
        "trailer",
        "upgrade",
        "proxy-authenticate",
    ];
    for name in HOP {
        headers.remove(name);
    }
}

/// Rewrite a redirect `Location` back under the `/proxy/<port>/` prefix so the
/// browser stays inside the tunnel (path mode only).
fn rewrite_location(headers: &mut HeaderMap, port: u16) {
    let Some(loc) = headers.get(header::LOCATION).and_then(|v| v.to_str().ok()) else {
        return;
    };

    let new = if loc.starts_with("//") {
        None
    } else if let Some(rest) = loc.strip_prefix('/') {
        Some(format!("/proxy/{port}/{rest}"))
    } else {
        let loopbacks = [
            format!("http://127.0.0.1:{port}"),
            format!("http://localhost:{port}"),
        ];
        loopbacks.iter().find_map(|origin| {
            loc.strip_prefix(origin).map(|rest| {
                let rest = if rest.is_empty() { "/" } else { rest };
                format!("/proxy/{port}{rest}")
            })
        })
    };

    if let Some(v) = new.and_then(|s| HeaderValue::from_str(&s).ok()) {
        headers.insert(header::LOCATION, v);
    }
}

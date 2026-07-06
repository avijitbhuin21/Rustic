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
    State(shared): State<Arc<Shared>>,
    AxumPath(port): AxumPath<u16>,
    ws: Option<WebSocketUpgrade>,
    req: Request<Body>,
) -> Response {
    if let Err(denied) = ensure_port_allowed(&shared, port).await {
        return denied;
    }
    let target = stripped_target(&req, port);
    forward(port, target, ws, req).await
}

/// `ANY /proxy/:port/*path` — path mode, loopback service sub-path.
pub async fn with_path(
    State(shared): State<Arc<Shared>>,
    AxumPath((port, _path)): AxumPath<(u16, String)>,
    ws: Option<WebSocketUpgrade>,
    req: Request<Body>,
) -> Response {
    if let Err(denied) = ensure_port_allowed(&shared, port).await {
        return denied;
    }
    let target = stripped_target(&req, port);
    forward(port, target, ws, req).await
}

/// SSRF guard for the port tunnel (defense-in-depth on top of auth): a
/// requested loopback port must be a plausible user dev server.
///
/// Always denied: the rustic-server bind port (auth-bypass loop), the embedded
/// Chromium CDP port (a full RCE surface) and cloudflared's metrics ports /
/// well-known metrics range. Where the kernel exposes the listen table
/// (`/proc/net/tcp`, i.e. the Linux server image — the same source the port
/// monitor uses) the port must additionally be actually LISTENing or already
/// registered with a Cloudflare tunnel; anything else is a 403 rather than a
/// connect attempt. On platforms without `/proc` (local Windows/macOS dev,
/// where the server binds loopback anyway) only the deny-list applies.
pub async fn ensure_port_allowed(shared: &Shared, port: u16) -> Result<(), Response> {
    // cloudflared metrics endpoints land here (mirrors `port_monitor`).
    const METRICS_RANGE: std::ops::Range<u16> = 20240..20260;

    if port == shared.config.bind_addr.port()
        || port == shared.ctx.browser.port()
        || METRICS_RANGE.contains(&port)
        || shared.ctx.cloudflared.metrics_ports().contains(&port)
    {
        return Err((
            StatusCode::FORBIDDEN,
            format!("port {port} is not proxyable"),
        )
            .into_response());
    }

    if let Some(listening) = linux_listening_ports() {
        if !listening.contains(&port)
            && !shared.ctx.cloudflared.managed_ports().await.contains(&port)
        {
            return Err((
                StatusCode::FORBIDDEN,
                format!("no known dev server listening on port {port}"),
            )
                .into_response());
        }
    }

    Ok(())
}

/// The set of TCP ports in LISTEN state from `/proc/net/tcp{,6}` (same parse as
/// `port_monitor::listening_ports`), or `None` when `/proc` isn't available.
fn linux_listening_ports() -> Option<std::collections::HashSet<u16>> {
    if std::fs::metadata("/proc/net/tcp").is_err() {
        return None;
    }
    let mut ports = std::collections::HashSet::new();
    for path in ["/proc/net/tcp", "/proc/net/tcp6"] {
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };
        for line in content.lines().skip(1) {
            // Columns: sl  local_address  rem_address  st  ... (st 0A = LISTEN)
            let cols: Vec<&str> = line.split_whitespace().collect();
            if cols.len() < 4 || cols[3] != "0A" {
                continue;
            }
            if let Some((_, port_hex)) = cols[1].rsplit_once(':') {
                if let Ok(port) = u16::from_str_radix(port_hex, 16) {
                    if port != 0 {
                        ports.insert(port);
                    }
                }
            }
        }
    }
    Some(ports)
}

/// Subdomain mode: forward the request verbatim (path + query untouched) to the
/// loopback port. Called from the host-routing middleware once auth has passed.
pub async fn forward_host(shared: &Shared, port: u16, req: Request<Body>) -> Response {
    if let Err(denied) = ensure_port_allowed(shared, port).await {
        return denied;
    }
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

/// Process-wide proxy client, built once and reused (connection pooling instead
/// of a fresh client + pool per proxied request). Redirects are disabled so the
/// dev server's own `Location` headers pass through (and get rewritten).
fn proxy_client() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("proxy reqwest client")
    })
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

    let client = proxy_client();

    let upstream = match client
        .request(method, &url)
        .headers(headers)
        .body(body)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                format!("no service on port {port}: {e}"),
            )
                .into_response();
        }
    };

    let status =
        StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
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

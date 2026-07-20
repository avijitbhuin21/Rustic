//! Router assembly: login, auth middleware, the `/api/:command` dispatch,
//! `/ws`, health, and static SPA serving.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    body::Body,
    extract::{ConnectInfo, DefaultBodyLimit, Path as AxumPath, State},
    http::{header, HeaderMap, Request, StatusCode},
    middleware::{from_fn, from_fn_with_state, Next},
    response::{IntoResponse, Response},
    routing::{any, get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;
use tower::ServiceBuilder;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_header::SetResponseHeaderLayer;

use rustic_app::config::ServerConfig;

use crate::auth::{self, RateLimiter};
use crate::browser;
use crate::context::ServerContext;
use crate::{api, proxy, ws};

/// Everything the handlers share, behind one `Arc`.
pub struct Shared {
    pub ctx: ServerContext,
    pub config: ServerConfig,
    pub rate: RateLimiter,
    /// Single-use short-TTL tickets for WebSocket (and download-navigation)
    /// auth — see [`auth::TicketStore`].
    pub tickets: auth::TicketStore,
}

const SESSION_COOKIE: &str = "rustic_session";

/// Build the full application router.
pub fn build_router(shared: Arc<Shared>) -> Router {
    let static_dir = shared.config.static_dir.clone();
    let index = static_dir.join("index.html");

    // Hashed Vite chunks under /assets are content-addressed, so they can be
    // cached forever. Crucially this service has NO index.html fallback: a
    // missing chunk returns a real 404 (rather than HTML), so the browser
    // surfaces a clear "failed to fetch module" instead of trying to execute
    // index.html as JS — and stale references fail loudly.
    let assets_service = ServiceBuilder::new()
        .layer(SetResponseHeaderLayer::overriding(
            header::CACHE_CONTROL,
            header::HeaderValue::from_static("public, max-age=31536000, immutable"),
        ))
        .service(ServeDir::new(static_dir.join("assets")));

    // SPA fallback: unknown non-API paths serve index.html so client-side
    // routing works on deep links / reloads. index.html itself must never be
    // cached, otherwise a rebuild (new chunk hashes) leaves the browser holding
    // an old document that imports chunks which no longer exist.
    let static_service = ServiceBuilder::new()
        .layer(SetResponseHeaderLayer::overriding(
            header::CACHE_CONTROL,
            header::HeaderValue::from_static("no-cache"),
        ))
        .service(ServeDir::new(&static_dir).not_found_service(ServeFile::new(index)));

    // Auth-gated routes.
    //
    // The `/ws/browser/cdp` + `/api/browser/*` routes reverse-proxy Chromium's
    // loopback CDP (a full RCE surface) — they live inside `protected` precisely
    // so the auth middleware below gates them; Chromium is loopback-bound as a
    // second layer. See `crate::browser::proxy`.
    let protected = Router::new()
        // Static segment wins over `/api/:command`, so this authed endpoint can
        // coexist with the generic dispatcher. It mints the one-time ticket a
        // browser WebSocket (which can't send an Authorization header) uses to
        // authenticate its upgrade request without putting the session token
        // in the URL.
        .route("/api/ws_ticket", post(ws_ticket_handler))
        .route("/api/:command", post(api_handler))
        .route("/api/upload_stream", post(upload_stream_handler))
        // Cloud sync: whole-environment push/pull (see rustic_app::cloud_sync).
        // The push body is a full tar.gz of the sender's environment — its own
        // (disabled) body limit overrides the router-wide 512 MB cap.
        .route(
            "/api/sync/push",
            post(sync_push_handler).layer(DefaultBodyLimit::disable()),
        )
        .route("/api/sync/state", get(sync_state_handler))
        .route("/api/sync/pull", post(sync_pull_handler))
        .route("/api/download", get(download_handler))
        .route("/api/asset", get(asset_handler))
        .route("/ws", get(ws::ws_handler))
        .route("/ws/browser/cdp", get(browser::proxy::cdp_ws))
        .route("/api/browser/json", get(browser::proxy::json_root))
        .route("/api/browser/json/*path", get(browser::proxy::json_path))
        .route("/api/browser/devtools/*path", get(browser::proxy::devtools))
        // Port-forwarding tunnel: open a VM dev server in the user's own
        // browser. Authed (it can reach any loopback port); `any` forwards
        // every method plus the WebSocket upgrade (HMR / live reload).
        .route("/proxy/:port", any(proxy::root))
        .route("/proxy/:port/*path", any(proxy::with_path))
        .layer(DefaultBodyLimit::max(512 * 1024 * 1024))
        .layer(from_fn_with_state(shared.clone(), auth_middleware));

    Router::new()
        .route("/healthz", get(health))
        .route("/login", post(login))
        .route("/logout", post(logout))
        // GitHub issue webhook: unauthenticated route — its auth is the
        // HMAC-SHA256 delivery signature checked inside the handler.
        .route(
            "/webhook/github",
            post(crate::github::webhook::github_webhook),
        )
        .merge(protected)
        .nest_service("/assets", assets_service)
        .fallback_service(static_service)
        // Origin policy on everything (incl. /login, the CSRF-sensitive one):
        // requests carrying a cross-site Origin outside the allow-list are
        // rejected before any handler runs. Requests without an Origin header
        // (same-origin GETs, curl, webhooks) pass through untouched.
        .layer(from_fn(cors_middleware))
        .with_state(shared)
}

/// Extra allowed origins from `RUSTIC_ALLOWED_ORIGINS` (comma-separated full
/// origins, e.g. `https://rustic.example.com`), parsed once. Same-origin
/// requests are always allowed and need no configuration.
fn allowed_origins() -> &'static Vec<String> {
    static ORIGINS: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    ORIGINS.get_or_init(|| {
        std::env::var("RUSTIC_ALLOWED_ORIGINS")
            .ok()
            .map(|v| {
                v.split(',')
                    .map(|s| s.trim().trim_end_matches('/').to_ascii_lowercase())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    })
}

/// Same-origin (Origin authority == Host header, any scheme) or allow-listed.
fn origin_allowed(origin: &str, host: Option<&str>) -> bool {
    let origin = origin.trim().trim_end_matches('/').to_ascii_lowercase();
    if let Some(host) = host {
        let host = host.trim().to_ascii_lowercase();
        let same = origin
            .strip_prefix("http://")
            .or_else(|| origin.strip_prefix("https://"))
            .map_or(false, |authority| authority == host);
        if same {
            return true;
        }
    }
    allowed_origins().iter().any(|o| *o == origin)
}

/// CORS / cross-site enforcement: reject any request whose `Origin` is neither
/// same-origin nor in `RUSTIC_ALLOWED_ORIGINS` (a hard 403, so cookie-bearing
/// cross-site requests never reach a handler), and emit the CORS response
/// headers (plus preflight handling) for the origins that ARE allowed.
async fn cors_middleware(req: Request<Body>, next: Next) -> Response {
    let Some(origin) = req
        .headers()
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
    else {
        return next.run(req).await;
    };
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    if !origin_allowed(&origin, host.as_deref()) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "Origin not allowed" })),
        )
            .into_response();
    }

    let origin_value = header::HeaderValue::from_str(&origin).ok();

    // Preflight for allow-listed cross-origin callers.
    if req.method() == axum::http::Method::OPTIONS {
        let mut resp = StatusCode::NO_CONTENT.into_response();
        let h = resp.headers_mut();
        if let Some(v) = origin_value {
            h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, v);
        }
        h.insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            header::HeaderValue::from_static("GET, POST, OPTIONS"),
        );
        h.insert(
            header::ACCESS_CONTROL_ALLOW_HEADERS,
            header::HeaderValue::from_static("authorization, content-type"),
        );
        h.insert(
            header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
            header::HeaderValue::from_static("true"),
        );
        h.insert(header::VARY, header::HeaderValue::from_static("Origin"));
        return resp;
    }

    let mut resp = next.run(req).await;
    let h = resp.headers_mut();
    if let Some(v) = origin_value {
        h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, v);
    }
    h.insert(
        header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
        header::HeaderValue::from_static("true"),
    );
    h.append(header::VARY, header::HeaderValue::from_static("Origin"));
    resp
}

async fn health() -> impl IntoResponse {
    Json(json!({ "status": "ok" }))
}

#[derive(Deserialize)]
struct LoginBody {
    password: String,
}

/// Extract the client IP for rate limiting. The socket peer is the default;
/// the first `X-Forwarded-For` hop is trusted ONLY when the socket peer itself
/// is one of the reverse proxies listed in `RUSTIC_TRUSTED_PROXIES`
/// (comma-separated IPs or `<ip>/<prefix-len>` CIDRs). A client-supplied
/// header would otherwise let an attacker spoof a fresh "IP" per attempt and
/// defeat the per-IP login rate limiter.
fn client_ip(headers: &header::HeaderMap, peer: SocketAddr) -> String {
    if peer_is_trusted_proxy(peer.ip()) {
        if let Some(ip) = headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.split(',').next())
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            return ip.to_string();
        }
    }
    peer.ip().to_string()
}

/// True when `peer` matches an entry of `RUSTIC_TRUSTED_PROXIES` (parsed once).
/// Unset/empty env var → nothing is trusted and `X-Forwarded-For` is ignored.
fn peer_is_trusted_proxy(peer: std::net::IpAddr) -> bool {
    static TRUSTED: std::sync::OnceLock<Vec<TrustedNet>> = std::sync::OnceLock::new();
    let nets = TRUSTED.get_or_init(|| {
        std::env::var("RUSTIC_TRUSTED_PROXIES")
            .ok()
            .map(|v| {
                v.split(',')
                    .filter_map(|s| TrustedNet::parse(s.trim()))
                    .collect()
            })
            .unwrap_or_default()
    });
    nets.iter().any(|n| n.contains(peer))
}

/// An exact IP or a simple `<ip>/<prefix-len>` CIDR.
struct TrustedNet {
    ip: std::net::IpAddr,
    prefix: u8,
}

impl TrustedNet {
    fn parse(s: &str) -> Option<Self> {
        if s.is_empty() {
            return None;
        }
        match s.split_once('/') {
            Some((ip, len)) => {
                let ip: std::net::IpAddr = ip.trim().parse().ok()?;
                let max = if ip.is_ipv4() { 32 } else { 128 };
                let prefix: u8 = len.trim().parse().ok()?;
                (prefix <= max).then_some(Self { ip, prefix })
            }
            None => {
                let ip: std::net::IpAddr = s.parse().ok()?;
                let prefix = if ip.is_ipv4() { 32 } else { 128 };
                Some(Self { ip, prefix })
            }
        }
    }

    fn contains(&self, addr: std::net::IpAddr) -> bool {
        fn bits(ip: std::net::IpAddr) -> (u128, u8) {
            match ip {
                std::net::IpAddr::V4(v4) => (u32::from(v4) as u128, 32),
                std::net::IpAddr::V6(v6) => (u128::from(v6), 128),
            }
        }
        let (net, net_len) = bits(self.ip);
        let (peer, peer_len) = bits(addr);
        if net_len != peer_len {
            return false; // v4 entry never matches a v6 peer and vice versa
        }
        if self.prefix == 0 {
            return true;
        }
        let shift = u32::from(net_len - self.prefix);
        (net >> shift) == (peer >> shift)
    }
}

async fn login(
    State(shared): State<Arc<Shared>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: header::HeaderMap,
    Json(body): Json<LoginBody>,
) -> Response {
    let ip = client_ip(&headers, peer);

    if let Some(secs) = shared.rate.locked_for(&ip) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({ "error": format!("Too many attempts. Try again in {secs}s.") })),
        )
            .into_response();
    }

    if !auth::password_matches(&shared.config.auth_password, &body.password) {
        shared.rate.record_failure(&ip);
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "Invalid password" })),
        )
            .into_response();
    }

    shared.rate.record_success(&ip);
    let gen = shared
        .ctx
        .session_gen
        .load(std::sync::atomic::Ordering::SeqCst);
    let token = auth::issue_token(
        &shared.config.session_secret,
        shared.config.session_ttl_secs,
        gen,
    );
    let mut cookie = format!(
        "{SESSION_COOKIE}={token}; HttpOnly; SameSite=Strict; Path=/; Max-Age={}",
        shared.config.session_ttl_secs
    );
    if let Some(domain) = &shared.config.cookie_domain {
        cookie.push_str(&format!("; Domain={domain}"));
    }

    let mut resp = Json(json!({ "token": token })).into_response();
    if let Ok(v) = header::HeaderValue::from_str(&cookie) {
        resp.headers_mut().insert(header::SET_COOKIE, v);
    }
    resp
}

async fn logout(State(shared): State<Arc<Shared>>, req: Request<Body>) -> Response {
    // Invalidate every outstanding token — not just this browser's cookie — by
    // bumping the session generation (persisted so it survives a restart),
    // exactly like `commands::power::power_off`. Without this, a copied token
    // stays valid until its TTL even after the user logs out. The bump only
    // happens for a request carrying a currently-valid token: `/logout` is an
    // open route, and an unauthenticated bump would let anyone kick the real
    // user out at will.
    let gen = shared
        .ctx
        .session_gen
        .load(std::sync::atomic::Ordering::SeqCst);
    let authed = extract_token(&req)
        .map(|t| auth::verify_token(&shared.config.session_secret, gen, &t))
        .unwrap_or(false);
    if authed {
        use rustic_app::sync_ext::MutexExt;
        let new_gen = shared
            .ctx
            .session_gen
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1;
        if let Err(e) = shared
            .ctx
            .state
            .db
            .lock_safe()
            .set_setting("session_generation", &new_gen.to_string())
        {
            tracing::warn!("logout: failed to persist session generation: {e}");
        }
    }

    let mut cookie = format!("{SESSION_COOKIE}=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0");
    if let Some(domain) = shared
        .ctx
        .tunnel
        .read()
        .ok()
        .and_then(|g| g.active_cookie_domain())
    {
        cookie.push_str(&format!("; Domain={domain}"));
    }
    let mut resp = Json(json!({ "ok": true })).into_response();
    if let Ok(v) = header::HeaderValue::from_str(&cookie) {
        resp.headers_mut().insert(header::SET_COOKIE, v);
    }
    resp
}

/// Match a preview-subdomain Host (`<port>.<preview-domain>`) to its port.
/// Only a single numeric leading label is accepted; the `:port` suffix on the
/// Host header (if any) is ignored.
// Kept for the planned subdomain-preview proxy mode (see proxy.rs docs) —
// currently unwired, so allow dead_code until the router mounts it.
#[allow(dead_code)]
fn match_preview_host(host: &str, preview_domain: &str) -> Option<u16> {
    let host = host.split(':').next()?;
    let label = host.strip_suffix(&format!(".{preview_domain}"))?;
    if label.is_empty() || label.contains('.') {
        return None;
    }
    label.parse::<u16>().ok()
}

/// Outermost layer: in subdomain mode, a request whose Host is
/// `<port>.<preview-domain>` is authed and forwarded to that loopback port
/// (covering every path on that host). All other hosts pass through untouched.
#[allow(dead_code)]
async fn host_proxy_middleware(
    State(shared): State<Arc<Shared>>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let Some(preview_domain) = shared.config.preview_domain.as_deref() else {
        return next.run(req).await;
    };
    let port = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .and_then(|h| match_preview_host(h, preview_domain));
    let Some(port) = port else {
        return next.run(req).await;
    };

    let gen = shared
        .ctx
        .session_gen
        .load(std::sync::atomic::Ordering::SeqCst);
    let valid = extract_token(&req)
        .map(|t| auth::verify_token(&shared.config.session_secret, gen, &t))
        .unwrap_or(false);
    if !valid {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "Authentication required" })),
        )
            .into_response();
    }

    proxy::forward_host(&shared, port, req).await
}

/// Reject any `/api/*` or `/ws` request lacking a valid session token. The
/// token is read from the `Authorization: Bearer` header or the session
/// cookie. For requests that can't carry either reliably — WebSocket upgrades
/// and download navigations — a `?ticket=` query param holding a single-use
/// ticket from `POST /api/ws_ticket` is also accepted (see
/// [`auth::TicketStore`]). The long-lived session token itself is never
/// accepted from the URL, so it can't leak into proxy/access logs.
async fn auth_middleware(
    State(shared): State<Arc<Shared>>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let gen = shared
        .ctx
        .session_gen
        .load(std::sync::atomic::Ordering::SeqCst);
    let token = extract_token(&req);
    let mut valid = token
        .map(|t| auth::verify_token(&shared.config.session_secret, gen, &t))
        .unwrap_or(false);

    // One-time ticket in the query string, accepted ONLY on WebSocket upgrade
    // paths and the download-navigation endpoint. Redeeming consumes it.
    if !valid && path_may_use_ticket(req.uri().path()) {
        if let Some(t) = extract_query_param(&req, "ticket") {
            valid = shared.tickets.redeem(&t, gen);
        }
    }

    if valid {
        next.run(req).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "Authentication required" })),
        )
            .into_response()
    }
}

/// Paths where a `?ticket=` credential is honored: WebSocket upgrades (a
/// browser `WebSocket` can't set headers) and the direct-navigation download
/// endpoint (an `<a>` click can't either; the session cookie normally covers
/// it, the ticket is the cookie-less fallback).
fn path_may_use_ticket(path: &str) -> bool {
    path == "/ws" || path.starts_with("/ws/") || path == "/api/download"
}

/// Extract the session token from the `Authorization: Bearer` header or the
/// session cookie. Deliberately does NOT read the query string: the long-lived
/// token must never ride in a URL (proxies log query strings) — URL-borne auth
/// goes through single-use tickets instead.
fn extract_token(req: &Request<Body>) -> Option<String> {
    // Bearer header.
    if let Some(auth) = req.headers().get(header::AUTHORIZATION) {
        if let Ok(s) = auth.to_str() {
            if let Some(tok) = s.strip_prefix("Bearer ") {
                return Some(tok.to_string());
            }
        }
    }
    // Cookie.
    if let Some(cookie) = req.headers().get(header::COOKIE) {
        if let Ok(s) = cookie.to_str() {
            for part in s.split(';') {
                let part = part.trim();
                if let Some(v) = part.strip_prefix(&format!("{SESSION_COOKIE}=")) {
                    return Some(v.to_string());
                }
            }
        }
    }
    None
}

/// Pull one query-string parameter (percent-decoded) off the request URI.
fn extract_query_param(req: &Request<Body>, name: &str) -> Option<String> {
    let q = req.uri().query()?;
    let prefix = format!("{name}=");
    for pair in q.split('&') {
        if let Some(v) = pair.strip_prefix(prefix.as_str()) {
            return Some(urldecode(v));
        }
    }
    None
}

fn urldecode(s: &str) -> String {
    // Minimal percent-decode for query params (tickets are plain hex, but a
    // proxy might still encode something — handle %XX anyway).
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `POST /api/ws_ticket` — mint a single-use, short-TTL WebSocket-auth ticket
/// bound to the current session generation. Sits inside the protected router,
/// so the caller must already hold a valid session credential.
async fn ws_ticket_handler(State(shared): State<Arc<Shared>>) -> Response {
    let gen = shared
        .ctx
        .session_gen
        .load(std::sync::atomic::Ordering::SeqCst);
    let ticket = shared.tickets.issue(gen);
    Json(json!({ "ticket": ticket })).into_response()
}

async fn api_handler(
    State(shared): State<Arc<Shared>>,
    AxumPath(command): AxumPath<String>,
    body: Option<Json<serde_json::Value>>,
) -> Response {
    let args = body.map(|Json(v)| v).unwrap_or(serde_json::Value::Null);
    match api::dispatch(&shared.ctx, &command, args).await {
        Ok(value) => Json(value).into_response(),
        Err(e) => {
            let status = StatusCode::from_u16(e.status).unwrap_or(StatusCode::BAD_REQUEST);
            (status, Json(json!({ "error": e.message }))).into_response()
        }
    }
}

#[derive(Deserialize)]
struct DownloadQuery {
    path: String,
}

/// `POST /api/sync/push` — receive a full-environment sync archive (tar.gz
/// body) and apply it in-process: the server's DB, secrets, file history and
/// project files become a copy of the sender's. See `rustic_app::cloud_sync`.
async fn sync_push_handler(State(shared): State<Arc<Shared>>, body: Body) -> Response {
    use futures_util::StreamExt;
    use tokio::io::AsyncWriteExt;

    let tmp = shared.config.data_dir.join("sync-upload.tar.zst");
    let _ = tokio::fs::remove_file(&tmp).await;
    {
        let mut file = match tokio::fs::File::create(&tmp).await {
            Ok(f) => f,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": e.to_string() })),
                )
                    .into_response()
            }
        };
        let mut stream = body.into_data_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = match chunk {
                Ok(c) => c,
                Err(e) => {
                    let _ = tokio::fs::remove_file(&tmp).await;
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({ "error": format!("upload stream error: {e}") })),
                    )
                        .into_response();
                }
            };
            if let Err(e) = file.write_all(&chunk).await {
                let _ = tokio::fs::remove_file(&tmp).await;
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": e.to_string() })),
                )
                    .into_response();
            }
        }
        if let Err(e) = file.flush().await {
            let _ = tokio::fs::remove_file(&tmp).await;
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    }

    let ctx = shared.ctx.clone();
    let tmp_apply = tmp.clone();
    let result = tokio::task::spawn_blocking(move || {
        use rustic_app::cloud_sync::{apply_sync_archive, safe_dir_name, SyncProjectEntry};

        let emitter: Arc<dyn rustic_app::EventEmitter> = Arc::new(ctx.clone());
        let projects_root = ctx.data_dir.join("projects");
        // Imported projects keep their existing server location when this
        // server already knows the project id; new ones land under
        // <data_dir>/projects/<name> (deduped against this import batch).
        let used: std::sync::Mutex<std::collections::HashSet<String>> = Default::default();
        let resolve = |entry: &SyncProjectEntry, old: Option<&str>| -> std::path::PathBuf {
            if let Some(old) = old {
                let p = std::path::PathBuf::from(old);
                if p.is_dir() {
                    return p;
                }
            }
            let base = safe_dir_name(&entry.name);
            let mut used = ctx_lock(&used);
            let mut candidate = base.clone();
            let mut n = 1;
            while !used.insert(candidate.clone()) {
                n += 1;
                candidate = format!("{base}-{n}");
            }
            projects_root.join(candidate)
        };
        apply_sync_archive(
            &ctx.state,
            &ctx.data_dir,
            &*ctx.secrets,
            &tmp_apply,
            emitter,
            &resolve,
        )
    })
    .await;
    let _ = tokio::fs::remove_file(&tmp).await;

    match result {
        Ok(Ok(manifest)) => {
            // The imported environment may carry a different git token — refresh
            // the terminal-git credential helper with it.
            let token = shared
                .ctx
                .state
                .git_token
                .lock()
                .ok()
                .and_then(|g| (*g).clone());
            crate::git_credentials::apply(&shared.config.data_dir, token.as_deref());
            Json(json!({ "ok": true, "projects": manifest.projects.len() })).into_response()
        }
        Ok(Err(e)) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e }))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// Poison-tolerant lock helper for the resolver's dedup set.
fn ctx_lock<'a, T>(m: &'a std::sync::Mutex<T>) -> std::sync::MutexGuard<'a, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// `GET /api/sync/state` — report this server's per-project sync fingerprints
/// (sync generation + whether files changed since) so a pushing client can
/// decide which project trees to skip. See `rustic_app::cloud_sync`.
async fn sync_state_handler(State(shared): State<Arc<Shared>>) -> Response {
    let ctx = shared.ctx.clone();
    let result = tokio::task::spawn_blocking(move || {
        rustic_app::cloud_sync::compute_peer_state(&ctx.state, &ctx.data_dir)
    })
    .await;
    match result {
        Ok(projects) => Json(json!({ "projects": projects })).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct SyncPullBody {
    /// The client's per-project sync state; projects both sides hold
    /// unchanged since a shared sync generation travel manifest-only.
    #[serde(default)]
    projects: Vec<rustic_app::cloud_sync::PeerProjectState>,
}

/// `POST /api/sync/pull` — build a full-environment sync archive of this
/// server and stream it back. The request body carries the client's sync
/// state so unchanged project trees are skipped. The temp file is deleted
/// when the response stream drops.
async fn sync_pull_handler(
    State(shared): State<Arc<Shared>>,
    body: Option<Json<SyncPullBody>>,
) -> Response {
    let client_state = body.map(|Json(b)| b.projects).unwrap_or_default();
    let ctx = shared.ctx.clone();
    let tmp = shared
        .config
        .data_dir
        .join(format!("sync-pull-{}.tar.zst", std::process::id()));
    let tmp_build = tmp.clone();
    let result = tokio::task::spawn_blocking(move || {
        let skips = rustic_app::cloud_sync::decide_skips(&ctx.state, &ctx.data_dir, &client_state);
        rustic_app::cloud_sync::build_sync_archive(
            &ctx.state,
            &ctx.data_dir,
            &*ctx.secrets,
            &tmp_build,
            &skips,
        )
    })
    .await;

    match result {
        Ok(Ok(_manifest)) => {}
        Ok(Err(e)) => {
            let _ = std::fs::remove_file(&tmp);
            return (StatusCode::BAD_REQUEST, Json(json!({ "error": e }))).into_response();
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    }

    let file = match tokio::fs::File::open(&tmp).await {
        Ok(f) => f,
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };
    let len = tokio::fs::metadata(&tmp).await.map(|m| m.len()).ok();
    let stream = TempFileStream {
        inner: Some(tokio_util::io::ReaderStream::new(file)),
        path: tmp,
    };
    let mut resp = (
        [
            (header::CONTENT_TYPE, "application/zstd".to_string()),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=\"rustic-sync.tar.zst\"".to_string(),
            ),
        ],
        Body::from_stream(stream),
    )
        .into_response();
    if let Some(len) = len {
        if let Ok(v) = header::HeaderValue::from_str(&len.to_string()) {
            resp.headers_mut().insert(header::CONTENT_LENGTH, v);
        }
    }
    resp
}


#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UploadStreamQuery {
    dst_dir: Option<String>,
    name: Option<String>,
    relative_path: Option<String>,
    path: Option<String>,
    offset: Option<u64>,
}

/// `POST /api/upload_stream` — chunked raw-byte upload: the first chunk
/// (`dstDir` + `name`, offset 0) resolves and returns the target path, later
/// chunks resend `path` + `offset` and append in place, so multi-GB files
/// stream to disk without ever being buffered or base64-encoded.
async fn upload_stream_handler(
    State(_shared): State<Arc<Shared>>,
    axum::extract::Query(q): axum::extract::Query<UploadStreamQuery>,
    body: Body,
) -> Response {
    use futures_util::StreamExt;
    use rustic_app::path_scope::validate_writable_path;
    use tokio::io::{AsyncSeekExt, AsyncWriteExt};

    use crate::commands::file_tree::{sanitize_relative, unique_destination};

    /// Build a JSON error response with the given status.
    fn err(status: StatusCode, msg: impl Into<String>) -> Response {
        (status, Json(json!({ "error": msg.into() }))).into_response()
    }

    let offset = q.offset.unwrap_or(0);

    let target = if let Some(p) = q.path.as_deref().filter(|p| !p.is_empty()) {
        std::path::PathBuf::from(p)
    } else {
        let Some(dst_dir) = q.dst_dir.as_deref().filter(|d| !d.is_empty()) else {
            return err(StatusCode::BAD_REQUEST, "missing dstDir or path");
        };
        let dst_dir = std::path::Path::new(dst_dir);
        if let Err(e) = validate_writable_path(dst_dir) {
            return err(StatusCode::FORBIDDEN, e);
        }
        if !dst_dir.is_dir() {
            return err(
                StatusCode::BAD_REQUEST,
                format!("Destination is not a directory: {}", dst_dir.display()),
            );
        }
        match q.relative_path.as_deref().filter(|r| !r.is_empty()) {
            Some(rel) => match sanitize_relative(rel) {
                Some(safe) => dst_dir.join(safe),
                None => {
                    return err(
                        StatusCode::BAD_REQUEST,
                        format!("Unsafe upload path: {rel}"),
                    )
                }
            },
            None => {
                let Some(name) = q.name.as_deref().filter(|n| !n.is_empty()) else {
                    return err(StatusCode::BAD_REQUEST, "missing name");
                };
                unique_destination(dst_dir, name)
            }
        }
    };

    if let Err(e) = validate_writable_path(&target) {
        return err(StatusCode::FORBIDDEN, e);
    }
    if let Some(parent) = target.parent() {
        if let Err(e) = tokio::fs::create_dir_all(parent).await {
            return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
        }
    }

    if offset > 0 {
        match tokio::fs::metadata(&target).await {
            Ok(m) if m.len() >= offset => {}
            Ok(m) => {
                return err(
                    StatusCode::BAD_REQUEST,
                    format!("upload offset {offset} is beyond current size {}", m.len()),
                )
            }
            Err(e) => {
                return err(
                    StatusCode::BAD_REQUEST,
                    format!("cannot continue upload: {e}"),
                )
            }
        }
    }

    let mut file = match tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .open(&target)
        .await
    {
        Ok(f) => f,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    if let Err(e) = file.set_len(offset).await {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    if let Err(e) = file.seek(std::io::SeekFrom::Start(offset)).await {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }

    let mut written: u64 = 0;
    let mut stream = body.into_data_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => return err(StatusCode::BAD_REQUEST, format!("upload stream error: {e}")),
        };
        if let Err(e) = file.write_all(&chunk).await {
            return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
        }
        written += chunk.len() as u64;
    }
    if let Err(e) = file.flush().await {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }

    Json(json!({ "path": target.to_string_lossy(), "size": offset + written })).into_response()
}

/// `GET /api/download?path=…` — stream a single file as a raw attachment, or a
/// directory as a generated zip. Path-scope guarded like every other read.
async fn download_handler(
    State(_shared): State<Arc<Shared>>,
    axum::extract::Query(q): axum::extract::Query<DownloadQuery>,
) -> Response {
    use rustic_app::path_scope::validate_readable_path;

    let path = std::path::PathBuf::from(&q.path);
    if let Err(e) = validate_readable_path(&path) {
        return (StatusCode::FORBIDDEN, Json(json!({ "error": e }))).into_response();
    }
    let meta = match std::fs::metadata(&path) {
        Ok(m) => m,
        Err(e) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": e.to_string() })),
            )
                .into_response()
        }
    };

    let file_name = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "download".to_string());

    if meta.is_dir() {
        // The archive is built on disk (never fully in memory) and streamed
        // out; the temp file is deleted when the response stream is dropped.
        let tmp = match tokio::task::spawn_blocking(move || zip_directory_to_temp(&path)).await {
            Ok(Ok(tmp)) => tmp,
            Ok(Err(e)) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": e })),
                )
                    .into_response()
            }
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": e.to_string() })),
                )
                    .into_response()
            }
        };
        let file = match tokio::fs::File::open(&tmp).await {
            Ok(f) => f,
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": e.to_string() })),
                )
                    .into_response();
            }
        };
        let zip_len = tokio::fs::metadata(&tmp).await.map(|m| m.len()).ok();
        let stream = TempFileStream {
            inner: Some(tokio_util::io::ReaderStream::new(file)),
            path: tmp,
        };
        let disposition = format!(
            "attachment; filename=\"{}.zip\"",
            sanitize_filename(&file_name)
        );
        let mut resp = (
            [
                (header::CONTENT_TYPE, "application/zip".to_string()),
                (header::CONTENT_DISPOSITION, disposition),
            ],
            Body::from_stream(stream),
        )
            .into_response();
        if let Some(len) = zip_len {
            if let Ok(v) = header::HeaderValue::from_str(&len.to_string()) {
                resp.headers_mut().insert(header::CONTENT_LENGTH, v);
            }
        }
        return resp;
    }

    let file = match tokio::fs::File::open(&path).await {
        Ok(f) => f,
        Err(e) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": e.to_string() })),
            )
                .into_response()
        }
    };
    let stream = tokio_util::io::ReaderStream::new(file);
    let disposition = format!("attachment; filename=\"{}\"", sanitize_filename(&file_name));
    (
        [
            (header::CONTENT_TYPE, "application/octet-stream".to_string()),
            (header::CONTENT_DISPOSITION, disposition),
        ],
        Body::from_stream(stream),
    )
        .into_response()
}

/// `GET /api/asset?path=…` — stream a single file INLINE (no attachment
/// disposition) with a guessed content-type and single-range support, so very
/// large binary previews (video/audio/PDF) can play without a base64
/// round-trip through `read_file_base64`. Path-scope guarded like every read.
async fn asset_handler(
    State(_shared): State<Arc<Shared>>,
    headers: HeaderMap,
    axum::extract::Query(q): axum::extract::Query<DownloadQuery>,
) -> Response {
    use rustic_app::path_scope::validate_readable_path;
    use tokio::io::{AsyncReadExt, AsyncSeekExt};

    let path = std::path::PathBuf::from(&q.path);
    if let Err(e) = validate_readable_path(&path) {
        return (StatusCode::FORBIDDEN, Json(json!({ "error": e }))).into_response();
    }
    let meta = match tokio::fs::metadata(&path).await {
        Ok(m) => m,
        Err(e) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": e.to_string() })),
            )
                .into_response()
        }
    };
    if meta.is_dir() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "path is a directory" })),
        )
            .into_response();
    }
    let total = meta.len();
    let mime = mime_for_path(&path);

    let mut file = match tokio::fs::File::open(&path).await {
        Ok(f) => f,
        Err(e) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": e.to_string() })),
            )
                .into_response()
        }
    };

    // `sandbox` keeps a directly-navigated asset (e.g. an SVG with embedded
    // script) from executing in the app's origin; <img>/<video> embedding is
    // unaffected.
    let csp = (header::CONTENT_SECURITY_POLICY, "sandbox".to_string());

    let range = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| parse_byte_range(s, total));
    if let Some((start, end)) = range {
        if file.seek(std::io::SeekFrom::Start(start)).await.is_err() {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "seek failed" })),
            )
                .into_response();
        }
        let len = end - start + 1;
        let stream = tokio_util::io::ReaderStream::new(file.take(len));
        return (
            StatusCode::PARTIAL_CONTENT,
            [
                (header::CONTENT_TYPE, mime.to_string()),
                (
                    header::CONTENT_RANGE,
                    format!("bytes {}-{}/{}", start, end, total),
                ),
                (header::CONTENT_LENGTH, len.to_string()),
                (header::ACCEPT_RANGES, "bytes".to_string()),
                csp,
            ],
            Body::from_stream(stream),
        )
            .into_response();
    }

    let stream = tokio_util::io::ReaderStream::new(file);
    (
        [
            (header::CONTENT_TYPE, mime.to_string()),
            (header::CONTENT_LENGTH, total.to_string()),
            (header::ACCEPT_RANGES, "bytes".to_string()),
            csp,
        ],
        Body::from_stream(stream),
    )
        .into_response()
}

/// Parse a single `bytes=start-end` / `bytes=start-` / `bytes=-suffix` range,
/// clamped to the file size. Multi-range requests fall back to a full response.
fn parse_byte_range(header_value: &str, total: u64) -> Option<(u64, u64)> {
    if total == 0 {
        return None;
    }
    let spec = header_value.strip_prefix("bytes=")?;
    if spec.contains(',') {
        return None;
    }
    let (start_s, end_s) = spec.split_once('-')?;
    let (start_s, end_s) = (start_s.trim(), end_s.trim());
    if start_s.is_empty() {
        let suffix: u64 = end_s.parse().ok()?;
        if suffix == 0 {
            return None;
        }
        return Some((total.saturating_sub(suffix), total - 1));
    }
    let start: u64 = start_s.parse().ok()?;
    if start >= total {
        return None;
    }
    let end: u64 = if end_s.is_empty() {
        total - 1
    } else {
        end_s.parse().ok()?
    };
    if end < start {
        return None;
    }
    Some((start, end.min(total - 1)))
}

/// Minimal extension→MIME map for preview streaming (avoids a new dependency).
fn mime_for_path(path: &std::path::Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("svg") => "image/svg+xml",
        Some("bmp") => "image/bmp",
        Some("ico") => "image/x-icon",
        Some("avif") => "image/avif",
        Some("mp4") | Some("m4v") => "video/mp4",
        Some("webm") => "video/webm",
        Some("mov") => "video/quicktime",
        Some("mkv") => "video/x-matroska",
        Some("mp3") => "audio/mpeg",
        Some("wav") => "audio/wav",
        Some("ogg") => "audio/ogg",
        Some("m4a") => "audio/mp4",
        Some("flac") => "audio/flac",
        Some("pdf") => "application/pdf",
        _ => "application/octet-stream",
    }
}

/// Strip characters that would break a `Content-Disposition` filename or allow
/// header injection; keep it conservative (no quotes, CR/LF, or path seps).
fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '"' | '\\' | '/' | '\r' | '\n' => '_',
            other => other,
        })
        .collect()
}

/// Recursively zip a directory into a uniquely-named TEMP FILE (the `zip`
/// writer needs `Seek`, so it can't target the HTTP body directly — but a temp
/// file keeps the archive out of memory entirely; the caller streams it and
/// [`TempFileStream`] deletes it afterwards). Entries are stored relative to
/// the directory's parent so the archive expands into a folder of the same
/// name. Symlinks are skipped (their targets may be out of scope).
fn zip_directory_to_temp(root: &std::path::Path) -> Result<std::path::PathBuf, String> {
    let tmp = std::env::temp_dir().join(format!("rustic-download-{}.zip", uuid::Uuid::new_v4()));
    match write_zip(root, &tmp) {
        Ok(()) => Ok(tmp),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

fn write_zip(root: &std::path::Path, out: &std::path::Path) -> Result<(), String> {
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    let base = root.parent().unwrap_or(root);
    let file = std::fs::File::create(out).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipWriter::new(std::io::BufWriter::new(file));
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    for entry in walkdir_files(root) {
        let rel = match entry.strip_prefix(base) {
            Ok(r) => r.to_string_lossy().replace('\\', "/"),
            Err(_) => continue,
        };
        let md = match std::fs::symlink_metadata(&entry) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if md.file_type().is_symlink() {
            continue;
        }
        if md.is_dir() {
            let dir_entry = format!("{}/", rel);
            zip.add_directory(dir_entry, options)
                .map_err(|e| e.to_string())?;
        } else if md.is_file() {
            zip.start_file(rel, options).map_err(|e| e.to_string())?;
            let mut f = std::fs::File::open(&entry).map_err(|e| e.to_string())?;
            // Streaming copy — never the whole file in memory at once.
            std::io::copy(&mut f, &mut zip).map_err(|e| e.to_string())?;
        }
    }

    let mut inner = zip.finish().map_err(|e| e.to_string())?;
    inner.flush().map_err(|e| e.to_string())?;
    Ok(())
}

/// A byte stream that deletes its backing temp file once dropped (response
/// fully sent or connection aborted). The inner stream (which owns the open
/// file handle) is dropped FIRST so the delete also works on Windows.
struct TempFileStream<S> {
    inner: Option<S>,
    path: std::path::PathBuf,
}

impl<S> futures_util::Stream for TempFileStream<S>
where
    S: futures_util::Stream + Unpin,
{
    type Item = S::Item;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let this = self.get_mut();
        match this.inner.as_mut() {
            Some(s) => std::pin::Pin::new(s).poll_next(cx),
            None => std::task::Poll::Ready(None),
        }
    }
}

impl<S> Drop for TempFileStream<S> {
    fn drop(&mut self) {
        self.inner = None; // close the file handle before unlinking
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Iterative directory walk returning every descendant path (dirs + files),
/// skipping the `.git` directory to keep repo downloads sane.
fn walkdir_files(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let rd = match std::fs::read_dir(&dir) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for entry in rd.flatten() {
            let p = entry.path();
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if is_dir {
                if p.file_name().map(|n| n == ".git").unwrap_or(false) {
                    continue;
                }
                out.push(p.clone());
                stack.push(p);
            } else {
                out.push(p);
            }
        }
    }
    out
}

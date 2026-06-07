//! Router assembly: login, auth middleware, the `/api/:command` dispatch,
//! `/ws`, health, and static SPA serving.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    body::Body,
    extract::{ConnectInfo, Path as AxumPath, State},
    http::{header, Request, StatusCode},
    middleware::{from_fn_with_state, Next},
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
        .route("/api/:command", post(api_handler))
        .route("/api/download", get(download_handler))
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
        .layer(from_fn_with_state(shared.clone(), auth_middleware));

    Router::new()
        .route("/healthz", get(health))
        .route("/login", post(login))
        .route("/logout", post(logout))
        .merge(protected)
        .nest_service("/assets", assets_service)
        .fallback_service(static_service)
        .with_state(shared)
}

async fn health() -> impl IntoResponse {
    Json(json!({ "status": "ok" }))
}

#[derive(Deserialize)]
struct LoginBody {
    password: String,
}

/// Extract the best-effort client IP for rate limiting: the first
/// `X-Forwarded-For` hop (set by the reverse proxy) or the socket peer.
fn client_ip(headers: &header::HeaderMap, peer: SocketAddr) -> String {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| peer.ip().to_string())
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
    let gen = shared.ctx.session_gen.load(std::sync::atomic::Ordering::SeqCst);
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

async fn logout(State(shared): State<Arc<Shared>>) -> Response {
    let mut cookie = format!("{SESSION_COOKIE}=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0");
    if let Some(domain) = shared.ctx.tunnel.read().ok().and_then(|g| g.active_cookie_domain()) {
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

    let gen = shared.ctx.session_gen.load(std::sync::atomic::Ordering::SeqCst);
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

    proxy::forward_host(port, req).await
}

/// Reject any `/api/*` or `/ws` request lacking a valid session token. The
/// token is read from (in order) the `Authorization: Bearer` header, the
/// session cookie, or a `?token=` query param (the only option a browser
/// WebSocket can carry besides the auto-sent cookie).
async fn auth_middleware(
    State(shared): State<Arc<Shared>>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let gen = shared.ctx.session_gen.load(std::sync::atomic::Ordering::SeqCst);
    let token = extract_token(&req);
    let valid = token
        .map(|t| auth::verify_token(&shared.config.session_secret, gen, &t))
        .unwrap_or(false);

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
    // Query param (browser WebSocket).
    if let Some(q) = req.uri().query() {
        for pair in q.split('&') {
            if let Some(v) = pair.strip_prefix("token=") {
                return Some(urldecode(v));
            }
        }
    }
    None
}

fn urldecode(s: &str) -> String {
    // Minimal percent-decode for the token query param (tokens are
    // `digits.hex`, but a proxy might encode the dot — handle %XX anyway).
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
            return (StatusCode::NOT_FOUND, Json(json!({ "error": e.to_string() }))).into_response()
        }
    };

    let file_name = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "download".to_string());

    if meta.is_dir() {
        let zip_bytes = match tokio::task::spawn_blocking(move || zip_directory(&path)).await {
            Ok(Ok(bytes)) => bytes,
            Ok(Err(e)) => {
                return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e })))
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
        let disposition = format!("attachment; filename=\"{}.zip\"", sanitize_filename(&file_name));
        return (
            [
                (header::CONTENT_TYPE, "application/zip".to_string()),
                (header::CONTENT_DISPOSITION, disposition),
            ],
            zip_bytes,
        )
            .into_response();
    }

    let file = match tokio::fs::File::open(&path).await {
        Ok(f) => f,
        Err(e) => {
            return (StatusCode::NOT_FOUND, Json(json!({ "error": e.to_string() }))).into_response()
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

/// Recursively zip a directory into an in-memory buffer. Entries are stored
/// relative to the directory's parent so the archive expands into a folder of
/// the same name. Symlinks are skipped (their targets may be out of scope).
fn zip_directory(root: &std::path::Path) -> Result<Vec<u8>, String> {
    use std::io::{Cursor, Read, Write};
    use zip::write::SimpleFileOptions;

    let base = root.parent().unwrap_or(root);
    let mut cursor = Cursor::new(Vec::new());
    let mut zip = zip::ZipWriter::new(&mut cursor);
    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

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
            zip.add_directory(dir_entry, options).map_err(|e| e.to_string())?;
        } else if md.is_file() {
            zip.start_file(rel, options).map_err(|e| e.to_string())?;
            let mut f = std::fs::File::open(&entry).map_err(|e| e.to_string())?;
            let mut buf = Vec::new();
            f.read_to_end(&mut buf).map_err(|e| e.to_string())?;
            zip.write_all(&buf).map_err(|e| e.to_string())?;
        }
    }

    zip.finish().map_err(|e| e.to_string())?;
    Ok(cursor.into_inner())
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

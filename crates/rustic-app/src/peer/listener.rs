//! Peer HTTP routes + optional TLS listener. Unauthenticated: `info`,
//! `pair`. Token-authenticated (a paired device's `token_in`): `request`
//! (push/pull approval) and the rustic-server-compatible `api/sync/*`,
//! `api/list_projects`, `api/sync/v2/*` routes.
//!
//! [`build_router`] serves everything under `/lan/*`; with `legacy_root` the
//! `api/*` routes are ALSO served at the root (older desktop clients).
//! rustic-server mounts it without `legacy_root` (it owns root `/api/sync`).

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use super::{consent, HostRef, Identity, LanState, Peer};

#[derive(Clone)]
struct Ctx {
    host: HostRef,
    lan: LanState,
    uploads: Arc<std::sync::Mutex<std::collections::HashMap<String, Arc<UploadSession>>>>,
}

/// A resumable upload being received from a paired device.
struct UploadSession {
    device_id: String,
    peer_name: String,
    dest: PathBuf,
    plan: std::collections::HashMap<String, Option<String>>,
    sizes: std::collections::HashMap<String, (bool, u64)>,
    summary: std::sync::Mutex<super::files::UnpackSummary>,
    received: std::sync::atomic::AtomicU64,
    last_chunk: std::sync::Mutex<std::time::Instant>,
    handle: Arc<crate::transfers::Handle>,
    done: std::sync::atomic::AtomicBool,
}

impl Ctx {
    fn emit(&self, event: &str, payload: serde_json::Value) {
        self.host.emitter().emit_json(event, payload);
    }
}

/// Data dir or an HTTP 500.
#[allow(clippy::result_large_err)] // the Err is the ready-to-send HTTP response
fn data_dir(ctx: &Ctx) -> Result<PathBuf, Response> {
    ctx.host
        .data_dir()
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))
}

/// JSON error response.
fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": msg.into() }))).into_response()
}

/// Resolve the paired device calling us from its `Authorization: Bearer` token.
#[allow(clippy::result_large_err)]
fn authorize(ctx: &Ctx, headers: &HeaderMap) -> Result<Peer, Response> {
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or_else(|| err(StatusCode::UNAUTHORIZED, "not paired"))?;
    let dir = data_dir(ctx)?;
    super::load_peers(&dir)
        .into_iter()
        .find(|p| !p.token_in.is_empty() && p.token_in == token)
        .ok_or_else(|| {
            tracing::warn!("peer: rejected request with an unknown pairing token (device forgot us or we forgot it)");
            err(StatusCode::UNAUTHORIZED, "unknown device — pair again")
        })
}

/// Caller's socket address. The TLS listener attaches it per connection; a
/// host mounting [`build_router`] elsewhere may add it as an
/// `axum::Extension(Remote(addr))` layer (optional).
#[derive(Clone, Copy)]
pub struct Remote(pub std::net::SocketAddr);

/// Where the caller can be reached: its advertised public URL, else its
/// socket IP plus the port it advertises in [`super::PORT_HEADER`].
/// Loopback callers are cloudflared relaying a tunnel request — their socket
/// IP says nothing about where the peer is.
fn caller_addr(headers: &HeaderMap, remote: Option<Remote>) -> Option<String> {
    if let Some(url) = headers
        .get(super::URL_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|u| u.starts_with("https://"))
    {
        return Some(url.trim_end_matches('/').to_string());
    }
    let ip = remote?.0.ip();
    if ip.is_loopback() {
        return None;
    }
    let port: u16 = headers
        .get(super::PORT_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse().ok())
        .filter(|p| *p != 0)?;
    Some(std::net::SocketAddr::new(ip, port).to_string())
}

/// Middleware: refuse every peer route except `/lan/info` unless the caller
/// runs exactly our version (old builds send no version header at all).
async fn require_same_version(req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    let path = req.uri().path();
    if path.ends_with("/info") {
        return next.run(req).await;
    }
    let theirs = req
        .headers()
        .get(super::VERSION_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    if theirs.as_deref() == Some(super::app_version()) {
        return next.run(req).await;
    }
    tracing::warn!(
        path,
        theirs = theirs.as_deref().unwrap_or("<none>"),
        ours = super::app_version(),
        "peer: refused request from a different Rustic version"
    );
    (
        StatusCode::CONFLICT,
        Json(json!({
            "error": format!("Version mismatch: this machine runs Rustic v{}, yours is {}. Update both to the same version.",
                super::app_version(), theirs.as_deref().map(|v| format!("v{v}")).unwrap_or_else(|| "older".into())),
            "code": super::VERSION_MISMATCH,
            "version": super::app_version(),
        })),
    )
        .into_response()
}

/// Middleware: any request from a paired device marks it online and refreshes
/// its saved address. This is what lets the machine being *called* show the
/// caller as online even when mDNS only works in one direction.
async fn track_seen(
    State(ctx): State<Ctx>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let token = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_string);
    if let Some(token) = token.filter(|t| !t.is_empty()) {
        let remote = req.extensions().get::<Remote>().copied();
        let addr = caller_addr(req.headers(), remote);
        if let Ok(dir) = ctx.host.data_dir() {
            let mut peers = super::load_peers(&dir);
            if let Some(p) = peers.iter_mut().find(|p| p.token_in == token) {
                ctx.lan.lock().mark_seen(&p.device_id, addr.clone());
                if addr.is_some() && p.addr != addr {
                    tracing::info!(device = %p.device_id, old = ?p.addr, new = ?addr, "peer: device address changed");
                    p.addr = addr;
                    let _ = super::save_peers(&dir, &peers);
                }
            }
        }
    }
    next.run(req).await
}

/// Peer routes for `host`. Everything is served under `/lan`
/// (`/lan/info`, `/lan/pair`, `/lan/request`, `/lan/api/sync/*`,
/// `/lan/api/list_projects`, `/lan/api/sync/v2/*`); `legacy_root` also
/// serves the `api/*` routes at the root. Clears leftover chunked-transfer
/// sessions in `<data>/lan-sync-sessions`. `/lan/info` answers 503 until
/// `lan.identity` is set (see [`super::ops::start`] / [`super::ops::start_identity_only`]).
pub fn build_router(host: HostRef, lan: LanState, legacy_root: bool) -> Result<Router, String> {
    let ctx = Ctx { host, lan, uploads: Arc::default() };
    let sessions_dir = ctx.host.data_dir()?.join("lan-sync-sessions");
    let _ = std::fs::remove_dir_all(&sessions_dir); // leftovers from a previous run
    let transfer = Arc::new(LanTransferHost {
        ctx: ctx.clone(),
        sessions: crate::transfer::session::SessionManager::new(sessions_dir),
    });
    let api: Router<Ctx> = Router::new()
        // Chunked, parallel, resumable sync (streamed pack ‖ send ‖ extract).
        .merge(crate::transfer::routes::router::<LanTransferHost, Ctx>(transfer))
        .route("/api/sync/state", get(sync_state))
        .route(
            "/api/sync/push",
            post(sync_push).layer(axum::extract::DefaultBodyLimit::disable()),
        )
        .route("/api/sync/pull", post(sync_pull))
        .route("/api/list_projects", post(list_projects))
        .route("/api/fs/list", post(fs_list))
        .route("/api/fs/preview", post(fs_preview))
        .route("/api/fs/size", post(fs_size))
        .route("/api/fs/exists", post(fs_exists))
        .route("/api/fs/manifest", post(fs_manifest))
        .route("/api/fs/read", post(fs_read))
        .route("/api/fs/upload/begin", post(upload_begin))
        .route("/api/fs/upload/offset", post(upload_offset))
        .route(
            "/api/fs/upload/chunk",
            post(upload_chunk).layer(axum::extract::DefaultBodyLimit::disable()),
        )
        .route("/api/fs/upload/finish", post(upload_finish))
        .route("/api/fs/pull", post(fs_pull))
        .route(
            "/api/fs/push",
            post(fs_push).layer(axum::extract::DefaultBodyLimit::disable()),
        )
        .route(
            "/api/sync/meta",
            get(meta_export)
                .post(meta_import)
                .layer(axum::extract::DefaultBodyLimit::max(256 * 1024 * 1024)),
        );
    let ctl: Router<Ctx> = Router::new()
        .route("/info", get(info))
        .route("/hello", post(hello))
        .route("/pair", post(pair))
        .route("/pair/cancel", post(pair_cancel))
        .route("/unpair", post(unpair))
        .route("/request", post(transfer_request))
        .route("/request/cancel", post(transfer_cancel));
    let mut router: Router<Ctx> = Router::new().nest("/lan", ctl.merge(api.clone()));
    if legacy_root {
        router = router.merge(api);
    }
    Ok(router
        .layer(axum::middleware::from_fn_with_state(ctx.clone(), track_seen))
        .layer(axum::middleware::from_fn(require_same_version))
        .with_state(ctx))
}

/// Start the TLS listener (this install's self-signed certificate, pinned by
/// peers) on [`super::DEFAULT_PORT`], or an OS-assigned port when that one is
/// taken, serving [`build_router`] with legacy root routes. Returns the port.
pub async fn start(host: HostRef, lan: LanState, id: Identity) -> Result<u16, String> {
    let tls = super::server_tls_config(&id)?;
    let acceptor = tokio_rustls::TlsAcceptor::from(tls);
    let listener = bind_listener().await?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    tracing::info!(port, version = super::app_version(), "peer: LAN listener started");
    let router = build_router(host, lan.clone(), true)?;

    let (tx, mut rx) = tokio::sync::oneshot::channel::<()>();
    {
        let mut inner = lan.lock();
        inner.port = port;
        inner.shutdown = Some(tx);
    }
    super::set_listen_port(port);
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut rx => break,
                accepted = listener.accept() => {
                    let Ok((stream, addr)) = accepted else { continue };
                    let acceptor = acceptor.clone();
                    let router = router.clone().layer(axum::Extension(Remote(addr)));
                    tokio::spawn(async move {
                        let Ok(tls) = acceptor.accept(stream).await else { return };
                        let io = hyper_util::rt::TokioIo::new(tls);
                        let svc = hyper_util::service::TowerToHyperService::new(router);
                        let _ = hyper_util::server::conn::auto::Builder::new(hyper_util::rt::TokioExecutor::new())
                            .serve_connection(io, svc)
                            .await;
                    });
                }
            }
        }
    });
    Ok(port)
}

/// Bind [`super::DEFAULT_PORT`], retrying for ~8s (a previous instance may
/// still be shutting down after an update / restart — saved peer addresses
/// point at this port), then fall back to an OS-assigned port.
async fn bind_listener() -> Result<tokio::net::TcpListener, String> {
    let mut last_err = String::new();
    for attempt in 0..16 {
        match tokio::net::TcpListener::bind(("0.0.0.0", super::DEFAULT_PORT)).await {
            Ok(l) => {
                if attempt > 0 {
                    tracing::info!(attempt, "peer: LAN port {} became free", super::DEFAULT_PORT);
                }
                return Ok(l);
            }
            Err(e) => last_err = e.to_string(),
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    tracing::warn!(
        "peer: LAN port {} still unavailable after retries ({last_err}); using a random port — peers will learn it on the next contact",
        super::DEFAULT_PORT
    );
    tokio::net::TcpListener::bind("0.0.0.0:0")
        .await
        .map_err(|e| format!("LAN listener bind failed: {e}"))
}

/// `GET /lan/info` — who this device is.
async fn info(State(ctx): State<Ctx>) -> Response {
    let inner = ctx.lan.lock();
    match inner.identity.as_ref() {
        Some(id) => Json(json!({ "device_id": id.device_id, "name": id.device_name, "fingerprint": id.fingerprint, "version": super::app_version() })).into_response(),
        None => err(StatusCode::SERVICE_UNAVAILABLE, "local-network sync is off"),
    }
}

/// `POST /lan/hello` — a paired device checks in (after an IP change, or as
/// an authenticated probe). `track_seen` already refreshed its address.
async fn hello(State(ctx): State<Ctx>, headers: HeaderMap) -> Response {
    let peer = match authorize(&ctx, &headers) {
        Ok(p) => p,
        Err(r) => return r,
    };
    ctx.emit("lan-peers-changed", json!({}));
    Json(json!({ "ok": true, "device_id": peer.device_id, "version": super::app_version() })).into_response()
}

/// `POST /lan/unpair` — a paired device forgot us; forget it too so neither
/// side is left holding a dead pairing.
async fn unpair(State(ctx): State<Ctx>, headers: HeaderMap) -> Response {
    let peer = match authorize(&ctx, &headers) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let dir = match data_dir(&ctx) {
        Ok(d) => d,
        Err(r) => return r,
    };
    let mut peers = super::load_peers(&dir);
    peers.retain(|p| p.device_id != peer.device_id);
    if let Err(e) = super::save_peers(&dir, &peers) {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e);
    }
    tracing::info!(device = %peer.device_id, name = %peer.display_name(), "peer: device unpaired itself; forgot it here too");
    ctx.emit("lan-peers-changed", json!({}));
    ctx.emit("lan-peer-unpaired", json!({ "device_id": peer.device_id, "name": peer.display_name() }));
    Json(json!({ "ok": true })).into_response()
}

#[derive(Deserialize)]
struct PairCancel {
    device_id: String,
}

/// `POST /lan/pair/cancel` — the requester gave up; dismiss its prompt here.
async fn pair_cancel(State(ctx): State<Ctx>, Json(req): Json<PairCancel>) -> Response {
    let tx = {
        let mut inner = ctx.lan.lock();
        let rid = inner.pair_by_device.remove(&req.device_id);
        rid.and_then(|rid| inner.pending_pairs.remove(&rid).map(|tx| (rid, tx)))
    };
    if let Some((rid, tx)) = tx {
        let _ = tx.send(false);
        tracing::info!(device = %req.device_id, "peer: pairing request cancelled by the requester");
        ctx.emit("lan-pair-cancelled", json!({ "request_id": rid }));
    }
    Json(json!({ "ok": true })).into_response()
}

/// `POST /lan/request/cancel` — the requester stopped waiting; dismiss its
/// pending approval prompts here.
async fn transfer_cancel(State(ctx): State<Ctx>, headers: HeaderMap) -> Response {
    let peer = match authorize(&ctx, &headers) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let cancelled: Vec<(String, tokio::sync::oneshot::Sender<bool>)> = {
        let mut inner = ctx.lan.lock();
        let ids = inner.transfer_by_device.remove(&peer.device_id).unwrap_or_default();
        ids.into_iter()
            .filter_map(|id| inner.pending_transfers.remove(&id).map(|tx| (id, tx)))
            .collect()
    };
    for (id, tx) in cancelled {
        let _ = tx.send(false);
        ctx.emit("lan-transfer-cancelled", json!({ "request_id": id }));
    }
    tracing::info!(device = %peer.device_id, "peer: transfer request cancelled by the requester");
    Json(json!({ "ok": true })).into_response()
}

#[derive(Deserialize)]
struct PairRequest {
    device_id: String,
    name: String,
    fingerprint: String,
    /// Token THIS device will present when calling the requester.
    token: String,
}

/// `POST /lan/pair` — ask the user here to Accept/Decline; on accept, both
/// sides exchange tokens and pin each other's certificates.
async fn pair(
    State(ctx): State<Ctx>,
    remote: Option<axum::Extension<Remote>>,
    headers: HeaderMap,
    Json(req): Json<PairRequest>,
) -> Response {
    let caller = caller_addr(&headers, remote.map(|axum::Extension(r)| r));
    let (my, request_id, rx) = {
        let mut inner = ctx.lan.lock();
        let Some(id) = inner.identity.clone() else {
            return err(StatusCode::SERVICE_UNAVAILABLE, "local-network sync is off");
        };
        let request_id = super::random_token().unwrap_or_default();
        let (tx, rx) = tokio::sync::oneshot::channel::<bool>();
        // A repeated request from the same device replaces its old prompt.
        if let Some(old) = inner.pair_by_device.insert(req.device_id.clone(), request_id.clone()) {
            inner.pending_pairs.remove(&old);
            ctx.host.emitter().emit_json("lan-pair-cancelled", json!({ "request_id": old }));
        }
        inner.pending_pairs.insert(request_id.clone(), tx);
        (id, request_id, rx)
    };
    tracing::info!(device = %req.device_id, name = %req.name, caller = ?caller, "peer: incoming pairing request — prompting the user");
    let code = super::pairing_code(&my.fingerprint, &req.fingerprint);
    ctx.emit(
        "lan-pair-request",
        json!({ "request_id": request_id, "device_id": req.device_id, "name": req.name, "code": code }),
    );
    let accepted = matches!(
        tokio::time::timeout(std::time::Duration::from_secs(90), rx).await,
        Ok(Ok(true))
    );
    {
        let mut inner = ctx.lan.lock();
        let was_pending = inner.pending_pairs.remove(&request_id).is_some();
        if inner.pair_by_device.get(&req.device_id) == Some(&request_id) {
            inner.pair_by_device.remove(&req.device_id);
        }
        if was_pending {
            // Timed out with the prompt still open.
            ctx.host.emitter().emit_json("lan-pair-cancelled", json!({ "request_id": request_id }));
        }
    }
    tracing::info!(device = %req.device_id, accepted, "peer: pairing request answered");
    if !accepted {
        return Json(json!({ "accepted": false })).into_response();
    }
    let token_in = match super::random_token() {
        Ok(t) => t,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    let dir = match data_dir(&ctx) {
        Ok(d) => d,
        Err(r) => return r,
    };
    let addr = caller.or_else(|| {
        ctx.lan
            .lock()
            .discovered
            .get(&req.device_id)
            .map(|d| d.addr.clone())
    });
    let existing = super::load_peers(&dir)
        .into_iter()
        .find(|p| p.device_id == req.device_id);
    let nickname = existing.as_ref().and_then(|p| p.nickname.clone());
    let meta_view = existing.as_ref().is_some_and(|p| p.meta_view);
    let share = existing.map(|p| p.share).unwrap_or_default();
    {
        let mut inner = ctx.lan.lock();
        inner.mark_seen(&req.device_id, addr.clone());
        inner.needs_repair.remove(&req.device_id);
        inner.versions.insert(req.device_id.clone(), super::app_version().to_string());
    }
    let peer = Peer {
        device_id: req.device_id,
        name: req.name,
        fingerprint: req.fingerprint,
        token_out: req.token,
        token_in: token_in.clone(),
        addr,
        nickname,
        share,
        meta_view,
    };
    if let Err(e) = super::upsert_peer(&dir, peer) {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e);
    }
    ctx.emit("lan-peers-changed", json!({}));
    Json(json!({
        "accepted": true,
        "device_id": my.device_id,
        "name": my.device_name,
        "fingerprint": my.fingerprint,
        "token": token_in,
    }))
    .into_response()
}

/// `POST /lan/request` — a paired device asks to push items here or pull
/// items from here. Pulls are limited to what we share with it. The user on
/// THIS machine sees the exact list and approves or denies (pushes are
/// approved immediately when the host auto-accepts them); approval returns a
/// ticket the device must present on the actual transfer requests.
async fn transfer_request(
    State(ctx): State<Ctx>,
    headers: HeaderMap,
    Json(req): Json<consent::TransferRequest>,
) -> Response {
    let peer = match authorize(&ctx, &headers) {
        Ok(p) => p,
        Err(r) => return r,
    };
    if let Err(e) = consent::validate(&req) {
        return err(StatusCode::BAD_REQUEST, e);
    }
    if req.kind == "pull" {
        let mut check = req.clone();
        if peer.meta_view {
            check.meta.clear();
        }
        let denied = consent::unshared_items(&check, &peer.share);
        if !denied.is_empty() {
            return err(
                StatusCode::FORBIDDEN,
                format!("not shared with you: {}", denied.join(", ")),
            );
        }
    }
    // Push: tell the receiver which incoming projects would replace one it has.
    let mut req = req;
    if req.kind == "push" {
        let local: std::collections::HashSet<String> = ctx
            .host
            .state()
            .workspace
            .lock()
            .map(|ws| ws.list_projects().into_iter().map(|p| p.id.to_string()).collect())
            .unwrap_or_default();
        for p in &mut req.projects {
            p.exists = local.contains(&p.id);
        }
    }
    let request_id = super::random_token().unwrap_or_default();
    tracing::info!(
        device = %peer.device_id,
        kind = %req.kind,
        projects = req.projects.len(),
        meta = req.meta.len(),
        files = req.files.len(),
        "peer: incoming transfer request"
    );
    let payload = json!({
        "request_id": request_id,
        "device_id": peer.device_id,
        "from": peer.display_name(),
        "kind": req.kind,
        "projects": req.projects,
        "meta": req.meta,
        "files": req.files,
        "meta_access": req.meta_access,
    });
    let approved = if req.kind == "push" && ctx.host.auto_accept_push() {
        ctx.emit("lan-transfer-auto-accepted", payload);
        true
    } else {
        let (tx, rx) = tokio::sync::oneshot::channel::<bool>();
        {
            let mut inner = ctx.lan.lock();
            inner.pending_transfers.insert(request_id.clone(), tx);
            inner
                .transfer_by_device
                .entry(peer.device_id.clone())
                .or_default()
                .push(request_id.clone());
        }
        ctx.emit("lan-transfer-request", payload);
        let ok = matches!(
            tokio::time::timeout(consent::PROMPT_TIMEOUT, rx).await,
            Ok(Ok(true))
        );
        let mut inner = ctx.lan.lock();
        if inner.pending_transfers.remove(&request_id).is_some() {
            ctx.host.emitter().emit_json("lan-transfer-cancelled", json!({ "request_id": request_id }));
        }
        if let Some(ids) = inner.transfer_by_device.get_mut(&peer.device_id) {
            ids.retain(|i| i != &request_id);
        }
        ok
    };
    tracing::info!(device = %peer.device_id, approved, "peer: transfer request answered");
    if approved && req.kind == "meta_access" {
        if let Ok(dir) = data_dir(&ctx) {
            let mut peers = super::load_peers(&dir);
            if let Some(p) = peers.iter_mut().find(|p| p.device_id == peer.device_id) {
                p.meta_view = true;
                let _ = super::save_peers(&dir, &peers);
            }
        }
        return Json(json!({ "approved": true, "ticket": "" })).into_response();
    }
    if !approved {
        return Json(json!({ "approved": false })).into_response();
    }
    let ticket_id = super::random_token().unwrap_or_default();
    {
        let mut inner = ctx.lan.lock();
        inner.tickets.retain(|_, t| t.expires > std::time::Instant::now());
        inner
            .tickets
            .insert(ticket_id.clone(), consent::Ticket::for_request(&peer.device_id, &req));
    }
    Json(json!({ "approved": true, "ticket": ticket_id })).into_response()
}

/// The approved ticket on this request, if it lets `peer` do a `kind` transfer.
#[allow(clippy::result_large_err)]
fn ticket(ctx: &Ctx, headers: &HeaderMap, peer: &Peer, kind: &str) -> Result<consent::Ticket, Response> {
    let id = headers
        .get(super::TICKET_HEADER)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| {
            err(
                StatusCode::FORBIDDEN,
                format!("this {kind} needs approval on {} first", my_name(ctx)),
            )
        })?;
    let mut inner = ctx.lan.lock();
    let t = inner
        .tickets
        .get_mut(id)
        .filter(|t| t.valid_for(&peer.device_id, kind))
        .ok_or_else(|| err(StatusCode::FORBIDDEN, "approval expired or doesn't cover this — ask again"))?;
    // Keep an approval alive while its transfer is still moving.
    t.expires = std::time::Instant::now() + consent::TICKET_TTL;
    Ok(t.clone())
}

/// This machine's display name, for error messages.
fn my_name(ctx: &Ctx) -> String {
    ctx.lan
        .lock()
        .identity
        .as_ref()
        .map(|i| i.device_name.clone())
        .unwrap_or_else(|| "the other machine".into())
}

/// `GET /api/sync/state` — per-project sync fingerprints (skip unchanged trees).
async fn sync_state(State(ctx): State<Ctx>, headers: HeaderMap) -> Response {
    if let Err(r) = authorize(&ctx, &headers) {
        return r;
    }
    let host = ctx.host.clone();
    let res = tokio::task::spawn_blocking(move || {
        let dir = host.data_dir()?;
        Ok::<_, String>(crate::cloud_sync::compute_peer_state(host.state(), &dir))
    })
    .await;
    match res {
        Ok(Ok(projects)) => Json(json!({ "projects": projects })).into_response(),
        Ok(Err(e)) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// Apply an incoming archive stream from a paired device.
fn apply_incoming(
    host: &HostRef,
    source: Box<dyn std::io::Read + Send>,
    scoped: bool,
) -> Result<crate::cloud_sync::SyncManifest, String> {
    use crate::cloud_sync::{apply_project_archive_from, apply_sync_archive_from, SyncProjectEntry};
    let dir = host.data_dir()?;
    let emitter = host.emitter();
    let reporter = crate::cloud_sync::SyncReporter::new("pull", emitter.clone());
    let h = host.clone();
    let resolve = move |e: &SyncProjectEntry, old: Option<&str>| h.incoming_root(e, old);
    if scoped {
        apply_project_archive_from(host.state(), &dir, source, emitter, &resolve, &reporter)
    } else {
        apply_sync_archive_from(
            host.state(),
            &dir,
            host.secrets(),
            source,
            emitter,
            &resolve,
            &reporter,
        )
    }
}

/// Build an outgoing archive for a paired device into `sink`.
fn build_outgoing(
    host: &HostRef,
    sink: Box<dyn std::io::Write + Send>,
    projects: &[crate::cloud_sync::PeerProjectState],
    project_id: Option<&str>,
) -> Result<crate::cloud_sync::SyncManifest, String> {
    let dir = host.data_dir()?;
    let reporter = crate::cloud_sync::SyncReporter::new("push", host.emitter());
    if let Some(pid) = project_id {
        return crate::cloud_sync::build_project_archive_into(host.state(), pid, sink, &reporter);
    }
    let skips = crate::cloud_sync::decide_skips(host.state(), &dir, projects);
    crate::cloud_sync::build_sync_archive_into(
        host.state(),
        &dir,
        host.secrets(),
        sink,
        &skips,
        &reporter,
    )
}

/// Chunked-transfer host for the peer routes (paired-device token auth).
struct LanTransferHost {
    ctx: Ctx,
    sessions: Arc<crate::transfer::session::SessionManager>,
}

impl crate::transfer::routes::TransferHost for LanTransferHost {
    fn sessions(&self) -> Arc<crate::transfer::session::SessionManager> {
        Arc::clone(&self.sessions)
    }

    fn authorize(&self, headers: &HeaderMap) -> Result<(), String> {
        authorize(&self.ctx, headers)
            .map(|_| ())
            .map_err(|_| "not paired — pair again".to_string())
    }

    fn apply_for(
        &self,
        kind: &str,
        headers: &HeaderMap,
    ) -> Result<crate::transfer::session::ApplyFn, String> {
        let scoped = match kind {
            "project" => true,
            "full" => {
                return Err("full-environment sync isn't available between desktops — pick projects".to_string())
            }
            other => return Err(format!("unknown archive kind: {other}")),
        };
        let peer = authorize(&self.ctx, headers).map_err(|_| "not paired".to_string())?;
        let t = ticket(&self.ctx, headers, &peer, "push").map_err(|_| "push not approved".to_string())?;
        let project = headers
            .get(super::PROJECT_HEADER)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if !t.projects.contains(project) {
            return Err("this project wasn't in the approved push".to_string());
        }
        let ctx = self.ctx.clone();
        Ok(Box::new(move |source| {
            let m = apply_incoming(&ctx.host, source, scoped)?;
            ctx.emit(
                "lan-sync-received",
                json!({ "from": peer.name, "projects": m.projects.len(), "scoped": m.project_scoped }),
            );
            Ok(json!({ "ok": true, "projects": m.projects.len() }))
        }))
    }

    fn build_for(
        &self,
        body: &serde_json::Value,
        headers: &HeaderMap,
    ) -> Result<crate::transfer::session::BuildFn, String> {
        let PullBody {
            projects,
            project_id,
        } = serde_json::from_value(body.clone()).map_err(|e| format!("bad pull request: {e}"))?;
        let peer = authorize(&self.ctx, headers).map_err(|_| "not paired".to_string())?;
        let t = ticket(&self.ctx, headers, &peer, "pull").map_err(|_| "pull not approved".to_string())?;
        pull_allowed(&peer, &t, project_id.as_deref())?;
        let ctx = self.ctx.clone();
        Ok(Box::new(move |sink| {
            let m = build_outgoing(&ctx.host, sink, &projects, project_id.as_deref())?;
            ctx.emit("lan-sync-sent", json!({ "to": peer.name }));
            Ok(json!({ "projects": m.projects.len() }))
        }))
    }
}

/// A pull may only take one project at a time that is both approved and shared.
fn pull_allowed(peer: &Peer, t: &consent::Ticket, project_id: Option<&str>) -> Result<(), String> {
    let Some(pid) = project_id else {
        return Err("full-environment sync isn't available between desktops — pick projects".into());
    };
    if !t.projects.contains(pid) {
        return Err("this project wasn't in the approved pull".into());
    }
    if !peer.share.has_project(pid) {
        return Err("this project is no longer shared with you".into());
    }
    Ok(())
}

/// `POST /api/sync/push` — a paired device sends a single-project archive;
/// apply it here and tell the user.
async fn sync_push(State(ctx): State<Ctx>, headers: HeaderMap, body: Body) -> Response {
    let peer = match authorize(&ctx, &headers) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let t = match ticket(&ctx, &headers, &peer, "push") {
        Ok(t) => t,
        Err(r) => return r,
    };
    let project = headers
        .get(super::PROJECT_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !t.projects.contains(project) {
        return err(StatusCode::FORBIDDEN, "this project wasn't in the approved push");
    }
    let dir = match data_dir(&ctx) {
        Ok(d) => d,
        Err(r) => return r,
    };
    let tmp = dir.join("lan-sync-upload.tar.zst");
    {
        use futures_util::StreamExt;
        use tokio::io::AsyncWriteExt;
        let _ = tokio::fs::remove_file(&tmp).await;
        let mut file = match tokio::fs::File::create(&tmp).await {
            Ok(f) => f,
            Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        };
        let mut stream = body.into_data_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = match chunk {
                Ok(c) => c,
                Err(e) => return err(StatusCode::BAD_REQUEST, format!("upload stream error: {e}")),
            };
            if let Err(e) = file.write_all(&chunk).await {
                return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
            }
        }
        if let Err(e) = file.flush().await {
            return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
        }
    }

    let host = ctx.host.clone();
    let tmp_apply = tmp.clone();
    let result = tokio::task::spawn_blocking(move || {
        let scoped = crate::cloud_sync::read_archive_manifest(&tmp_apply)
            .map(|m| m.project_scoped)
            .unwrap_or(false);
        if !scoped {
            return Err("full-environment sync isn't available between desktops — pick projects".to_string());
        }
        let file = std::fs::File::open(&tmp_apply).map_err(|e| e.to_string())?;
        apply_incoming(&host, Box::new(file), scoped)
    })
    .await;
    let _ = tokio::fs::remove_file(&tmp).await;
    match result {
        Ok(Ok(manifest)) => {
            ctx.emit(
                "lan-sync-received",
                json!({ "from": peer.name, "projects": manifest.projects.len(), "scoped": manifest.project_scoped }),
            );
            Json(json!({ "ok": true, "projects": manifest.projects.len() })).into_response()
        }
        Ok(Err(e)) => err(StatusCode::BAD_REQUEST, e),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct PullBody {
    #[serde(default)]
    projects: Vec<crate::cloud_sync::PeerProjectState>,
    #[serde(default)]
    project_id: Option<String>,
}

/// `POST /api/sync/pull` — build a single-project archive and stream it back.
async fn sync_pull(
    State(ctx): State<Ctx>,
    headers: HeaderMap,
    body: Option<Json<PullBody>>,
) -> Response {
    let peer = match authorize(&ctx, &headers) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let PullBody {
        projects,
        project_id,
    } = body.map(|Json(b)| b).unwrap_or_default();
    let t = match ticket(&ctx, &headers, &peer, "pull") {
        Ok(t) => t,
        Err(r) => return r,
    };
    if let Err(e) = pull_allowed(&peer, &t, project_id.as_deref()) {
        return err(StatusCode::FORBIDDEN, e);
    }
    let dir = match data_dir(&ctx) {
        Ok(d) => d,
        Err(r) => return r,
    };
    let tmp = dir.join("lan-sync-download.tar.zst");
    let host = ctx.host.clone();
    let tmp_build = tmp.clone();
    let built = tokio::task::spawn_blocking(move || {
        let _ = std::fs::remove_file(&tmp_build);
        let file = std::fs::File::create(&tmp_build).map_err(|e| e.to_string())?;
        build_outgoing(&host, Box::new(file), &projects, project_id.as_deref())
    })
    .await;
    match built {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => return err(StatusCode::BAD_REQUEST, e),
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
    let file = match tokio::fs::File::open(&tmp).await {
        Ok(f) => f,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let len = tokio::fs::metadata(&tmp)
        .await
        .map(|m| m.len())
        .unwrap_or(0);
    ctx.emit("lan-sync-sent", json!({ "to": peer.name }));
    let stream = tokio_util::io::ReaderStream::with_capacity(file, 256 * 1024);
    let mut resp = Body::from_stream(stream).into_response();
    let headers = resp.headers_mut();
    headers.insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/zstd"),
    );
    if let Ok(v) = len.to_string().parse() {
        headers.insert(axum::http::header::CONTENT_LENGTH, v);
    }
    resp
}

/// `POST /api/list_projects` — the projects this device shares with the caller.
async fn list_projects(State(ctx): State<Ctx>, headers: HeaderMap) -> Response {
    let peer = match authorize(&ctx, &headers) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let rows: Vec<serde_json::Value> = ctx
        .host
        .state()
        .workspace
        .lock()
        .map(|ws| {
            ws.list_projects()
                .into_iter()
                .filter(|p| peer.share.has_project(&p.id.to_string()))
                .map(|p| json!({ "id": p.id.to_string(), "name": p.name, "root_path": p.root_path.to_string_lossy() }))
                .collect()
        })
        .unwrap_or_default();
    Json(rows).into_response()
}

/// `GET /api/sync/meta` — the metadata this device shares with the caller.
/// Without a pull ticket only content hashes are returned (enough to show
/// what's new / different); with one, the full items the ticket covers.
async fn meta_export(State(ctx): State<Ctx>, headers: HeaderMap) -> Response {
    let peer = match authorize(&ctx, &headers) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let approved = if headers.contains_key(super::TICKET_HEADER) {
        match ticket(&ctx, &headers, &peer, "pull") {
            Ok(t) => Some(t.meta),
            Err(r) => return r,
        }
    } else {
        None
    };
    let full_view = peer.meta_view && headers.contains_key("x-rustic-meta-view");
    let host = ctx.host.clone();
    let res = tokio::task::spawn_blocking(move || {
        let dir = host.data_dir()?;
        let mut bundle = crate::meta_sync::export_bundle(host.state(), &dir, host.secrets());
        if !peer.meta_view {
            bundle.items.retain(|i| peer.share.has_meta(&i.key()));
        }
        match approved {
            Some(keys) => bundle.items.retain(|i| keys.contains(&i.key())),
            None if full_view => {}
            None => bundle = consent::summarize(&bundle),
        }
        Ok::<_, String>(bundle)
    })
    .await;
    match res {
        Ok(Ok(b)) => {
            let mut r = Json(b).into_response();
            r.headers_mut().insert(
                "x-rustic-meta-granted",
                axum::http::HeaderValue::from_static(if full_view { "1" } else { "0" }),
            );
            r
        }
        Ok(Err(e)) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Deserialize)]
struct MetaImportBody {
    bundle: crate::meta_sync::MetaBundle,
    #[serde(default)]
    overwrite: Vec<String>,
}

/// `POST /api/sync/meta` — merge a paired device's approved metadata items.
async fn meta_import(
    State(ctx): State<Ctx>,
    headers: HeaderMap,
    Json(body): Json<MetaImportBody>,
) -> Response {
    let peer = match authorize(&ctx, &headers) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let allowed = match ticket(&ctx, &headers, &peer, "push") {
        Ok(t) => t.meta,
        Err(r) => return r,
    };
    let host = ctx.host.clone();
    let res = tokio::task::spawn_blocking(move || {
        let dir = host.data_dir()?;
        let mut bundle = body.bundle;
        bundle.items.retain(|i| allowed.contains(&i.key()));
        let set: std::collections::HashSet<String> = body.overwrite.into_iter().collect();
        Ok::<_, String>(crate::meta_sync::apply_bundle(
            host.state(),
            &dir,
            host.secrets(),
            &bundle,
            &set,
        ))
    })
    .await;
    match res {
        Ok(Ok(summary)) => {
            ctx.emit(
                "lan-sync-received",
                json!({ "from": peer.name, "metadata": true }),
            );
            Json(summary).into_response()
        }
        Ok(Err(e)) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Deserialize)]
struct FsPathBody {
    project_id: String,
    #[serde(default)]
    path: String,
}

/// The local root of `project_id` if it's shared with `peer`.
#[allow(clippy::result_large_err)]
fn shared_root(ctx: &Ctx, peer: &Peer, project_id: &str) -> Result<PathBuf, Response> {
    if !peer.share.has_project(project_id) {
        return Err(err(StatusCode::FORBIDDEN, "that project isn't shared with you"));
    }
    super::files::project_root(ctx.host.state(), project_id).map_err(|e| err(StatusCode::NOT_FOUND, e))
}

/// `POST /api/fs/list` — one folder of a shared project (no approval needed).
async fn fs_list(State(ctx): State<Ctx>, headers: HeaderMap, Json(b): Json<FsPathBody>) -> Response {
    let peer = match authorize(&ctx, &headers) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let root = match shared_root(&ctx, &peer, &b.project_id) {
        Ok(r) => r,
        Err(r) => return r,
    };
    match tokio::task::spawn_blocking(move || super::files::list_dir(&root, &b.path)).await {
        Ok(Ok(entries)) => Json(entries).into_response(),
        Ok(Err(e)) => err(StatusCode::BAD_REQUEST, e),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// `POST /api/fs/preview` — one file of a shared project, loaded on demand.
async fn fs_preview(State(ctx): State<Ctx>, headers: HeaderMap, Json(b): Json<FsPathBody>) -> Response {
    let peer = match authorize(&ctx, &headers) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let root = match shared_root(&ctx, &peer, &b.project_id) {
        Ok(r) => r,
        Err(r) => return r,
    };
    match tokio::task::spawn_blocking(move || super::files::read_preview(&root, &b.path)).await {
        Ok(Ok(p)) => Json(p).into_response(),
        Ok(Err(e)) => err(StatusCode::BAD_REQUEST, e),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Deserialize)]
struct FsItemsBody {
    items: Vec<consent::RequestedFile>,
}

/// Absolute paths + archive names for `items`, all inside shared projects.
#[allow(clippy::result_large_err)]
fn resolve_items(ctx: &Ctx, peer: &Peer, items: &[consent::RequestedFile]) -> Result<Vec<super::files::PackItem>, Response> {
    let mut out = Vec::new();
    for it in items {
        let root = shared_root(ctx, peer, &it.project_id)?;
        let rel = super::files::safe_rel(&it.path).map_err(|e| err(StatusCode::BAD_REQUEST, e))?;
        out.push(super::files::PackItem {
            abs: root.join(rel),
            name: super::files::top_name(&it.project_name, &it.path),
        });
    }
    Ok(out)
}

/// `POST /api/fs/size` — total bytes / files of a selection (for the ETA).
async fn fs_size(State(ctx): State<Ctx>, headers: HeaderMap, Json(b): Json<FsItemsBody>) -> Response {
    let peer = match authorize(&ctx, &headers) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let items = match resolve_items(&ctx, &peer, &b.items) {
        Ok(i) => i,
        Err(r) => return r,
    };
    let paths: Vec<PathBuf> = items.into_iter().map(|i| i.abs).collect();
    match tokio::task::spawn_blocking(move || super::files::measure(&paths)).await {
        Ok(s) => Json(s).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Deserialize)]
struct FsExistsBody {
    project_id: String,
    #[serde(default)]
    dir: String,
    names: Vec<String>,
}

/// `POST /api/fs/exists` — which upload names already exist in a folder.
async fn fs_exists(State(ctx): State<Ctx>, headers: HeaderMap, Json(b): Json<FsExistsBody>) -> Response {
    let peer = match authorize(&ctx, &headers) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let root = match shared_root(&ctx, &peer, &b.project_id) {
        Ok(r) => r,
        Err(r) => return r,
    };
    let dir = match super::files::safe_rel(&b.dir) {
        Ok(d) => root.join(d),
        Err(e) => return err(StatusCode::BAD_REQUEST, e),
    };
    Json(json!({ "existing": super::files::conflicts(&dir, &b.names) })).into_response()
}

/// `POST /api/fs/pull` — stream the approved selection as a zstd tar.
async fn fs_pull(State(ctx): State<Ctx>, headers: HeaderMap, Json(b): Json<FsItemsBody>) -> Response {
    let peer = match authorize(&ctx, &headers) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let t = match ticket(&ctx, &headers, &peer, "pull") {
        Ok(t) => t,
        Err(r) => return r,
    };
    if let Some(f) = b.items.iter().find(|f| !t.covers_file(&f.project_id, &f.path)) {
        return err(StatusCode::FORBIDDEN, format!("{} wasn't in the approved pull", f.path));
    }
    let items = match resolve_items(&ctx, &peer, &b.items) {
        Ok(i) => i,
        Err(r) => return r,
    };
    let label = label_for(&b.items);
    tracing::info!(device = %peer.device_id, items = b.items.len(), "peer: sending files");
    let (writer, rx) = super::files::ChannelWriter::new();
    let host = ctx.host.clone();
    let peer_name = peer.display_name();
    tokio::task::spawn_blocking(move || {
        let size = super::files::measure(&items.iter().map(|i| i.abs.clone()).collect::<Vec<_>>());
        let handle = crate::transfers::begin(host.emitter(), "push", &label, &peer_name, size.bytes, size.files);
        let mut writer = writer;
        let h = handle.clone();
        let res = super::files::pack(&items, &mut writer, &|| h.is_cancelled(), &|n| h.progress("uploading", n, 0));
        if let Err(e) = &res {
            writer.fail(e);
        }
        handle.finish(res.map(|_| None));
    });
    let mut resp = Body::from_stream(super::files::channel_stream(rx)).into_response();
    resp.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/zstd"),
    );
    resp
}

/// Short tray label for a selection.
fn label_for(items: &[consent::RequestedFile]) -> String {
    match items {
        [] => "files".into(),
        [one] => super::files::top_name(&one.project_name, &one.path),
        [first, rest @ ..] => format!("{} + {} more", super::files::top_name(&first.project_name, &first.path), rest.len()),
    }
}

/// Upload destination + conflict handling, JSON in the `x-rustic-dest` header.
#[derive(Deserialize)]
struct PushDest {
    project_id: String,
    #[serde(default)]
    dir: String,
    #[serde(default)]
    policy: super::files::ConflictPolicy,
    #[serde(default)]
    renames: std::collections::HashMap<String, String>,
    #[serde(default)]
    total_bytes: u64,
    #[serde(default)]
    label: String,
}

/// Header carrying [`PushDest`].
pub const DEST_HEADER: &str = "x-rustic-dest";

/// `POST /api/fs/push` — receive an approved upload (zstd tar body) into a
/// folder of one of our projects.
async fn fs_push(State(ctx): State<Ctx>, headers: HeaderMap, body: Body) -> Response {
    let peer = match authorize(&ctx, &headers) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let t = match ticket(&ctx, &headers, &peer, "push") {
        Ok(t) => t,
        Err(r) => return r,
    };
    let dest: PushDest = match headers
        .get(DEST_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| serde_json::from_str(v).ok())
    {
        Some(d) => d,
        None => return err(StatusCode::BAD_REQUEST, "missing upload destination"),
    };
    if !t.covers_file(&dest.project_id, &dest.dir) {
        return err(StatusCode::FORBIDDEN, "this folder wasn't in the approved upload");
    }
    let root = match super::files::project_root(ctx.host.state(), &dest.project_id) {
        Ok(r) => r,
        Err(e) => return err(StatusCode::NOT_FOUND, e),
    };
    let dir = match super::files::safe_rel(&dest.dir) {
        Ok(d) => root.join(d),
        Err(e) => return err(StatusCode::BAD_REQUEST, e),
    };
    tracing::info!(device = %peer.device_id, dir = %dir.display(), "peer: receiving files");
    let (reader, tx) = super::files::ChannelReader::new();
    let handle = crate::transfers::begin(
        ctx.host.emitter(),
        "pull",
        if dest.label.is_empty() { "files" } else { &dest.label },
        &peer.display_name(),
        dest.total_bytes,
        0,
    );
    let h = handle.clone();
    let dir_c = dir.clone();
    let unpacker = tokio::task::spawn_blocking(move || {
        super::files::unpack(reader, &dir_c, dest.policy, &dest.renames, &|| h.is_cancelled(), &|n| h.progress("downloading", n, 0))
    });
    {
        use futures_util::StreamExt;
        let mut stream = body.into_data_stream();
        while let Some(chunk) = stream.next().await {
            let item = chunk.map_err(|e| e.to_string());
            let failed = item.is_err();
            if tx.send(item).await.is_err() || failed {
                break;
            }
        }
        drop(tx);
    }
    let res = unpacker.await.map_err(|e| e.to_string()).and_then(|r| r);
    handle.finish(res.as_ref().map(|_| Some(dir.to_string_lossy().into_owned())).map_err(|e| e.clone()));
    match res {
        Ok(summary) => {
            ctx.emit(
                "lan-sync-received",
                json!({ "from": peer.display_name(), "files": summary.files, "dir": dir.to_string_lossy(), "scoped": true }),
            );
            Json(summary).into_response()
        }
        Err(e) => err(StatusCode::BAD_REQUEST, e),
    }
}

/// `POST /api/fs/manifest` — every file/folder of a selection in sending order
/// (resumable pulls fetch them one by one). Share-gated, no approval needed.
async fn fs_manifest(State(ctx): State<Ctx>, headers: HeaderMap, Json(b): Json<FsItemsBody>) -> Response {
    let peer = match authorize(&ctx, &headers) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let mut specs = Vec::new();
    for it in &b.items {
        let root = match shared_root(&ctx, &peer, &it.project_id) {
            Ok(r) => r,
            Err(r) => return r,
        };
        let rel = match super::files::safe_rel(&it.path) {
            Ok(r) => r,
            Err(e) => return err(StatusCode::BAD_REQUEST, e),
        };
        specs.push((it.project_id.clone(), it.path.trim_matches('/').to_string(), root.join(rel), super::files::top_name(&it.project_name, &it.path)));
    }
    match tokio::task::spawn_blocking(move || super::files::manifest(&specs)).await {
        Ok(entries) => {
            let bytes: u64 = entries.iter().map(|e| e.size).sum();
            let files = entries.iter().filter(|e| !e.is_dir).count();
            Json(json!({ "entries": entries, "bytes": bytes, "files": files })).into_response()
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Deserialize)]
struct FsReadBody {
    project_id: String,
    path: String,
    #[serde(default)]
    offset: u64,
}

/// `POST /api/fs/read` — one approved file's bytes from `offset` (resume).
async fn fs_read(State(ctx): State<Ctx>, headers: HeaderMap, Json(b): Json<FsReadBody>) -> Response {
    let peer = match authorize(&ctx, &headers) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let t = match ticket(&ctx, &headers, &peer, "pull") {
        Ok(t) => t,
        Err(r) => return r,
    };
    if !t.covers_file(&b.project_id, &b.path) {
        return err(StatusCode::FORBIDDEN, format!("{} wasn't in the approved pull", b.path));
    }
    let root = match shared_root(&ctx, &peer, &b.project_id) {
        Ok(r) => r,
        Err(r) => return r,
    };
    let path = match super::files::safe_rel(&b.path) {
        Ok(r) => root.join(r),
        Err(e) => return err(StatusCode::BAD_REQUEST, e),
    };
    let mut file = match tokio::fs::File::open(&path).await {
        Ok(f) => f,
        Err(e) => return err(StatusCode::NOT_FOUND, format!("{}: {e}", b.path)),
    };
    let len = file.metadata().await.map(|m| m.len()).unwrap_or(0);
    if b.offset > len {
        return err(StatusCode::RANGE_NOT_SATISFIABLE, "offset past the end of the file");
    }
    {
        use tokio::io::AsyncSeekExt;
        if let Err(e) = file.seek(std::io::SeekFrom::Start(b.offset)).await {
            return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
        }
    }
    let stream = tokio_util::io::ReaderStream::with_capacity(file, 256 * 1024);
    let mut resp = Body::from_stream(stream).into_response();
    let h = resp.headers_mut();
    h.insert(axum::http::header::CONTENT_TYPE, axum::http::HeaderValue::from_static("application/octet-stream"));
    if let Ok(v) = (len - b.offset).to_string().parse() {
        h.insert(axum::http::header::CONTENT_LENGTH, v);
    }
    resp
}

#[derive(Deserialize)]
struct UploadBeginBody {
    project_id: String,
    #[serde(default)]
    dir: String,
    #[serde(default)]
    policy: super::files::ConflictPolicy,
    #[serde(default)]
    renames: std::collections::HashMap<String, String>,
    entries: Vec<super::files::ManifestEntry>,
    #[serde(default)]
    label: String,
}

/// Seconds without a chunk before the receiver shows the upload as paused.
const UPLOAD_STALL_SECS: u64 = 10;
/// Abandoned upload sessions are dropped after this long without a chunk.
const UPLOAD_EXPIRE_SECS: u64 = 2 * 60 * 60;

/// `POST /api/fs/upload/begin` — open a resumable upload into a folder of one
/// of our projects (approved push). Name clashes are planned once here.
async fn upload_begin(State(ctx): State<Ctx>, headers: HeaderMap, Json(b): Json<UploadBeginBody>) -> Response {
    let peer = match authorize(&ctx, &headers) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let t = match ticket(&ctx, &headers, &peer, "push") {
        Ok(t) => t,
        Err(r) => return r,
    };
    if !t.covers_file(&b.project_id, &b.dir) {
        return err(StatusCode::FORBIDDEN, "this folder wasn't in the approved upload");
    }
    let root = match super::files::project_root(ctx.host.state(), &b.project_id) {
        Ok(r) => r,
        Err(e) => return err(StatusCode::NOT_FOUND, e),
    };
    let dest = match super::files::safe_rel(&b.dir) {
        Ok(d) => root.join(d),
        Err(e) => return err(StatusCode::BAD_REQUEST, e),
    };
    let mut tops: Vec<String> = Vec::new();
    for e in &b.entries {
        if super::files::safe_rel(&e.rel).is_err() {
            return err(StatusCode::BAD_REQUEST, format!("invalid path: {}", e.rel));
        }
        let top = e.rel.split('/').next().unwrap_or_default().to_string();
        if !tops.contains(&top) {
            tops.push(top);
        }
    }
    let (plan, summary) = match super::files::plan_tops(&dest, &tops, b.policy, &b.renames) {
        Ok(p) => p,
        Err(e) => return err(StatusCode::BAD_REQUEST, e),
    };
    for e in b.entries.iter().filter(|e| e.is_dir) {
        if let Ok(Some(p)) = super::files::planned_target(&dest, &plan, &e.rel) {
            let _ = std::fs::create_dir_all(p);
        }
    }
    let total: u64 = b.entries.iter().map(|e| e.size).sum();
    let files = b.entries.iter().filter(|e| !e.is_dir).count() as u64;
    let label = if b.label.is_empty() { "files".to_string() } else { b.label.clone() };
    let handle = crate::transfers::begin(ctx.host.emitter(), "pull", &label, &peer.display_name(), total, files);
    let id = super::random_token().map(|t| t[..24].to_string()).unwrap_or_default();
    let session = Arc::new(UploadSession {
        device_id: peer.device_id.clone(),
        peer_name: peer.display_name(),
        dest: dest.clone(),
        plan,
        sizes: b.entries.iter().map(|e| (e.rel.clone(), (e.is_dir, e.size))).collect(),
        summary: std::sync::Mutex::new(summary),
        received: std::sync::atomic::AtomicU64::new(0),
        last_chunk: std::sync::Mutex::new(std::time::Instant::now()),
        handle,
        done: std::sync::atomic::AtomicBool::new(false),
    });
    ctx.uploads.lock().unwrap_or_else(|p| p.into_inner()).insert(id.clone(), Arc::clone(&session));
    tracing::info!(device = %peer.device_id, dest = %dest.display(), files, total, "peer: upload session opened");
    // Show stalls on this side's tray and drop abandoned sessions.
    let (uploads, sid, s) = (ctx.uploads.clone(), id.clone(), Arc::clone(&session));
    tokio::spawn(async move {
        let mut paused = false;
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            if s.done.load(std::sync::atomic::Ordering::SeqCst) {
                return;
            }
            let idle = s.last_chunk.lock().unwrap_or_else(|p| p.into_inner()).elapsed().as_secs();
            if idle >= UPLOAD_EXPIRE_SECS {
                s.handle.finish(Err(format!("{} didn't reconnect — upload abandoned", s.peer_name)));
                uploads.lock().unwrap_or_else(|p| p.into_inner()).remove(&sid);
                return;
            }
            if idle >= UPLOAD_STALL_SECS && !paused {
                s.handle.net_pause(&format!("Connection lost — waiting for {}", s.peer_name));
                paused = true;
            } else if idle < UPLOAD_STALL_SECS && paused {
                s.handle.net_resume();
                paused = false;
            }
        }
    });
    Json(json!({ "id": id, "renamed": session.summary.lock().unwrap_or_else(|p| p.into_inner()).renamed })).into_response()
}

/// The open upload session `id` belonging to `peer`.
#[allow(clippy::result_large_err)]
fn upload_session(ctx: &Ctx, peer: &Peer, id: &str) -> Result<Arc<UploadSession>, Response> {
    ctx.uploads
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(id)
        .filter(|s| s.device_id == peer.device_id)
        .cloned()
        .ok_or_else(|| err(StatusCode::GONE, "upload session expired — start the upload again"))
}

#[derive(Deserialize)]
struct UploadFileBody {
    id: String,
    rel: String,
}

/// Local target of `rel` in `s` (`None` = skipped by the conflict plan).
#[allow(clippy::result_large_err)]
fn upload_target(s: &UploadSession, rel: &str) -> Result<Option<(PathBuf, u64)>, Response> {
    let (_, size) = *s.sizes.get(rel).ok_or_else(|| err(StatusCode::BAD_REQUEST, format!("{rel} isn't part of this upload")))?;
    super::files::planned_target(&s.dest, &s.plan, rel)
        .map(|t| t.map(|t| (t, size)))
        .map_err(|e| err(StatusCode::BAD_REQUEST, e))
}

/// `POST /api/fs/upload/offset` — bytes already received for one file.
async fn upload_offset(State(ctx): State<Ctx>, headers: HeaderMap, Json(b): Json<UploadFileBody>) -> Response {
    let peer = match authorize(&ctx, &headers) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let s = match upload_session(&ctx, &peer, &b.id) {
        Ok(s) => s,
        Err(r) => return r,
    };
    match upload_target(&s, &b.rel) {
        Ok(Some((target, size))) => Json(json!({ "offset": super::files::received_len(&target, size), "skip": false })).into_response(),
        Ok(None) => Json(json!({ "offset": 0, "skip": true })).into_response(),
        Err(r) => r,
    }
}

/// Header carrying `{id, rel, offset}` for an upload chunk.
pub const CHUNK_HEADER: &str = "x-rustic-chunk";

#[derive(Deserialize)]
struct ChunkMeta {
    id: String,
    rel: String,
    offset: u64,
}

/// `POST /api/fs/upload/chunk` — bytes of one file starting at `offset`
/// (streamed; whatever arrives before a disconnect is kept for resuming).
async fn upload_chunk(State(ctx): State<Ctx>, headers: HeaderMap, body: Body) -> Response {
    let peer = match authorize(&ctx, &headers) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let Some(meta) = headers
        .get(CHUNK_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| serde_json::from_str::<ChunkMeta>(v).ok())
    else {
        return err(StatusCode::BAD_REQUEST, "missing chunk header");
    };
    let s = match upload_session(&ctx, &peer, &meta.id) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let (target, size) = match upload_target(&s, &meta.rel) {
        Ok(Some(t)) => t,
        Ok(None) => return Json(json!({ "received": 0, "skipped": true })).into_response(),
        Err(r) => return r,
    };
    let mut file = match super::files::open_part_at(&target, meta.offset) {
        Ok(f) => f,
        Err(e) => return err(StatusCode::CONFLICT, e),
    };
    *s.last_chunk.lock().unwrap_or_else(|p| p.into_inner()) = std::time::Instant::now();
    s.handle.net_resume();
    let mut written = meta.offset;
    {
        use futures_util::StreamExt;
        use std::io::Write;
        let mut stream = body.into_data_stream();
        while let Some(chunk) = stream.next().await {
            let Ok(chunk) = chunk else { break };
            if let Err(e) = file.write_all(&chunk) {
                return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
            }
            written += chunk.len() as u64;
            let total = s.received.fetch_add(chunk.len() as u64, std::sync::atomic::Ordering::SeqCst) + chunk.len() as u64;
            s.handle.progress("downloading", total, 0);
            *s.last_chunk.lock().unwrap_or_else(|p| p.into_inner()) = std::time::Instant::now();
        }
        let _ = file.flush();
    }
    drop(file);
    if written >= size {
        if let Err(e) = super::files::finish_part(&target) {
            return err(StatusCode::INTERNAL_SERVER_ERROR, e);
        }
        let mut sum = s.summary.lock().unwrap_or_else(|p| p.into_inner());
        sum.files += 1;
        sum.bytes += size;
    }
    Json(json!({ "received": written })).into_response()
}

#[derive(Deserialize)]
struct UploadIdBody {
    id: String,
}

/// `POST /api/fs/upload/finish` — close the session and report what landed.
async fn upload_finish(State(ctx): State<Ctx>, headers: HeaderMap, Json(b): Json<UploadIdBody>) -> Response {
    let peer = match authorize(&ctx, &headers) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let s = match upload_session(&ctx, &peer, &b.id) {
        Ok(s) => s,
        Err(r) => return r,
    };
    ctx.uploads.lock().unwrap_or_else(|p| p.into_inner()).remove(&b.id);
    s.done.store(true, std::sync::atomic::Ordering::SeqCst);
    let summary = s.summary.lock().unwrap_or_else(|p| p.into_inner()).clone();
    s.handle.finish(Ok(Some(s.dest.to_string_lossy().into_owned())));
    tracing::info!(device = %peer.device_id, files = summary.files, "peer: upload finished");
    ctx.emit(
        "lan-sync-received",
        json!({ "from": peer.display_name(), "files": summary.files, "dir": s.dest.to_string_lossy(), "scoped": true }),
    );
    Json(summary).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caller_addr_prefers_url_and_ignores_loopback() {
        let mut h = HeaderMap::new();
        h.insert(super::super::PORT_HEADER, "47820".parse().unwrap());
        let lan: std::net::SocketAddr = "10.0.0.5:5555".parse().unwrap();
        let lo: std::net::SocketAddr = "127.0.0.1:5555".parse().unwrap();
        assert_eq!(caller_addr(&h, Some(Remote(lan))).as_deref(), Some("10.0.0.5:47820"));
        assert_eq!(caller_addr(&h, Some(Remote(lo))), None);
        h.insert(super::super::URL_HEADER, "https://x.trycloudflare.com/".parse().unwrap());
        assert_eq!(caller_addr(&h, Some(Remote(lo))).as_deref(), Some("https://x.trycloudflare.com"));
    }
}

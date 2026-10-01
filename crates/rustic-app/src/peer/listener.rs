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
        .ok_or_else(|| err(StatusCode::UNAUTHORIZED, "unknown device — pair again"))
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
    let ctx = Ctx { host, lan };
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
        .route(
            "/api/sync/meta",
            get(meta_export)
                .post(meta_import)
                .layer(axum::extract::DefaultBodyLimit::max(256 * 1024 * 1024)),
        );
    let ctl: Router<Ctx> = Router::new()
        .route("/info", get(info))
        .route("/pair", post(pair))
        .route("/request", post(transfer_request));
    let mut router: Router<Ctx> = Router::new().nest("/lan", ctl.merge(api.clone()));
    if legacy_root {
        router = router.merge(api);
    }
    Ok(router
        .layer(axum::middleware::from_fn_with_state(ctx.clone(), track_seen))
        .with_state(ctx))
}

/// Start the TLS listener (this install's self-signed certificate, pinned by
/// peers) on [`super::DEFAULT_PORT`], or an OS-assigned port when that one is
/// taken, serving [`build_router`] with legacy root routes. Returns the port.
pub async fn start(host: HostRef, lan: LanState, id: Identity) -> Result<u16, String> {
    let tls = super::server_tls_config(&id)?;
    let acceptor = tokio_rustls::TlsAcceptor::from(tls);
    let listener = match tokio::net::TcpListener::bind(("0.0.0.0", super::DEFAULT_PORT)).await {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!(
                "LAN port {} unavailable ({e}); using a random port",
                super::DEFAULT_PORT
            );
            tokio::net::TcpListener::bind("0.0.0.0:0")
                .await
                .map_err(|e| format!("LAN listener bind failed: {e}"))?
        }
    };
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
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

/// `GET /lan/info` — who this device is.
async fn info(State(ctx): State<Ctx>) -> Response {
    let inner = ctx.lan.lock();
    match inner.identity.as_ref() {
        Some(id) => Json(json!({ "device_id": id.device_id, "name": id.device_name, "fingerprint": id.fingerprint })).into_response(),
        None => err(StatusCode::SERVICE_UNAVAILABLE, "local-network sync is off"),
    }
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
        inner.pending_pairs.insert(request_id.clone(), tx);
        (id, request_id, rx)
    };
    let code = super::pairing_code(&my.fingerprint, &req.fingerprint);
    ctx.emit(
        "lan-pair-request",
        json!({ "request_id": request_id, "device_id": req.device_id, "name": req.name, "code": code }),
    );
    let accepted = matches!(
        tokio::time::timeout(std::time::Duration::from_secs(90), rx).await,
        Ok(Ok(true))
    );
    ctx.lan.lock().pending_pairs.remove(&request_id);
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
    let share = existing.map(|p| p.share).unwrap_or_default();
    ctx.lan.lock().mark_seen(&req.device_id, addr.clone());
    let peer = Peer {
        device_id: req.device_id,
        name: req.name,
        fingerprint: req.fingerprint,
        token_out: req.token,
        token_in: token_in.clone(),
        addr,
        nickname,
        share,
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
        let denied = consent::unshared_items(&req, &peer.share);
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
    let payload = json!({
        "request_id": request_id,
        "device_id": peer.device_id,
        "from": peer.display_name(),
        "kind": req.kind,
        "projects": req.projects,
        "meta": req.meta,
    });
    let approved = if req.kind == "push" && ctx.host.auto_accept_push() {
        ctx.emit("lan-transfer-auto-accepted", payload);
        true
    } else {
        let (tx, rx) = tokio::sync::oneshot::channel::<bool>();
        ctx.lan.lock().pending_transfers.insert(request_id.clone(), tx);
        ctx.emit("lan-transfer-request", payload);
        let ok = matches!(
            tokio::time::timeout(consent::PROMPT_TIMEOUT, rx).await,
            Ok(Ok(true))
        );
        ctx.lan.lock().pending_transfers.remove(&request_id);
        ok
    };
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
    ctx.lan
        .lock()
        .tickets
        .get(id)
        .filter(|t| t.valid_for(&peer.device_id, kind))
        .cloned()
        .ok_or_else(|| err(StatusCode::FORBIDDEN, "approval expired or doesn't cover this — ask again"))
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
    let host = ctx.host.clone();
    let res = tokio::task::spawn_blocking(move || {
        let dir = host.data_dir()?;
        let mut bundle = crate::meta_sync::export_bundle(host.state(), &dir, host.secrets());
        bundle.items.retain(|i| peer.share.has_meta(&i.key()));
        match approved {
            Some(keys) => bundle.items.retain(|i| keys.contains(&i.key())),
            None => bundle = consent::summarize(&bundle),
        }
        Ok::<_, String>(bundle)
    })
    .await;
    match res {
        Ok(Ok(b)) => Json(b).into_response(),
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

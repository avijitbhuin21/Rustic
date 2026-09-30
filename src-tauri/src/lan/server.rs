//! TLS listener for local-network sync. Unauthenticated: `/lan/info`,
//! `/lan/pair`. Token-authenticated (a paired device's `token_in`): the
//! rustic-server-compatible `/api/sync/*`, `/api/list_projects` routes.

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
use tauri::{AppHandle, Emitter, Manager};

use super::{Identity, LanState, Peer};
use crate::state::AppState;
use crate::transport::{KeychainSecretStore, TauriEmitter};

#[derive(Clone)]
struct Ctx {
    app: AppHandle,
    lan: LanState,
}

/// App data dir or an HTTP 500.
#[allow(clippy::result_large_err)] // the Err is the ready-to-send HTTP response
fn data_dir(app: &AppHandle) -> Result<PathBuf, Response> {
    crate::app_paths::app_data_dir(app)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
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
    let dir = data_dir(&ctx.app)?;
    super::load_peers(&dir)
        .into_iter()
        .find(|p| !p.token_in.is_empty() && p.token_in == token)
        .ok_or_else(|| err(StatusCode::UNAUTHORIZED, "unknown device — pair again"))
}

/// Start the TLS listener on an OS-assigned port. Returns the port.
pub async fn start(app: AppHandle, lan: LanState, id: Identity) -> Result<u16, String> {
    let tls = super::server_tls_config(&id)?;
    let acceptor = tokio_rustls::TlsAcceptor::from(tls);
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0")
        .await
        .map_err(|e| format!("LAN listener bind failed: {e}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();

    let ctx = Ctx {
        app,
        lan: lan.clone(),
    };
    let sessions_dir = crate::app_paths::app_data_dir(&ctx.app)
        .map_err(|e| e.to_string())?
        .join("lan-sync-sessions");
    let _ = std::fs::remove_dir_all(&sessions_dir); // leftovers from a previous run
    let host = Arc::new(LanTransferHost {
        ctx: ctx.clone(),
        sessions: rustic_app::transfer::session::SessionManager::new(sessions_dir),
    });
    let router = Router::new()
        .route("/lan/info", get(info))
        .route("/lan/pair", post(pair))
        // Chunked, parallel, resumable sync (streamed pack ‖ send ‖ extract).
        .merge(rustic_app::transfer::routes::router::<LanTransferHost, Ctx>(host))
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
        )
        .with_state(ctx);

    let (tx, mut rx) = tokio::sync::oneshot::channel::<()>();
    {
        let mut inner = lan.lock();
        inner.port = port;
        inner.shutdown = Some(tx);
    }
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut rx => break,
                accepted = listener.accept() => {
                    let Ok((stream, _addr)) = accepted else { continue };
                    let acceptor = acceptor.clone();
                    let router = router.clone();
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
async fn pair(State(ctx): State<Ctx>, Json(req): Json<PairRequest>) -> Response {
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
    let _ = ctx.app.emit(
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
    let dir = match data_dir(&ctx.app) {
        Ok(d) => d,
        Err(r) => return r,
    };
    let addr = ctx
        .lan
        .lock()
        .discovered
        .get(&req.device_id)
        .map(|d| d.addr.clone());
    let peer = Peer {
        device_id: req.device_id,
        name: req.name,
        fingerprint: req.fingerprint,
        token_out: req.token,
        token_in: token_in.clone(),
        addr,
    };
    if let Err(e) = super::upsert_peer(&dir, peer) {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e);
    }
    let _ = ctx.app.emit("lan-peers-changed", json!({}));
    Json(json!({
        "accepted": true,
        "device_id": my.device_id,
        "name": my.device_name,
        "fingerprint": my.fingerprint,
        "token": token_in,
    }))
    .into_response()
}

/// `GET /api/sync/state` — per-project sync fingerprints (skip unchanged trees).
async fn sync_state(State(ctx): State<Ctx>, headers: HeaderMap) -> Response {
    if let Err(r) = authorize(&ctx, &headers) {
        return r;
    }
    let app = ctx.app.clone();
    let res = tokio::task::spawn_blocking(move || {
        let dir = crate::app_paths::app_data_dir(&app).map_err(|e| e.to_string())?;
        let state = app.state::<AppState>();
        Ok::<_, String>(rustic_app::cloud_sync::compute_peer_state(
            state.inner(),
            &dir,
        ))
    })
    .await;
    match res {
        Ok(Ok(projects)) => Json(json!({ "projects": projects })).into_response(),
        Ok(Err(e)) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// Where incoming projects land: the existing local root for that project id,
/// else the sender's path when it's native to this OS, else ~/projects/<name>.
fn incoming_root(
    app: &AppHandle,
    entry: &rustic_app::cloud_sync::SyncProjectEntry,
    old: Option<&str>,
) -> PathBuf {
    if let Some(old) = old {
        return PathBuf::from(old);
    }
    if crate::commands::cloud_sync::path_is_native(&entry.origin_root_path) {
        return PathBuf::from(&entry.origin_root_path);
    }
    let home = app.path().home_dir().unwrap_or_else(|_| PathBuf::from("."));
    home.join("projects")
        .join(rustic_app::cloud_sync::safe_dir_name(&entry.name))
}

/// Apply an incoming archive stream from a paired device.
fn apply_incoming(
    app: &AppHandle,
    source: Box<dyn std::io::Read + Send>,
    scoped: bool,
) -> Result<rustic_app::cloud_sync::SyncManifest, String> {
    use rustic_app::cloud_sync::{
        apply_project_archive_from, apply_sync_archive_from, SyncProjectEntry,
    };
    let state = app.state::<AppState>();
    let dir = crate::app_paths::app_data_dir(app).map_err(|e| e.to_string())?;
    let emitter: Arc<dyn rustic_app::EventEmitter> = Arc::new(TauriEmitter::new(app.clone()));
    let reporter = rustic_app::cloud_sync::SyncReporter::new("pull", emitter.clone());
    let app_r = app.clone();
    let resolve = move |e: &SyncProjectEntry, old: Option<&str>| incoming_root(&app_r, e, old);
    if scoped {
        apply_project_archive_from(state.inner(), &dir, source, emitter, &resolve, &reporter)
    } else {
        apply_sync_archive_from(
            state.inner(),
            &dir,
            &KeychainSecretStore,
            source,
            emitter,
            &resolve,
            &reporter,
        )
    }
}

/// Build an outgoing archive for a paired device into `sink`.
fn build_outgoing(
    app: &AppHandle,
    sink: Box<dyn std::io::Write + Send>,
    projects: &[rustic_app::cloud_sync::PeerProjectState],
    project_id: Option<&str>,
) -> Result<rustic_app::cloud_sync::SyncManifest, String> {
    let state = app.state::<AppState>();
    let dir = crate::app_paths::app_data_dir(app).map_err(|e| e.to_string())?;
    let emitter: Arc<dyn rustic_app::EventEmitter> = Arc::new(TauriEmitter::new(app.clone()));
    let reporter = rustic_app::cloud_sync::SyncReporter::new("push", emitter);
    if let Some(pid) = project_id {
        return rustic_app::cloud_sync::build_project_archive_into(
            state.inner(),
            pid,
            sink,
            &reporter,
        );
    }
    let skips = rustic_app::cloud_sync::decide_skips(state.inner(), &dir, projects);
    rustic_app::cloud_sync::build_sync_archive_into(
        state.inner(),
        &dir,
        &KeychainSecretStore,
        sink,
        &skips,
        &reporter,
    )
}

/// Chunked-transfer host for the LAN listener (paired-device token auth).
struct LanTransferHost {
    ctx: Ctx,
    sessions: Arc<rustic_app::transfer::session::SessionManager>,
}

impl rustic_app::transfer::routes::TransferHost for LanTransferHost {
    fn sessions(&self) -> Arc<rustic_app::transfer::session::SessionManager> {
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
    ) -> Result<rustic_app::transfer::session::ApplyFn, String> {
        let scoped = match kind {
            "project" => true,
            "full" => false,
            other => return Err(format!("unknown archive kind: {other}")),
        };
        let peer = authorize(&self.ctx, headers).map_err(|_| "not paired".to_string())?;
        let app = self.ctx.app.clone();
        Ok(Box::new(move |source| {
            let m = apply_incoming(&app, source, scoped)?;
            let _ = app.emit(
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
    ) -> Result<rustic_app::transfer::session::BuildFn, String> {
        let PullBody {
            projects,
            project_id,
        } = serde_json::from_value(body.clone()).map_err(|e| format!("bad pull request: {e}"))?;
        let peer = authorize(&self.ctx, headers).map_err(|_| "not paired".to_string())?;
        let app = self.ctx.app.clone();
        Ok(Box::new(move |sink| {
            let m = build_outgoing(&app, sink, &projects, project_id.as_deref())?;
            let _ = app.emit("lan-sync-sent", json!({ "to": peer.name }));
            Ok(json!({ "projects": m.projects.len() }))
        }))
    }
}

/// `POST /api/sync/push` — a paired device sends a full or single-project
/// archive; apply it here and tell the user.
async fn sync_push(State(ctx): State<Ctx>, headers: HeaderMap, body: Body) -> Response {
    let peer = match authorize(&ctx, &headers) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let dir = match data_dir(&ctx.app) {
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

    let app = ctx.app.clone();
    let tmp_apply = tmp.clone();
    let result = tokio::task::spawn_blocking(move || {
        let scoped = rustic_app::cloud_sync::read_archive_manifest(&tmp_apply)
            .map(|m| m.project_scoped)
            .unwrap_or(false);
        let file = std::fs::File::open(&tmp_apply).map_err(|e| e.to_string())?;
        apply_incoming(&app, Box::new(file), scoped)
    })
    .await;
    let _ = tokio::fs::remove_file(&tmp).await;
    match result {
        Ok(Ok(manifest)) => {
            let _ = ctx.app.emit(
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
    projects: Vec<rustic_app::cloud_sync::PeerProjectState>,
    #[serde(default)]
    project_id: Option<String>,
}

/// `POST /api/sync/pull` — build a full or single-project archive and stream it back.
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
    let dir = match data_dir(&ctx.app) {
        Ok(d) => d,
        Err(r) => return r,
    };
    let tmp = dir.join("lan-sync-download.tar.zst");
    let app = ctx.app.clone();
    let tmp_build = tmp.clone();
    let built = tokio::task::spawn_blocking(move || {
        let _ = std::fs::remove_file(&tmp_build);
        let file = std::fs::File::create(&tmp_build).map_err(|e| e.to_string())?;
        build_outgoing(&app, Box::new(file), &projects, project_id.as_deref())
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
    let _ = ctx.app.emit("lan-sync-sent", json!({ "to": peer.name }));
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

/// `POST /api/list_projects` — this device's projects (for the Pull picker).
async fn list_projects(State(ctx): State<Ctx>, headers: HeaderMap) -> Response {
    if let Err(r) = authorize(&ctx, &headers) {
        return r;
    }
    let state = ctx.app.state::<AppState>();
    let rows: Vec<serde_json::Value> = state
        .inner()
        .workspace
        .lock()
        .map(|ws| {
            ws.list_projects()
                .into_iter()
                .map(|p| json!({ "id": p.id.to_string(), "name": p.name, "root_path": p.root_path.to_string_lossy() }))
                .collect()
        })
        .unwrap_or_default();
    Json(rows).into_response()
}

/// `GET /api/sync/meta` — this device's metadata bundle.
async fn meta_export(State(ctx): State<Ctx>, headers: HeaderMap) -> Response {
    if let Err(r) = authorize(&ctx, &headers) {
        return r;
    }
    let app = ctx.app.clone();
    let res = tokio::task::spawn_blocking(move || {
        let dir = crate::app_paths::app_data_dir(&app).map_err(|e| e.to_string())?;
        let state = app.state::<AppState>();
        Ok::<_, String>(rustic_app::meta_sync::export_bundle(
            state.inner(),
            &dir,
            &KeychainSecretStore,
        ))
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
    bundle: rustic_app::meta_sync::MetaBundle,
    #[serde(default)]
    overwrite: Vec<String>,
}

/// `POST /api/sync/meta` — merge a paired device's metadata bundle.
async fn meta_import(
    State(ctx): State<Ctx>,
    headers: HeaderMap,
    Json(body): Json<MetaImportBody>,
) -> Response {
    let peer = match authorize(&ctx, &headers) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let app = ctx.app.clone();
    let res = tokio::task::spawn_blocking(move || {
        let dir = crate::app_paths::app_data_dir(&app).map_err(|e| e.to_string())?;
        let state = app.state::<AppState>();
        let set: std::collections::HashSet<String> = body.overwrite.into_iter().collect();
        Ok::<_, String>(rustic_app::meta_sync::apply_bundle(
            state.inner(),
            &dir,
            &KeychainSecretStore,
            &body.bundle,
            &set,
        ))
    })
    .await;
    match res {
        Ok(Ok(summary)) => {
            let _ = ctx.app.emit(
                "lan-sync-received",
                json!({ "from": peer.name, "metadata": true }),
            );
            Json(summary).into_response()
        }
        Ok(Err(e)) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

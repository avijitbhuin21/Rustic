//! HTTP routes for streamed chunk transfers (`/api/sync/v2/*`), shared by
//! rustic-server and the desktop LAN listener. Hosts plug in auth and the
//! archive apply/build functions via [`TransferHost`].

use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use super::session::{ApplyFn, BuildFn, ChunkReply, SessionManager};
use super::CHUNK_SIZE;

/// Header carrying a chunk's SHA-256 (lowercase hex).
pub const CHUNK_SHA_HEADER: &str = "x-chunk-sha256";

/// What a host provides to serve transfers.
pub trait TransferHost: Send + Sync + 'static {
    /// The host's session manager.
    fn sessions(&self) -> Arc<SessionManager>;
    /// Reject unauthenticated callers (hosts behind an auth middleware return Ok).
    fn authorize(&self, _headers: &HeaderMap) -> Result<(), String> {
        Ok(())
    }
    /// Consumer for an incoming archive of `kind` ("full" or "project").
    fn apply_for(&self, kind: &str, headers: &HeaderMap) -> Result<ApplyFn, String>;
    /// Producer for an outgoing archive; `body` is the pull request
    /// (`{ projects, projectId }`, same as the legacy `/api/sync/pull`).
    fn build_for(&self, body: &Value, headers: &HeaderMap) -> Result<BuildFn, String>;
}

/// JSON error response.
fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": msg.into() }))).into_response()
}

/// Routes for any outer router state `S`.
pub fn router<H: TransferHost, S: Clone + Send + Sync + 'static>(host: Arc<H>) -> Router<S> {
    Router::new()
        .route("/api/sync/v2/upload", post(begin_upload::<H>))
        .route("/api/sync/v2/upload/:id", get(upload_status::<H>))
        .route("/api/sync/v2/upload/:id/chunk/:index", put(put_chunk::<H>))
        .route("/api/sync/v2/upload/:id/finish", post(finish_upload::<H>))
        .route("/api/sync/v2/download", post(begin_download::<H>))
        .route(
            "/api/sync/v2/download/:id",
            get(download_status::<H>).delete(finish_download::<H>),
        )
        .route(
            "/api/sync/v2/download/:id/chunk/:index",
            get(download_chunk::<H>),
        )
        .layer(axum::extract::DefaultBodyLimit::max(
            (CHUNK_SIZE * 2) as usize,
        ))
        .with_state(host)
}

#[derive(Deserialize)]
struct BeginUpload {
    kind: String,
}

async fn begin_upload<H: TransferHost>(
    State(host): State<Arc<H>>,
    headers: HeaderMap,
    Json(b): Json<BeginUpload>,
) -> Response {
    if let Err(e) = host.authorize(&headers) {
        return err(StatusCode::UNAUTHORIZED, e);
    }
    let apply = match host.apply_for(&b.kind, &headers) {
        Ok(a) => a,
        Err(e) => return err(StatusCode::BAD_REQUEST, e),
    };
    match host.sessions().begin_upload(apply) {
        Ok(id) => Json(json!({ "id": id, "chunk_size": CHUNK_SIZE })).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

async fn put_chunk<H: TransferHost>(
    State(host): State<Arc<H>>,
    Path((id, index)): Path<(String, u64)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(e) = host.authorize(&headers) {
        return err(StatusCode::UNAUTHORIZED, e);
    }
    let Some(sha) = headers
        .get(CHUNK_SHA_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
    else {
        return err(StatusCode::BAD_REQUEST, "missing chunk checksum");
    };
    let sessions = host.sessions();
    let res =
        tokio::task::spawn_blocking(move || sessions.put_chunk(&id, index, &body, &sha)).await;
    match res {
        Ok(Ok(())) => Json(json!({ "ok": true })).into_response(),
        Ok(Err(e)) => err(StatusCode::UNPROCESSABLE_ENTITY, e),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn upload_status<H: TransferHost>(
    State(host): State<Arc<H>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(e) = host.authorize(&headers) {
        return err(StatusCode::UNAUTHORIZED, e);
    }
    match host.sessions().upload_status(&id) {
        Ok(received) => Json(json!({ "received": received })).into_response(),
        Err(e) => err(StatusCode::NOT_FOUND, e),
    }
}

#[derive(Deserialize)]
struct FinishUpload {
    total_size: u64,
}

async fn finish_upload<H: TransferHost>(
    State(host): State<Arc<H>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(b): Json<FinishUpload>,
) -> Response {
    if let Err(e) = host.authorize(&headers) {
        return err(StatusCode::UNAUTHORIZED, e);
    }
    let sessions = host.sessions();
    let res = tokio::task::spawn_blocking(move || sessions.finish_upload(&id, b.total_size)).await;
    match res {
        Ok(Ok(v)) => Json(v).into_response(),
        Ok(Err(e)) if e.starts_with("missing chunks") => err(StatusCode::CONFLICT, e),
        Ok(Err(e)) => err(StatusCode::BAD_REQUEST, e),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn begin_download<H: TransferHost>(
    State(host): State<Arc<H>>,
    headers: HeaderMap,
    body: Option<Json<Value>>,
) -> Response {
    if let Err(e) = host.authorize(&headers) {
        return err(StatusCode::UNAUTHORIZED, e);
    }
    let body = body.map(|Json(v)| v).unwrap_or_else(|| json!({}));
    let build = match host.build_for(&body, &headers) {
        Ok(b) => b,
        Err(e) => return err(StatusCode::BAD_REQUEST, e),
    };
    match host.sessions().begin_download(build) {
        Ok(id) => Json(json!({ "id": id, "chunk_size": CHUNK_SIZE })).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

async fn download_chunk<H: TransferHost>(
    State(host): State<Arc<H>>,
    Path((id, index)): Path<(String, u64)>,
    headers: HeaderMap,
) -> Response {
    if let Err(e) = host.authorize(&headers) {
        return err(StatusCode::UNAUTHORIZED, e);
    }
    let sessions = host.sessions();
    let res = tokio::task::spawn_blocking(move || {
        sessions.download_chunk(&id, index, Duration::from_secs(20))
    })
    .await;
    match res {
        Ok(Ok(ChunkReply::Data(data, sha))) => {
            let mut resp = data.into_response();
            if let Ok(v) = HeaderValue::from_str(&sha) {
                resp.headers_mut().insert(CHUNK_SHA_HEADER, v);
            }
            resp
        }
        Ok(Ok(ChunkReply::End)) => StatusCode::NO_CONTENT.into_response(),
        Ok(Ok(ChunkReply::NotReady)) => StatusCode::TOO_EARLY.into_response(),
        Ok(Err(e)) => err(StatusCode::BAD_REQUEST, e),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn download_status<H: TransferHost>(
    State(host): State<Arc<H>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(e) = host.authorize(&headers) {
        return err(StatusCode::UNAUTHORIZED, e);
    }
    match host.sessions().download_status(&id) {
        Ok((committed, done, error, result)) => {
            Json(json!({ "committed": committed, "done": done, "error": error, "result": result }))
                .into_response()
        }
        Err(e) => err(StatusCode::NOT_FOUND, e),
    }
}

async fn finish_download<H: TransferHost>(
    State(host): State<Arc<H>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(e) = host.authorize(&headers) {
        return err(StatusCode::UNAUTHORIZED, e);
    }
    host.sessions().finish_download(&id);
    Json(json!({ "ok": true })).into_response()
}

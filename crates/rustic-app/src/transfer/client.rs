//! Parallel, resumable client for `/api/sync/v2/*` (see [`super::routes`]).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use super::routes::CHUNK_SHA_HEADER;
use super::{chunk_count, Assembler, Growing, CHUNK_SIZE};

/// Where and how to talk to the other side.
#[derive(Clone)]
pub struct Endpoint {
    pub client: reqwest::Client,
    /// e.g. `https://host:port` (no trailing slash).
    pub base: String,
    /// Bearer token.
    pub token: String,
    /// Chunks in flight at once.
    pub parallel: usize,
}

/// `(phase, done_bytes, total_bytes_or_0)`.
pub type Progress = Arc<dyn Fn(&str, u64, u64) + Send + Sync>;

/// Why a transfer did not complete.
#[derive(Debug)]
pub enum TransferError {
    /// The other side predates chunked transfers — use the legacy single stream.
    Unsupported,
    Failed(String),
}

impl std::fmt::Display for TransferError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransferError::Unsupported => write!(f, "the other side does not support chunked sync"),
            TransferError::Failed(m) => write!(f, "{m}"),
        }
    }
}

/// Attempts per chunk before giving up (with exponential backoff).
const CHUNK_ATTEMPTS: u32 = 6;

/// Backoff before retry `attempt` (1-based): 0.5s, 1s, 2s, 4s, 8s…
fn backoff(attempt: u32) -> Duration {
    Duration::from_millis(500u64 << (attempt.saturating_sub(1)).min(5))
}

/// Error text from a failed JSON response.
async fn error_text(resp: reqwest::Response) -> String {
    let status = resp.status();
    let body: Value = resp.json().await.unwrap_or_default();
    let msg = body
        .get("error")
        .and_then(|e| e.as_str())
        .unwrap_or("unknown error");
    format!("HTTP {status}: {msg}")
}

/// Wait (without blocking the runtime) until `grow` has `need` bytes or is done.
async fn wait_growing(grow: &Arc<Growing>, need: u64) -> (u64, bool, Option<String>) {
    let g = Arc::clone(grow);
    tokio::task::spawn_blocking(move || g.wait_for(need, Duration::from_secs(2)))
        .await
        .unwrap_or((0, false, Some("wait task failed".into())))
}

/// Send one chunk, retrying transient failures.
async fn put_chunk(ep: &Endpoint, id: &str, index: u64, data: Vec<u8>) -> Result<(), String> {
    let sha = super::sha256_hex(&data);
    let url = format!("{}/api/sync/v2/upload/{id}/chunk/{index}", ep.base);
    let mut last = String::new();
    for attempt in 1..=CHUNK_ATTEMPTS {
        let res = ep
            .client
            .put(&url)
            .bearer_auth(&ep.token)
            .header(CHUNK_SHA_HEADER, &sha)
            .timeout(Duration::from_secs(120))
            .body(data.clone())
            .send()
            .await;
        match res {
            Ok(r) if r.status().is_success() => return Ok(()),
            Ok(r) => {
                let status = r.status();
                last = error_text(r).await;
                // A rejected checksum is transient (corruption in flight); other
                // client errors mean the receiver gave up — stop retrying.
                if status.is_client_error()
                    && status.as_u16() != 408
                    && status.as_u16() != 429
                    && !last.contains("checksum")
                {
                    return Err(format!("chunk {index}: {last}"));
                }
            }
            Err(e) => last = e.to_string(),
        }
        tokio::time::sleep(backoff(attempt)).await;
    }
    Err(format!(
        "chunk {index} failed after {CHUNK_ATTEMPTS} attempts: {last}"
    ))
}

/// Upload the archive being written into `source` (pack ‖ send). `kind` is
/// "full" or "project". Returns the receiver's apply result.
pub async fn upload(
    ep: &Endpoint,
    kind: &str,
    source: Arc<Growing>,
    progress: Progress,
) -> Result<Value, TransferError> {
    let resp = ep
        .client
        .post(format!("{}/api/sync/v2/upload", ep.base))
        .bearer_auth(&ep.token)
        .json(&json!({ "kind": kind }))
        .send()
        .await
        .map_err(|e| TransferError::Failed(format!("Could not start the upload: {e}")))?;
    if resp.status().as_u16() == 404 || resp.status().as_u16() == 405 {
        return Err(TransferError::Unsupported);
    }
    if !resp.status().is_success() {
        return Err(TransferError::Failed(error_text(resp).await));
    }
    let id = resp
        .json::<Value>()
        .await
        .ok()
        .and_then(|v| v.get("id").and_then(|x| x.as_str()).map(str::to_string))
        .ok_or_else(|| TransferError::Failed("upload start returned no id".into()))?;

    let next = Arc::new(AtomicU64::new(0));
    let sent = Arc::new(AtomicU64::new(0));
    let failed: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let mut workers = tokio::task::JoinSet::new();
    for _ in 0..ep.parallel.max(1) {
        let (ep, id, source, next, sent, failed, progress) = (
            ep.clone(),
            id.clone(),
            Arc::clone(&source),
            Arc::clone(&next),
            Arc::clone(&sent),
            Arc::clone(&failed),
            Arc::clone(&progress),
        );
        workers.spawn(async move {
            loop {
                if failed.lock().map(|f| f.is_some()).unwrap_or(true) {
                    return;
                }
                let index = next.fetch_add(1, Ordering::SeqCst);
                let start = index * CHUNK_SIZE;
                let end = start + CHUNK_SIZE;
                let (committed, done) = loop {
                    let (c, d, e) = wait_growing(&source, end).await;
                    if let Some(e) = e {
                        *failed.lock().unwrap_or_else(|p| p.into_inner()) =
                            Some(format!("packing failed: {e}"));
                        return;
                    }
                    if c >= end || d {
                        break (c, d);
                    }
                };
                if done && start >= committed {
                    return;
                }
                let len = (committed.min(end)) - start;
                let src = Arc::clone(&source);
                let data =
                    match tokio::task::spawn_blocking(move || src.read_range(start, len)).await {
                        Ok(Ok(d)) => d,
                        Ok(Err(e)) => {
                            *failed.lock().unwrap_or_else(|p| p.into_inner()) = Some(e.to_string());
                            return;
                        }
                        Err(e) => {
                            *failed.lock().unwrap_or_else(|p| p.into_inner()) = Some(e.to_string());
                            return;
                        }
                    };
                if let Err(e) = put_chunk(&ep, &id, index, data).await {
                    *failed.lock().unwrap_or_else(|p| p.into_inner()) = Some(e);
                    return;
                }
                let total_now = sent.fetch_add(len, Ordering::SeqCst) + len;
                let (c, d, _) = source.snapshot();
                progress("uploading", total_now, if d { c } else { 0 });
            }
        });
    }
    while workers.join_next().await.is_some() {}
    if let Some(e) = failed.lock().unwrap_or_else(|p| p.into_inner()).take() {
        return Err(TransferError::Failed(e));
    }
    let (total, _, _) = source.snapshot();

    // Resume sweep: anything the receiver still lacks goes again, then finish.
    for _round in 0..3 {
        let received: Vec<u64> = match ep
            .client
            .get(format!("{}/api/sync/v2/upload/{id}", ep.base))
            .bearer_auth(&ep.token)
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => r
                .json::<Value>()
                .await
                .ok()
                .and_then(|v| serde_json::from_value(v.get("received")?.clone()).ok())
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        let have: std::collections::HashSet<u64> = received.into_iter().collect();
        for index in (0..chunk_count(total)).filter(|i| !have.contains(i)) {
            let start = index * CHUNK_SIZE;
            let len = (total - start).min(CHUNK_SIZE);
            let src = Arc::clone(&source);
            let data = tokio::task::spawn_blocking(move || src.read_range(start, len))
                .await
                .map_err(|e| TransferError::Failed(e.to_string()))?
                .map_err(|e| TransferError::Failed(e.to_string()))?;
            put_chunk(ep, &id, index, data)
                .await
                .map_err(TransferError::Failed)?;
        }
        progress("applying", total, total);
        let resp = ep
            .client
            .post(format!("{}/api/sync/v2/upload/{id}/finish", ep.base))
            .bearer_auth(&ep.token)
            .json(&json!({ "total_size": total }))
            .send()
            .await
            .map_err(|e| TransferError::Failed(format!("finishing the upload failed: {e}")))?;
        if resp.status().as_u16() == 409 {
            continue; // receiver still misses chunks — resend and retry
        }
        if !resp.status().is_success() {
            return Err(TransferError::Failed(error_text(resp).await));
        }
        return resp
            .json::<Value>()
            .await
            .map_err(|e| TransferError::Failed(e.to_string()));
    }
    Err(TransferError::Failed(
        "the receiver kept reporting missing chunks".into(),
    ))
}

/// Fetch one chunk: `Ok(Some(bytes, sha))`, `Ok(None)` past the end, retrying
/// transient failures and waiting while the sender is still packing.
async fn get_chunk(
    ep: &Endpoint,
    id: &str,
    index: u64,
) -> Result<Option<(Vec<u8>, String)>, String> {
    let url = format!("{}/api/sync/v2/download/{id}/chunk/{index}", ep.base);
    let mut attempt = 0u32;
    let mut last;
    loop {
        let res = ep
            .client
            .get(&url)
            .bearer_auth(&ep.token)
            .timeout(Duration::from_secs(120))
            .send()
            .await;
        match res {
            Ok(r) if r.status().as_u16() == 204 => return Ok(None),
            Ok(r) if r.status().as_u16() == 425 => {
                tokio::time::sleep(Duration::from_millis(200)).await;
                continue; // still being packed — not a failure
            }
            Ok(r) if r.status().is_success() => {
                let sha = r
                    .headers()
                    .get(CHUNK_SHA_HEADER)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_string();
                match r.bytes().await {
                    Ok(b) if super::sha256_hex(&b) == sha => return Ok(Some((b.to_vec(), sha))),
                    Ok(_) => last = format!("chunk {index} failed its checksum"),
                    Err(e) => last = e.to_string(),
                }
            }
            Ok(r) => {
                let status = r.status();
                last = error_text(r).await;
                if status.is_client_error() && status.as_u16() != 408 && status.as_u16() != 429 {
                    return Err(format!("chunk {index}: {last}"));
                }
            }
            Err(e) => last = e.to_string(),
        }
        attempt += 1;
        if attempt >= CHUNK_ATTEMPTS {
            return Err(format!(
                "chunk {index} failed after {CHUNK_ATTEMPTS} attempts: {last}"
            ));
        }
        tokio::time::sleep(backoff(attempt)).await;
    }
}

/// Download an archive into `target` (receive ‖ extract: hand
/// `target.growing().reader()` to the extractor before calling this).
/// `request` is the pull body (`{ projects, projectId }`).
pub async fn download(
    ep: &Endpoint,
    request: Value,
    target: Arc<Assembler>,
    progress: Progress,
) -> Result<(), TransferError> {
    let resp = ep
        .client
        .post(format!("{}/api/sync/v2/download", ep.base))
        .bearer_auth(&ep.token)
        .json(&request)
        .send()
        .await
        .map_err(|e| TransferError::Failed(format!("Could not start the download: {e}")))?;
    if resp.status().as_u16() == 404 || resp.status().as_u16() == 405 {
        return Err(TransferError::Unsupported);
    }
    if !resp.status().is_success() {
        return Err(TransferError::Failed(error_text(resp).await));
    }
    let id = resp
        .json::<Value>()
        .await
        .ok()
        .and_then(|v| v.get("id").and_then(|x| x.as_str()).map(str::to_string))
        .ok_or_else(|| TransferError::Failed("download start returned no id".into()))?;

    let result = download_chunks(ep, &id, &target, &progress).await;
    let _ = ep
        .client
        .delete(format!("{}/api/sync/v2/download/{id}", ep.base))
        .bearer_auth(&ep.token)
        .send()
        .await;
    if let Err(e) = &result {
        target.fail(e.to_string());
    }
    result
}

/// Worker pool for [`download`].
async fn download_chunks(
    ep: &Endpoint,
    id: &str,
    target: &Arc<Assembler>,
    progress: &Progress,
) -> Result<(), TransferError> {
    let next = Arc::new(AtomicU64::new(0));
    let end_at = Arc::new(AtomicU64::new(u64::MAX));
    let got = Arc::new(AtomicU64::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let failed: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let mut workers = tokio::task::JoinSet::new();
    for _ in 0..ep.parallel.max(1) {
        let (ep, id, target, next, end_at, got, stop, failed, progress) = (
            ep.clone(),
            id.to_string(),
            Arc::clone(target),
            Arc::clone(&next),
            Arc::clone(&end_at),
            Arc::clone(&got),
            Arc::clone(&stop),
            Arc::clone(&failed),
            Arc::clone(progress),
        );
        workers.spawn(async move {
            while !stop.load(Ordering::SeqCst) {
                let index = next.fetch_add(1, Ordering::SeqCst);
                if index >= end_at.load(Ordering::SeqCst) {
                    return;
                }
                match get_chunk(&ep, &id, index).await {
                    Ok(Some((data, sha))) => {
                        let len = data.len() as u64;
                        let t = Arc::clone(&target);
                        let put =
                            tokio::task::spawn_blocking(move || t.put(index, &data, &sha)).await;
                        if let Err(e) = put.map_err(|e| e.to_string()).and_then(|r| r) {
                            *failed.lock().unwrap_or_else(|p| p.into_inner()) = Some(e);
                            stop.store(true, Ordering::SeqCst);
                            return;
                        }
                        let n = got.fetch_add(len, Ordering::SeqCst) + len;
                        progress("downloading", n, 0);
                    }
                    Ok(None) => {
                        end_at.fetch_min(index, Ordering::SeqCst);
                        return;
                    }
                    Err(e) => {
                        *failed.lock().unwrap_or_else(|p| p.into_inner()) = Some(e);
                        stop.store(true, Ordering::SeqCst);
                        return;
                    }
                }
            }
        });
    }
    while workers.join_next().await.is_some() {}
    if let Some(e) = failed.lock().unwrap_or_else(|p| p.into_inner()).take() {
        return Err(TransferError::Failed(e));
    }

    // Learn the final size, then fetch anything still missing (resume).
    let status: Value = ep
        .client
        .get(format!("{}/api/sync/v2/download/{id}", ep.base))
        .bearer_auth(&ep.token)
        .send()
        .await
        .map_err(|e| TransferError::Failed(e.to_string()))?
        .json()
        .await
        .map_err(|e| TransferError::Failed(e.to_string()))?;
    if let Some(e) = status.get("error").and_then(|v| v.as_str()) {
        return Err(TransferError::Failed(format!(
            "the other side failed to pack: {e}"
        )));
    }
    let total = status
        .get("committed")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    target.set_total(total);
    for index in target.missing() {
        match get_chunk(ep, id, index).await {
            Ok(Some((data, sha))) => {
                let t = Arc::clone(target);
                tokio::task::spawn_blocking(move || t.put(index, &data, &sha))
                    .await
                    .map_err(|e| TransferError::Failed(e.to_string()))?
                    .map_err(TransferError::Failed)?;
            }
            Ok(None) => {
                return Err(TransferError::Failed(format!(
                    "chunk {index} vanished on the other side"
                )))
            }
            Err(e) => return Err(TransferError::Failed(e)),
        }
    }
    if !target.is_complete() {
        return Err(TransferError::Failed("download incomplete".into()));
    }
    progress("downloading", total, total);
    Ok(())
}

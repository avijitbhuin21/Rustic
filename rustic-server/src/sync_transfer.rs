//! Cloud-sync archive apply/build shared by the legacy single-stream routes
//! (`/api/sync/push|pull`) and the chunked `/api/sync/v2/*` routes.

use std::collections::HashSet;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::http::HeaderMap;
use serde::Deserialize;
use serde_json::{json, Value};

use rustic_app::cloud_sync::{
    apply_project_archive_from, apply_sync_archive_from, safe_dir_name, PeerProjectState,
    SyncManifest, SyncProjectEntry, SyncReporter,
};
use rustic_app::transfer::routes::TransferHost;
use rustic_app::transfer::session::{ApplyFn, BuildFn, SessionManager};

use crate::app::Shared;

/// Apply an incoming archive stream: `scoped` replaces one project's tree,
/// otherwise the whole environment. Refreshes git credentials afterwards.
pub fn apply_incoming(
    shared: &Shared,
    source: Box<dyn Read + Send>,
    scoped: bool,
) -> Result<SyncManifest, String> {
    let ctx = &shared.ctx;
    let emitter: Arc<dyn rustic_app::EventEmitter> = Arc::new(ctx.clone());
    let projects_root = ctx.data_dir.join("projects");
    // Known project ids keep their server location; new ones land under
    // <data_dir>/projects/<name> (deduped against this import batch).
    let used: Mutex<HashSet<String>> = Default::default();
    let resolve = |entry: &SyncProjectEntry, old: Option<&str>| -> PathBuf {
        if let Some(old) = old {
            let p = PathBuf::from(old);
            if p.is_dir() {
                return p;
            }
        }
        let base = safe_dir_name(&entry.name);
        let mut used = used.lock().unwrap_or_else(|p| p.into_inner());
        let mut candidate = base.clone();
        let mut n = 1;
        while !used.insert(candidate.clone()) {
            n += 1;
            candidate = format!("{base}-{n}");
        }
        projects_root.join(candidate)
    };
    let reporter = SyncReporter::new("push", emitter.clone());
    let res = if scoped {
        apply_project_archive_from(
            &ctx.state,
            &ctx.data_dir,
            source,
            emitter,
            &resolve,
            &reporter,
        )
    } else {
        apply_sync_archive_from(
            &ctx.state,
            &ctx.data_dir,
            &*ctx.secrets,
            source,
            emitter,
            &resolve,
            &reporter,
        )
    };
    if res.is_ok() {
        // The imported environment may carry a different git token.
        let token = ctx.state.git_token.lock().ok().and_then(|g| (*g).clone());
        crate::git_credentials::apply(&shared.config.data_dir, token.as_deref());
    }
    res
}

/// A pull request body (legacy and v2 share it).
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PullRequest {
    /// The client's per-project sync state; unchanged trees are skipped.
    #[serde(default)]
    pub projects: Vec<PeerProjectState>,
    /// Build a single-project archive instead of a full-environment one.
    #[serde(default)]
    pub project_id: Option<String>,
}

/// Build an outgoing archive into `sink`.
pub fn build_outgoing(
    shared: &Shared,
    sink: Box<dyn Write + Send>,
    req: &PullRequest,
) -> Result<SyncManifest, String> {
    let ctx = &shared.ctx;
    let emitter: Arc<dyn rustic_app::EventEmitter> = Arc::new(ctx.clone());
    let reporter = SyncReporter::new("pull", emitter);
    if let Some(pid) = &req.project_id {
        return rustic_app::cloud_sync::build_project_archive_into(
            &ctx.state, pid, sink, &reporter,
        );
    }
    let skips = rustic_app::cloud_sync::decide_skips(&ctx.state, &ctx.data_dir, &req.projects);
    rustic_app::cloud_sync::build_sync_archive_into(
        &ctx.state,
        &ctx.data_dir,
        &*ctx.secrets,
        sink,
        &skips,
        &reporter,
    )
}

/// Chunked-transfer host for rustic-server (auth is the router middleware).
/// Holds `Shared` weakly: the router must not extend the server state's
/// lifetime (dropping `AppState` inside a request would drop its runtime there).
pub struct ServerTransferHost {
    shared: std::sync::Weak<Shared>,
    sessions: Arc<SessionManager>,
}

impl ServerTransferHost {
    /// Sessions keep partial files under `<data_dir>/sync-sessions`.
    pub fn new(shared: Arc<Shared>) -> Arc<Self> {
        let dir = shared.config.data_dir.join("sync-sessions");
        let _ = std::fs::remove_dir_all(&dir); // leftovers from a previous run
        Arc::new(Self {
            sessions: SessionManager::new(dir),
            shared: Arc::downgrade(&shared),
        })
    }

    fn shared(&self) -> Result<Arc<Shared>, String> {
        self.shared
            .upgrade()
            .ok_or_else(|| "server is shutting down".to_string())
    }
}

impl TransferHost for ServerTransferHost {
    fn sessions(&self) -> Arc<SessionManager> {
        Arc::clone(&self.sessions)
    }

    fn apply_for(&self, kind: &str, _headers: &HeaderMap) -> Result<ApplyFn, String> {
        let scoped = match kind {
            "project" => true,
            "full" => false,
            other => return Err(format!("unknown archive kind: {other}")),
        };
        let shared = self.shared()?;
        Ok(Box::new(move |source| {
            let m = apply_incoming(&shared, source, scoped)?;
            Ok(json!({ "ok": true, "projects": m.projects.len() }))
        }))
    }

    fn build_for(&self, body: &Value, _headers: &HeaderMap) -> Result<BuildFn, String> {
        let req: PullRequest =
            serde_json::from_value(body.clone()).map_err(|e| format!("bad pull request: {e}"))?;
        let shared = self.shared()?;
        Ok(Box::new(move |sink| {
            let m = build_outgoing(&shared, sink, &req)?;
            Ok(json!({ "projects": m.projects.len() }))
        }))
    }
}

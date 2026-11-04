//! Public issue tracker API. Anyone (people on the site, the Rustic agent,
//! scripts) can file and browse issues without a GitHub account.
//!
//! Storage is Postgres (`DATABASE_URL`); attachments live in the same database
//! so one managed, backed-up store holds everything. Moderation (close /
//! reopen / delete) has no public endpoint: it is declared in
//! `issue-actions.json` and applied idempotently on every deploy.

use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axum::{
    extract::{ConnectInfo, DefaultBodyLimit, FromRequest, Multipart, Path, Query, Request, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;
use sqlx::{postgres::PgPoolOptions, PgPool, Row};
use tower_http::set_header::SetResponseHeaderLayer;

pub const MAX_FILES: usize = 5;
pub const MAX_FILE_BYTES: usize = 10 * 1024 * 1024;
const MIN_TITLE: usize = 4;
const MAX_TITLE: usize = 160;
const MAX_BODY: usize = 20_000;
const RATE_LIMIT: usize = 10;
const RATE_WINDOW: Duration = Duration::from_secs(3600);
const KINDS: &[&str] = &["bug", "feature", "question"];
const SOURCES: &[&str] = &["web", "agent", "api"];

const SCHEMA: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS issues (
        id BIGSERIAL PRIMARY KEY,
        title TEXT NOT NULL,
        body TEXT NOT NULL DEFAULT '',
        kind TEXT NOT NULL DEFAULT 'bug',
        status TEXT NOT NULL DEFAULT 'open',
        source TEXT NOT NULL DEFAULT 'web',
        app_version TEXT,
        os TEXT,
        contact TEXT,
        resolution TEXT,
        created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
        closed_at TIMESTAMPTZ
    )",
    "CREATE INDEX IF NOT EXISTS issues_status_created ON issues (status, created_at DESC)",
    "CREATE TABLE IF NOT EXISTS issue_attachments (
        id BIGSERIAL PRIMARY KEY,
        issue_id BIGINT NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
        filename TEXT NOT NULL,
        content_type TEXT NOT NULL,
        size BIGINT NOT NULL,
        data BYTEA NOT NULL,
        created_at TIMESTAMPTZ NOT NULL DEFAULT now()
    )",
    "CREATE INDEX IF NOT EXISTS issue_attachments_issue ON issue_attachments (issue_id)",
    "CREATE TABLE IF NOT EXISTS issue_actions_applied (
        action_id TEXT PRIMARY KEY,
        applied_at TIMESTAMPTZ NOT NULL DEFAULT now()
    )",
];

/// Shared API state: the pool (absent when `DATABASE_URL` is unset) and the create rate limiter.
#[derive(Clone)]
pub struct IssuesState {
    db: Option<PgPool>,
    limiter: Arc<Mutex<HashMap<IpAddr, Vec<Instant>>>>,
}

impl IssuesState {
    /// State without a database: every API call answers 503 (local static-only runs).
    pub fn disabled() -> Self {
        Self { db: None, limiter: Arc::default() }
    }

    /// Lazily connect to `DATABASE_URL`, then migrate + apply moderation actions in the background.
    pub fn from_env() -> Self {
        let Some(url) = std::env::var("DATABASE_URL").ok().filter(|u| !u.trim().is_empty()) else {
            tracing::warn!("DATABASE_URL not set — issues API disabled");
            return Self::disabled();
        };
        let pool = match PgPoolOptions::new()
            .max_connections(5)
            .acquire_timeout(Duration::from_secs(10))
            .connect_lazy(&url)
        {
            Ok(p) => p,
            Err(err) => {
                tracing::error!(%err, "invalid DATABASE_URL — issues API disabled");
                return Self::disabled();
            }
        };
        let bg = pool.clone();
        tokio::spawn(async move {
            for attempt in 1..=30u32 {
                match prepare(&bg).await {
                    Ok(applied) => {
                        tracing::info!(applied, "issues database ready");
                        return;
                    }
                    Err(err) => {
                        tracing::warn!(attempt, %err, "issues database not ready, retrying");
                        tokio::time::sleep(Duration::from_secs(u64::from(attempt.min(10)) * 2)).await;
                    }
                }
            }
            tracing::error!("issues database never became ready");
        });
        Self { db: Some(pool), limiter: Arc::default() }
    }

    /// True when `ip` may create another issue now (does not record the attempt).
    fn may_create(&self, ip: IpAddr) -> bool {
        let mut map = self.limiter.lock().unwrap_or_else(|p| p.into_inner());
        let hits = map.entry(ip).or_default();
        hits.retain(|t| t.elapsed() < RATE_WINDOW);
        hits.len() < RATE_LIMIT
    }

    /// Count a successful create against `ip`.
    fn record_create(&self, ip: IpAddr) {
        let mut map = self.limiter.lock().unwrap_or_else(|p| p.into_inner());
        map.entry(ip).or_default().push(Instant::now());
        if map.len() > 10_000 {
            map.retain(|_, v| v.iter().any(|t| t.elapsed() < RATE_WINDOW));
        }
    }
}

/// Create tables (idempotent) and apply pending moderation actions; returns how many were applied.
pub async fn prepare(pool: &PgPool) -> Result<usize, sqlx::Error> {
    for stmt in SCHEMA {
        sqlx::query(stmt).execute(pool).await?;
    }
    apply_actions(pool, &load_actions()).await
}

/// API routes under `/api/issues`, never cached by browsers or proxies.
pub fn router(state: IssuesState) -> Router {
    Router::new()
        .route("/api/issues", get(list_issues).post(create_issue))
        .route("/api/issues/:id", get(get_issue))
        .route("/api/issues/:id/attachments/:aid", get(get_attachment))
        .layer(DefaultBodyLimit::max(MAX_FILES * MAX_FILE_BYTES + 1024 * 1024))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        ))
        .with_state(state)
}

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": msg.into() }))).into_response()
}

fn db_or_503(st: &IssuesState) -> Result<&PgPool, Response> {
    st.db.as_ref().ok_or_else(|| err(StatusCode::SERVICE_UNAVAILABLE, "the issue tracker is not configured"))
}

fn db_err(e: sqlx::Error) -> Response {
    tracing::error!(%e, "issues query failed");
    err(StatusCode::SERVICE_UNAVAILABLE, "the issue tracker is temporarily unavailable")
}

/// Client IP: first `X-Forwarded-For` hop (Railway's proxy), then `X-Real-IP`, then the socket.
fn client_ip(headers: &HeaderMap, addr: SocketAddr) -> IpAddr {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .and_then(|v| v.trim().parse().ok())
        .or_else(|| headers.get("x-real-ip").and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse().ok()))
        .unwrap_or_else(|| addr.ip())
}

/// Content type for an allowed attachment, verified against magic bytes; `None` = rejected.
fn classify_file(name: &str, data: &[u8]) -> Option<&'static str> {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase())?;
    match ext.as_str() {
        "png" if data.starts_with(b"\x89PNG\r\n\x1a\n") => Some("image/png"),
        "jpg" | "jpeg" if data.starts_with(&[0xFF, 0xD8, 0xFF]) => Some("image/jpeg"),
        "gif" if data.starts_with(b"GIF8") => Some("image/gif"),
        "webp" if data.len() > 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP" => Some("image/webp"),
        "pdf" if data.starts_with(b"%PDF-") => Some("application/pdf"),
        "txt" | "log" | "md" | "json" | "csv" | "yaml" | "yml" | "toml" if std::str::from_utf8(data).is_ok() => {
            Some("text/plain; charset=utf-8")
        }
        _ => None,
    }
}

/// Base name only, control characters stripped, at most 120 chars.
fn clean_filename(raw: &str) -> String {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or(raw);
    let cleaned: String = base.chars().filter(|c| !c.is_control()).take(120).collect();
    let cleaned = cleaned.trim().trim_start_matches('.').to_string();
    if cleaned.is_empty() { "attachment".into() } else { cleaned }
}

/// A validated new issue plus its attachments.
#[derive(Default)]
struct NewIssue {
    title: String,
    body: String,
    kind: String,
    source: String,
    app_version: Option<String>,
    os: Option<String>,
    contact: Option<String>,
    honeypot: bool,
    files: Vec<(String, &'static str, Vec<u8>)>,
}

#[derive(Deserialize)]
struct NewIssueJson {
    title: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    app_version: Option<String>,
    #[serde(default)]
    os: Option<String>,
    #[serde(default)]
    contact: Option<String>,
    #[serde(default)]
    website: Option<String>,
}

fn opt(v: Option<String>, max: usize) -> Option<String> {
    v.map(|s| s.trim().chars().take(max).collect::<String>()).filter(|s| !s.is_empty())
}

impl NewIssue {
    /// Normalise fields and enforce limits; `Err` is a user-facing message.
    fn validate(mut self) -> Result<Self, String> {
        self.title = self.title.trim().to_string();
        self.body = self.body.trim().to_string();
        let n = self.title.chars().count();
        if !(MIN_TITLE..=MAX_TITLE).contains(&n) {
            return Err(format!("title must be {MIN_TITLE}–{MAX_TITLE} characters"));
        }
        if self.body.chars().count() > MAX_BODY {
            return Err(format!("body must be at most {MAX_BODY} characters"));
        }
        self.kind = self.kind.trim().to_ascii_lowercase();
        if self.kind.is_empty() {
            self.kind = "bug".into();
        }
        if !KINDS.contains(&self.kind.as_str()) {
            return Err("kind must be bug, feature or question".into());
        }
        self.source = self.source.trim().to_ascii_lowercase();
        if !SOURCES.contains(&self.source.as_str()) {
            self.source = "api".into();
        }
        self.app_version = opt(self.app_version, 40);
        self.os = opt(self.os, 60);
        self.contact = opt(self.contact, 200);
        Ok(self)
    }
}

/// Read a multipart create request (text fields + up to [`MAX_FILES`] files).
async fn read_multipart(mut mp: Multipart) -> Result<NewIssue, String> {
    let mut out = NewIssue { source: "web".into(), ..Default::default() };
    while let Some(field) = mp.next_field().await.map_err(|e| format!("bad upload: {e}"))? {
        let name = field.name().unwrap_or_default().to_string();
        if let Some(fname) = field.file_name().map(str::to_string).filter(|f| !f.is_empty()) {
            if out.files.len() >= MAX_FILES {
                return Err(format!("at most {MAX_FILES} attachments"));
            }
            let data = field.bytes().await.map_err(|e| format!("bad upload: {e}"))?;
            if data.len() > MAX_FILE_BYTES {
                return Err(format!("{fname} is larger than {} MB", MAX_FILE_BYTES / (1024 * 1024)));
            }
            let fname = clean_filename(&fname);
            let ct = classify_file(&fname, &data).ok_or_else(|| {
                format!("{fname}: only images (png, jpg, gif, webp), pdf and text files (txt, log, md, json, csv, yaml, toml) are accepted")
            })?;
            out.files.push((fname, ct, data.to_vec()));
            continue;
        }
        let text = field.text().await.map_err(|e| format!("bad field {name}: {e}"))?;
        match name.as_str() {
            "title" => out.title = text,
            "body" => out.body = text,
            "kind" => out.kind = text,
            "source" => out.source = text,
            "app_version" => out.app_version = Some(text),
            "os" => out.os = Some(text),
            "contact" => out.contact = Some(text),
            "website" => out.honeypot = !text.trim().is_empty(),
            _ => {}
        }
    }
    Ok(out)
}

/// `POST /api/issues` — multipart (with files) or JSON (text only).
async fn create_issue(
    State(st): State<IssuesState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    req: Request,
) -> Response {
    let pool = match db_or_503(&st) {
        Ok(p) => p.clone(),
        Err(r) => return r,
    };
    let ip = client_ip(req.headers(), addr);
    if !st.may_create(ip) {
        return err(StatusCode::TOO_MANY_REQUESTS, format!("rate limit: at most {RATE_LIMIT} new issues per hour"));
    }
    let is_json = req
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/json"));
    let parsed = if is_json {
        match Json::<NewIssueJson>::from_request(req, &()).await {
            Ok(Json(j)) => Ok(NewIssue {
                title: j.title,
                body: j.body,
                kind: j.kind.unwrap_or_default(),
                source: j.source.unwrap_or_else(|| "api".into()),
                app_version: j.app_version,
                os: j.os,
                contact: j.contact,
                honeypot: j.website.is_some_and(|w| !w.trim().is_empty()),
                files: Vec::new(),
            }),
            Err(e) => Err(e.body_text()),
        }
    } else {
        match Multipart::from_request(req, &()).await {
            Ok(mp) => read_multipart(mp).await,
            Err(e) => Err(e.body_text()),
        }
    };
    let issue = match parsed.and_then(NewIssue::validate) {
        Ok(i) => i,
        Err(msg) => return err(StatusCode::BAD_REQUEST, msg),
    };
    if issue.honeypot {
        // Looks accepted to the bot; nothing is stored.
        return (StatusCode::CREATED, Json(json!({ "id": 0 }))).into_response();
    }
    let id = match insert_issue(&pool, &issue).await {
        Ok(id) => id,
        Err(e) => return db_err(e),
    };
    st.record_create(ip);
    tracing::info!(id, source = %issue.source, files = issue.files.len(), "issue created");
    (
        StatusCode::CREATED,
        Json(json!({ "id": id, "url": format!("/issues/?id={id}"), "api": format!("/api/issues/{id}") })),
    )
        .into_response()
}

async fn insert_issue(pool: &PgPool, i: &NewIssue) -> Result<i64, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let id: i64 = sqlx::query(
        "INSERT INTO issues (title, body, kind, source, app_version, os, contact)
         VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING id",
    )
    .bind(&i.title)
    .bind(&i.body)
    .bind(&i.kind)
    .bind(&i.source)
    .bind(&i.app_version)
    .bind(&i.os)
    .bind(&i.contact)
    .fetch_one(&mut *tx)
    .await?
    .try_get("id")?;
    for (name, ct, data) in &i.files {
        sqlx::query(
            "INSERT INTO issue_attachments (issue_id, filename, content_type, size, data) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(id)
        .bind(name)
        .bind(*ct)
        .bind(data.len() as i64)
        .bind(data.as_slice())
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(id)
}

#[derive(Deserialize)]
struct ListQuery {
    status: Option<String>,
    q: Option<String>,
    page: Option<i64>,
    per_page: Option<i64>,
}

/// `GET /api/issues?status=open|closed|all&q=&page=&per_page=` — newest first.
async fn list_issues(State(st): State<IssuesState>, Query(q): Query<ListQuery>) -> Response {
    let pool = match db_or_503(&st) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let status = match q.status.as_deref().unwrap_or("open") {
        s @ ("open" | "closed" | "all") => s.to_string(),
        _ => return err(StatusCode::BAD_REQUEST, "status must be open, closed or all"),
    };
    let search = q.q.unwrap_or_default().trim().chars().take(100).collect::<String>();
    let pattern = format!("%{}%", search.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
    let per_page = q.per_page.unwrap_or(30).clamp(1, 100);
    let page = q.page.unwrap_or(1).max(1);
    let filter = "($1 = 'all' OR i.status = $1) AND ($2 = '' OR i.title ILIKE $3 OR i.body ILIKE $3)";
    let rows = sqlx::query(&format!(
        "SELECT i.id, i.title, i.kind, i.status, i.source, LEFT(i.body, 240) AS excerpt,
                EXTRACT(EPOCH FROM i.created_at)::BIGINT AS created_at,
                EXTRACT(EPOCH FROM i.closed_at)::BIGINT AS closed_at,
                (SELECT COUNT(*) FROM issue_attachments a WHERE a.issue_id = i.id) AS attachments
         FROM issues i WHERE {filter}
         ORDER BY i.created_at DESC LIMIT $4 OFFSET $5"
    ))
    .bind(&status)
    .bind(&search)
    .bind(&pattern)
    .bind(per_page)
    .bind((page - 1) * per_page)
    .fetch_all(pool)
    .await;
    let rows = match rows {
        Ok(r) => r,
        Err(e) => return db_err(e),
    };
    let total: i64 = match sqlx::query(&format!("SELECT COUNT(*) AS n FROM issues i WHERE {filter}"))
        .bind(&status)
        .bind(&search)
        .bind(&pattern)
        .fetch_one(pool)
        .await
        .and_then(|r| r.try_get("n"))
    {
        Ok(n) => n,
        Err(e) => return db_err(e),
    };
    let mut counts = json!({ "open": 0, "closed": 0 });
    match sqlx::query("SELECT status, COUNT(*) AS n FROM issues GROUP BY status").fetch_all(pool).await {
        Ok(cs) => {
            for r in cs {
                let s: String = r.try_get("status").unwrap_or_default();
                let n: i64 = r.try_get("n").unwrap_or(0);
                counts[s] = json!(n);
            }
        }
        Err(e) => return db_err(e),
    }
    let issues: Vec<_> = rows
        .iter()
        .map(|r| {
            json!({
                "id": r.try_get::<i64, _>("id").unwrap_or_default(),
                "title": r.try_get::<String, _>("title").unwrap_or_default(),
                "kind": r.try_get::<String, _>("kind").unwrap_or_default(),
                "status": r.try_get::<String, _>("status").unwrap_or_default(),
                "source": r.try_get::<String, _>("source").unwrap_or_default(),
                "excerpt": r.try_get::<String, _>("excerpt").unwrap_or_default(),
                "created_at": r.try_get::<i64, _>("created_at").unwrap_or_default(),
                "closed_at": r.try_get::<Option<i64>, _>("closed_at").unwrap_or_default(),
                "attachments": r.try_get::<i64, _>("attachments").unwrap_or_default(),
            })
        })
        .collect();
    Json(json!({ "issues": issues, "total": total, "page": page, "per_page": per_page, "counts": counts })).into_response()
}

/// `GET /api/issues/:id` — full issue with attachment metadata (contact is never exposed).
async fn get_issue(State(st): State<IssuesState>, Path(id): Path<i64>) -> Response {
    let pool = match db_or_503(&st) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let row = sqlx::query(
        "SELECT id, title, body, kind, status, source, app_version, os, resolution,
                EXTRACT(EPOCH FROM created_at)::BIGINT AS created_at,
                EXTRACT(EPOCH FROM closed_at)::BIGINT AS closed_at
         FROM issues WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await;
    let r = match row {
        Ok(Some(r)) => r,
        Ok(None) => return err(StatusCode::NOT_FOUND, "no such issue"),
        Err(e) => return db_err(e),
    };
    let files = match sqlx::query(
        "SELECT id, filename, content_type, size FROM issue_attachments WHERE issue_id = $1 ORDER BY id",
    )
    .bind(id)
    .fetch_all(pool)
    .await
    {
        Ok(f) => f,
        Err(e) => return db_err(e),
    };
    let attachments: Vec<_> = files
        .iter()
        .map(|f| {
            let aid: i64 = f.try_get("id").unwrap_or_default();
            json!({
                "id": aid,
                "filename": f.try_get::<String, _>("filename").unwrap_or_default(),
                "content_type": f.try_get::<String, _>("content_type").unwrap_or_default(),
                "size": f.try_get::<i64, _>("size").unwrap_or_default(),
                "url": format!("/api/issues/{id}/attachments/{aid}"),
            })
        })
        .collect();
    Json(json!({
        "id": r.try_get::<i64, _>("id").unwrap_or_default(),
        "title": r.try_get::<String, _>("title").unwrap_or_default(),
        "body": r.try_get::<String, _>("body").unwrap_or_default(),
        "kind": r.try_get::<String, _>("kind").unwrap_or_default(),
        "status": r.try_get::<String, _>("status").unwrap_or_default(),
        "source": r.try_get::<String, _>("source").unwrap_or_default(),
        "app_version": r.try_get::<Option<String>, _>("app_version").unwrap_or_default(),
        "os": r.try_get::<Option<String>, _>("os").unwrap_or_default(),
        "resolution": r.try_get::<Option<String>, _>("resolution").unwrap_or_default(),
        "created_at": r.try_get::<i64, _>("created_at").unwrap_or_default(),
        "closed_at": r.try_get::<Option<i64>, _>("closed_at").unwrap_or_default(),
        "attachments": attachments,
    }))
    .into_response()
}

/// `GET /api/issues/:id/attachments/:aid` — images inline, everything else as a download, sandboxed.
async fn get_attachment(State(st): State<IssuesState>, Path((id, aid)): Path<(i64, i64)>) -> Response {
    let pool = match db_or_503(&st) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let row = sqlx::query("SELECT filename, content_type, data FROM issue_attachments WHERE id = $1 AND issue_id = $2")
        .bind(aid)
        .bind(id)
        .fetch_optional(pool)
        .await;
    let r = match row {
        Ok(Some(r)) => r,
        Ok(None) => return err(StatusCode::NOT_FOUND, "no such attachment"),
        Err(e) => return db_err(e),
    };
    let name: String = r.try_get("filename").unwrap_or_default();
    let ct: String = r.try_get("content_type").unwrap_or_else(|_| "application/octet-stream".into());
    let data: Vec<u8> = r.try_get("data").unwrap_or_default();
    let safe_name: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || ".-_ ".contains(c) { c } else { '_' }).collect();
    let disposition = if ct.starts_with("image/") { "inline" } else { "attachment" };
    let mut resp = data.into_response();
    let h = resp.headers_mut();
    for (k, v) in [
        (header::CONTENT_TYPE, ct),
        (header::CONTENT_DISPOSITION, format!("{disposition}; filename=\"{safe_name}\"")),
        (header::CONTENT_SECURITY_POLICY, "default-src 'none'; sandbox".into()),
        (header::CACHE_CONTROL, "public, max-age=86400".into()),
    ] {
        if let Ok(v) = HeaderValue::from_str(&v) {
            h.insert(k, v);
        }
    }
    resp
}

/// One deploy-time moderation step from `issue-actions.json`.
#[derive(Deserialize, Debug)]
pub struct Action {
    /// Unique, never reused — each id is applied once.
    pub id: String,
    /// `close`, `reopen` or `delete`.
    pub op: String,
    pub issue: i64,
    #[serde(default)]
    pub resolution: Option<String>,
}

#[derive(Deserialize, Default)]
struct ActionsFile {
    #[serde(default)]
    actions: Vec<Action>,
}

/// Path of the moderation file: `ISSUE_ACTIONS_FILE` or the crate-local `issue-actions.json`.
fn actions_path() -> PathBuf {
    std::env::var_os("ISSUE_ACTIONS_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("issue-actions.json"))
}

/// Parse the moderation file (missing or invalid → no actions, logged).
pub fn load_actions() -> Vec<Action> {
    let path = actions_path();
    let Ok(text) = std::fs::read_to_string(&path) else { return Vec::new() };
    match serde_json::from_str::<ActionsFile>(&text) {
        Ok(f) => f.actions,
        Err(e) => {
            tracing::error!(path = %path.display(), %e, "issue-actions.json is invalid — no moderation applied");
            Vec::new()
        }
    }
}

/// Apply every action whose id hasn't run yet, each in its own transaction; returns the count applied.
pub async fn apply_actions(pool: &PgPool, actions: &[Action]) -> Result<usize, sqlx::Error> {
    let mut applied = 0;
    for a in actions {
        let mut tx = pool.begin().await?;
        let fresh = sqlx::query("INSERT INTO issue_actions_applied (action_id) VALUES ($1) ON CONFLICT DO NOTHING")
            .bind(&a.id)
            .execute(&mut *tx)
            .await?
            .rows_affected()
            == 1;
        if !fresh {
            continue;
        }
        let res = match a.op.as_str() {
            "close" => sqlx::query("UPDATE issues SET status = 'closed', closed_at = now(), resolution = $2 WHERE id = $1")
                .bind(a.issue)
                .bind(&a.resolution)
                .execute(&mut *tx)
                .await?,
            "reopen" => sqlx::query("UPDATE issues SET status = 'open', closed_at = NULL, resolution = NULL WHERE id = $1")
                .bind(a.issue)
                .execute(&mut *tx)
                .await?,
            "delete" => sqlx::query("DELETE FROM issues WHERE id = $1").bind(a.issue).execute(&mut *tx).await?,
            other => {
                tracing::warn!(id = %a.id, op = other, "unknown issue action — skipped");
                tx.rollback().await?;
                continue;
            }
        };
        tx.commit().await?;
        tracing::info!(id = %a.id, op = %a.op, issue = a.issue, matched = res.rows_affected(), "issue action applied");
        applied += 1;
    }
    Ok(applied)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_by_extension_and_magic() {
        assert_eq!(classify_file("a.PNG", b"\x89PNG\r\n\x1a\nrest"), Some("image/png"));
        assert_eq!(classify_file("shot.png", b"not a png"), None);
        assert_eq!(classify_file("crash.log", b"panic at main.rs"), Some("text/plain; charset=utf-8"));
        assert_eq!(classify_file("bad.txt", &[0xFF, 0xFE, 0x00]), None);
        assert_eq!(classify_file("x.exe", b"MZ"), None);
        assert_eq!(classify_file("noext", b"hello"), None);
    }

    #[test]
    fn cleans_filenames() {
        assert_eq!(clean_filename("../../etc/passwd"), "passwd");
        assert_eq!(clean_filename("C:\\Users\\me\\log.txt"), "log.txt");
        assert_eq!(clean_filename(".hidden"), "hidden");
        assert_eq!(clean_filename("a\u{0}b.txt"), "ab.txt");
        assert_eq!(clean_filename(""), "attachment");
    }

    #[test]
    fn validates_new_issues() {
        let ok = NewIssue { title: "  Crash on open  ".into(), kind: "".into(), source: "agent".into(), ..Default::default() };
        let v = ok.validate().unwrap();
        assert_eq!(v.title, "Crash on open");
        assert_eq!(v.kind, "bug");
        assert_eq!(v.source, "agent");
        assert!(NewIssue { title: "no".into(), ..Default::default() }.validate().is_err());
        assert!(NewIssue { title: "Valid title".into(), kind: "spam".into(), ..Default::default() }.validate().is_err());
        let src = NewIssue { title: "Valid title".into(), source: "evil".into(), ..Default::default() }.validate().unwrap();
        assert_eq!(src.source, "api");
    }

    #[test]
    fn rate_limits_per_ip() {
        let st = IssuesState::disabled();
        let ip: IpAddr = "203.0.113.7".parse().unwrap();
        for _ in 0..RATE_LIMIT {
            assert!(st.may_create(ip));
            st.record_create(ip);
        }
        assert!(!st.may_create(ip));
        assert!(st.may_create("203.0.113.8".parse().unwrap()));
    }

    #[test]
    fn reads_forwarded_client_ip() {
        let addr: SocketAddr = "10.0.0.1:1234".parse().unwrap();
        let mut h = HeaderMap::new();
        assert_eq!(client_ip(&h, addr), "10.0.0.1".parse::<IpAddr>().unwrap());
        h.insert("x-forwarded-for", HeaderValue::from_static("198.51.100.4, 10.0.0.1"));
        assert_eq!(client_ip(&h, addr), "198.51.100.4".parse::<IpAddr>().unwrap());
    }
}

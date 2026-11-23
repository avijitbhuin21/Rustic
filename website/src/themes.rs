//! Theme Park API: browse and share Rustic themes without an account.
//!
//! A shared theme is public and immutable. Nothing is stored until someone
//! shares; the page keeps drafts in the browser. Every value is checked with
//! the same rules the app applies on import (`crates/rustic-app/src/themes.rs`),
//! and custom CSS is refused outright, so a theme from here can only change
//! colors and the listed style settings.
//!
//! Storage is the tracker's Postgres. Moderation has no public endpoint: hiding
//! a theme is declared in `theme-actions.json` and applied once per deploy.

use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axum::{
    extract::{rejection::JsonRejection, ConnectInfo, DefaultBodyLimit, Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sqlx::{PgPool, Row};
use tower_http::set_header::SetResponseHeaderLayer;

use crate::issues::client_ip;

/// Theme file format this site accepts (the app's `themes::FORMAT`).
pub const FORMAT: u64 = 2;
/// File name suffix the app's importer expects.
pub const EXT: &str = ".rustic-theme.json";
const MAX_BODY_BYTES: usize = 32 * 1024;
const MAX_NAME_BYTES: usize = 80;
const MAX_AUTHOR: usize = 80;
const MAX_VERSION: usize = 20;
const MAX_DESCRIPTION: usize = 300;
const RATE_LIMIT: usize = 10;
const RATE_WINDOW: Duration = Duration::from_secs(3600);

/// The color slots of one mode, in file order.
pub const COLOR_SLOTS: [&str; 31] = [
    "bg_hard", "bg", "bg_soft", "bg1", "bg2", "bg3", "bg4",
    "fg", "fg1", "fg2", "fg3", "fg4",
    "accent", "primary", "border",
    "bright_red", "bright_green", "bright_yellow", "bright_blue", "bright_purple", "bright_aqua", "bright_orange",
    "token_keyword", "token_string", "token_comment", "token_function", "token_type",
    "token_variable", "token_number", "token_operator", "token_punctuation",
];

const STYLE_KEYS: [&str; 11] = [
    "font_sans", "font_mono", "radius", "motion", "border_width",
    "shadow", "surface", "density", "button", "texture", "ink",
];

/// Allowed values for the enum-like style keys.
const STYLE_ENUMS: [(&str, &[&str]); 7] = [
    ("motion", &["full", "reduced"]),
    ("shadow", &["none", "soft", "strong", "glow", "hard"]),
    ("surface", &["solid", "glass"]),
    ("density", &["compact", "normal", "comfortable"]),
    ("button", &["solid", "pill", "raised", "brutal", "glow"]),
    ("texture", &["none", "grain", "grid", "dots", "scanlines", "paper"]),
    ("ink", &["none", "sketch"]),
];

const SCHEMA: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS themes (
        id BIGSERIAL PRIMARY KEY,
        name TEXT NOT NULL,
        author TEXT,
        description TEXT,
        modes TEXT NOT NULL,
        pack JSONB NOT NULL,
        content_hash TEXT NOT NULL UNIQUE,
        seed_key TEXT UNIQUE,
        hidden BOOLEAN NOT NULL DEFAULT false,
        created_at TIMESTAMPTZ NOT NULL DEFAULT now()
    )",
    "CREATE INDEX IF NOT EXISTS themes_visible_created ON themes (hidden, created_at DESC)",
    "CREATE TABLE IF NOT EXISTS theme_actions_applied (
        action_id TEXT PRIMARY KEY,
        applied_at TIMESTAMPTZ NOT NULL DEFAULT now()
    )",
];

/// Shared API state: the pool (absent when the site runs without a database) and the share rate limiter.
#[derive(Clone)]
pub struct ThemesState {
    db: Option<PgPool>,
    limiter: Arc<Mutex<HashMap<IpAddr, Vec<Instant>>>>,
}

impl ThemesState {
    /// State without a database: every API call answers 503.
    pub fn disabled() -> Self {
        Self { db: None, limiter: Arc::default() }
    }

    /// Use the tracker's pool; create tables, load seed themes and apply moderation in the background.
    pub fn new(db: Option<PgPool>) -> Self {
        let Some(pool) = db else {
            tracing::warn!("no database — Theme Park API disabled");
            return Self::disabled();
        };
        let bg = pool.clone();
        tokio::spawn(async move {
            for attempt in 1..=30u32 {
                match prepare(&bg).await {
                    Ok((seeded, applied)) => {
                        tracing::info!(seeded, applied, "theme park database ready");
                        return;
                    }
                    Err(err) => {
                        tracing::warn!(attempt, %err, "theme park database not ready, retrying");
                        tokio::time::sleep(Duration::from_secs(u64::from(attempt.min(10)) * 2)).await;
                    }
                }
            }
            tracing::error!("theme park database never became ready");
        });
        Self { db: Some(pool), limiter: Arc::default() }
    }

    /// True when `ip` may share another theme now (does not record the attempt).
    fn may_share(&self, ip: IpAddr) -> bool {
        let mut map = self.limiter.lock().unwrap_or_else(|p| p.into_inner());
        let hits = map.entry(ip).or_default();
        hits.retain(|t| t.elapsed() < RATE_WINDOW);
        hits.len() < RATE_LIMIT
    }

    /// Count a stored share against `ip`.
    fn record_share(&self, ip: IpAddr) {
        let mut map = self.limiter.lock().unwrap_or_else(|p| p.into_inner());
        map.entry(ip).or_default().push(Instant::now());
        if map.len() > 10_000 {
            map.retain(|_, v| v.iter().any(|t| t.elapsed() < RATE_WINDOW));
        }
    }
}

/// Create tables (idempotent), insert missing seed themes and apply pending moderation; returns (seeded, applied).
pub async fn prepare(pool: &PgPool) -> Result<(usize, usize), sqlx::Error> {
    for stmt in SCHEMA {
        sqlx::query(stmt).execute(pool).await?;
    }
    let seeded = insert_seeds(pool, &load_seeds()).await?;
    let applied = apply_actions(pool, &load_actions()).await?;
    Ok((seeded, applied))
}

/// API routes under `/api/themes`, never cached by browsers or proxies.
pub fn router(state: ThemesState) -> Router {
    Router::new()
        .route("/api/themes", get(list_themes).post(share_theme))
        .route("/api/themes/:id", get(get_theme))
        .route("/api/themes/:id/file", get(get_theme_file))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        ))
        .with_state(state)
}

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": msg.into() }))).into_response()
}

fn db_or_503(st: &ThemesState) -> Result<&PgPool, Response> {
    st.db.as_ref().ok_or_else(|| err(StatusCode::SERVICE_UNAVAILABLE, "Theme Park is not configured"))
}

fn db_err(e: sqlx::Error) -> Response {
    tracing::error!(%e, "themes query failed");
    err(StatusCode::SERVICE_UNAVAILABLE, "Theme Park is temporarily unavailable")
}

/// Whether `v` is a single CSS color and nothing that could carry other CSS. Same rule as the app.
pub fn is_safe_color(v: &str) -> bool {
    let v = v.trim();
    if v.is_empty() || v.len() > 80 {
        return false;
    }
    if v.contains(['{', '}', ';', '<', '>', '\\', '"', '\'', '@', '!']) || v.to_ascii_lowercase().contains("url") {
        return false;
    }
    if let Some(hex) = v.strip_prefix('#') {
        return matches!(hex.len(), 3 | 4 | 6 | 8) && hex.chars().all(|c| c.is_ascii_hexdigit());
    }
    let lower = v.to_ascii_lowercase();
    let funcs = [
        "rgb(", "rgba(", "hsl(", "hsla(", "hwb(", "lab(", "lch(", "oklab(", "oklch(", "color(", "color-mix(",
    ];
    if funcs.iter().any(|f| lower.starts_with(f)) {
        return lower.ends_with(')') && lower.chars().all(|c| c.is_ascii_alphanumeric() || " .,%/()-+#".contains(c));
    }
    lower.chars().all(|c| c.is_ascii_alphabetic())
}

/// Whether a style value (font stack, length, keyword) is plain enough to use in CSS. Same rule as the app.
fn is_safe_style_value(v: &str) -> bool {
    let v = v.trim();
    !v.is_empty()
        && v.len() <= 200
        && !v.to_ascii_lowercase().contains("url")
        && v.chars().all(|c| c.is_ascii_alphanumeric() || " .,%-_()'\"/".contains(c))
}

/// A one-line text field: trimmed, whitespace runs collapsed, control characters refused.
fn text_field(v: Option<&Value>, key: &str, max_chars: usize) -> Result<Option<String>, String> {
    let s = match v {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::String(s)) => s,
        Some(_) => return Err(format!("\"{key}\" must be text")),
    };
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.chars().any(char::is_control) {
        return Err(format!("\"{key}\" contains characters that are not allowed"));
    }
    if s.chars().count() > max_chars {
        return Err(format!("\"{key}\" must be at most {max_chars} characters"));
    }
    Ok(Some(s).filter(|s| !s.is_empty()))
}

/// The colors of one mode: known slots with valid values. Blank values mean "inherit" and are left out.
fn mode_colors(v: Option<&Value>, mode: &str) -> Result<Option<Map<String, Value>>, String> {
    let obj = match v {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::Object(o)) => o,
        Some(_) => return Err(format!("\"{mode}\" must be an object of colors")),
    };
    let mut out = Map::new();
    for slot in COLOR_SLOTS {
        let value = match obj.get(slot) {
            None | Some(Value::Null) => continue,
            Some(Value::String(s)) if s.trim().is_empty() => continue,
            Some(Value::String(s)) => s.trim(),
            Some(_) => return Err(format!("{mode}.{slot} must be a color")),
        };
        if !is_safe_color(value) {
            let shown: String = value.chars().take(40).collect();
            return Err(format!("{mode}.{slot}: \"{shown}\" isn't a valid color"));
        }
        out.insert(slot.to_string(), Value::String(value.to_string()));
    }
    Ok(Some(out))
}

/// The style block: known keys with valid values.
fn style_values(v: Option<&Value>) -> Result<Map<String, Value>, String> {
    let obj = match v {
        None | Some(Value::Null) => return Ok(Map::new()),
        Some(Value::Object(o)) => o,
        Some(_) => return Err("\"style\" must be an object".into()),
    };
    let mut out = Map::new();
    for key in STYLE_KEYS {
        let value = match obj.get(key) {
            None | Some(Value::Null) => continue,
            Some(Value::String(s)) if s.trim().is_empty() => continue,
            Some(Value::String(s)) => s.trim(),
            Some(_) => return Err(format!("style.{key} must be text")),
        };
        if !is_safe_style_value(value) {
            let shown: String = value.chars().take(40).collect();
            return Err(format!("style.{key}: \"{shown}\" isn't allowed"));
        }
        if let Some((_, allowed)) = STYLE_ENUMS.iter().find(|(k, _)| *k == key) {
            if !allowed.contains(&value) {
                return Err(format!("style.{key}: use one of {}", allowed.join(", ")));
            }
        }
        out.insert(key.to_string(), Value::String(value.to_string()));
    }
    Ok(out)
}

/// A theme accepted for sharing, rebuilt from known keys only.
#[derive(Debug, Clone, PartialEq)]
pub struct SharedPack {
    pub name: String,
    pub author: Option<String>,
    pub description: Option<String>,
    /// `"dark"`, `"light"` or `"dark,light"`.
    pub modes: String,
    /// The canonical theme file as JSON.
    pub pack: Value,
}

/// Validate a theme file object for sharing and rebuild it from known keys. `Err` is a user-facing message.
pub fn canonical_pack(input: &Value) -> Result<SharedPack, String> {
    let src = input.as_object().ok_or("The theme must be a JSON object")?;
    match src.get("format") {
        None | Some(Value::Null) => {}
        Some(Value::Number(n)) if n.as_u64().is_some_and(|f| f <= FORMAT) => {}
        Some(Value::Number(n)) => return Err(format!("This theme needs a newer Rustic (format {n}, supported {FORMAT})")),
        Some(_) => return Err("\"format\" must be a number".into()),
    }
    match src.get("css") {
        None | Some(Value::Null) => {}
        Some(Value::String(s)) if s.trim().is_empty() => {}
        Some(_) => return Err("Shared themes can't include custom CSS. Remove \"css\" and share again.".into()),
    }
    let name = text_field(src.get("name"), "name", MAX_NAME_BYTES)?
        .filter(|n| n.len() <= MAX_NAME_BYTES)
        .ok_or(format!("The theme needs a name (up to {MAX_NAME_BYTES} characters)"))?;
    let author = text_field(src.get("author"), "author", MAX_AUTHOR)?;
    let version = text_field(src.get("version"), "version", MAX_VERSION)?;
    let description = text_field(src.get("description"), "description", MAX_DESCRIPTION)?;
    let dark = mode_colors(src.get("dark"), "dark")?;
    let light = mode_colors(src.get("light"), "light")?;
    if dark.as_ref().is_none_or(Map::is_empty) && light.as_ref().is_none_or(Map::is_empty) {
        return Err("The theme defines no colors".into());
    }
    let style = style_values(src.get("style"))?;

    let mut modes = Vec::new();
    let mut pack = Map::new();
    pack.insert("format".into(), json!(FORMAT));
    pack.insert("name".into(), json!(name));
    for (key, value) in [("author", &author), ("version", &version), ("description", &description)] {
        if let Some(v) = value {
            pack.insert(key.into(), json!(v));
        }
    }
    for (mode, colors) in [("dark", dark), ("light", light)] {
        if let Some(c) = colors.filter(|c| !c.is_empty()) {
            modes.push(mode);
            pack.insert(mode.into(), Value::Object(c));
        }
    }
    pack.insert("style".into(), Value::Object(style));
    Ok(SharedPack { name, author, description, modes: modes.join(","), pack: Value::Object(pack) })
}

/// The theme file as text, keys in the order the app writes them.
pub fn to_file_text(pack: &Value) -> String {
    let empty = Map::new();
    let obj = pack.as_object().unwrap_or(&empty);
    let quote = |v: &Value| serde_json::to_string(v).unwrap_or_else(|_| "null".into());
    let block = |map: &Map<String, Value>, order: &[&str]| -> String {
        let lines: Vec<String> = order
            .iter()
            .filter_map(|k| map.get(*k).map(|v| format!("    {}: {}", quote(&json!(k)), quote(v))))
            .collect();
        if lines.is_empty() { "{}".into() } else { format!("{{\n{}\n  }}", lines.join(",\n")) }
    };
    let mut parts = Vec::new();
    for key in ["format", "name", "author", "version", "description"] {
        if let Some(v) = obj.get(key) {
            parts.push(format!("  {}: {}", quote(&json!(key)), quote(v)));
        }
    }
    for mode in ["dark", "light"] {
        if let Some(Value::Object(colors)) = obj.get(mode) {
            parts.push(format!("  {}: {}", quote(&json!(mode)), block(colors, &COLOR_SLOTS)));
        }
    }
    if let Some(Value::Object(style)) = obj.get("style") {
        parts.push(format!("  \"style\": {}", block(style, &STYLE_KEYS)));
    }
    format!("{{\n{}\n}}\n", parts.join(",\n"))
}

/// File name stem for a theme: lowercase ASCII letters and digits joined by single dashes.
fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-');
    if out.is_empty() { "theme".into() } else { out.chars().take(60).collect() }
}

#[derive(Deserialize)]
struct ListQuery {
    q: Option<String>,
    mode: Option<String>,
    page: Option<i64>,
    per_page: Option<i64>,
}

const THEME_COLUMNS: &str = "id, name, author, description, modes, seed_key IS NOT NULL AS seed, pack::text AS pack,
     EXTRACT(EPOCH FROM created_at)::BIGINT AS created_at";

/// One theme row as API JSON.
fn theme_json(r: &sqlx::postgres::PgRow) -> Value {
    let pack: String = r.try_get("pack").unwrap_or_default();
    let modes: String = r.try_get("modes").unwrap_or_default();
    let id: i64 = r.try_get("id").unwrap_or_default();
    json!({
        "id": id,
        "name": r.try_get::<String, _>("name").unwrap_or_default(),
        "author": r.try_get::<Option<String>, _>("author").unwrap_or_default(),
        "description": r.try_get::<Option<String>, _>("description").unwrap_or_default(),
        "modes": modes.split(',').filter(|m| !m.is_empty()).collect::<Vec<_>>(),
        "seed": r.try_get::<bool, _>("seed").unwrap_or_default(),
        "created_at": r.try_get::<i64, _>("created_at").unwrap_or_default(),
        "url": format!("/theme-park/?id={id}"),
        "file": format!("/api/themes/{id}/file"),
        "pack": serde_json::from_str::<Value>(&pack).unwrap_or(Value::Null),
    })
}

/// `GET /api/themes?q=&mode=all|dark|light&page=&per_page=` — newest first, each with its full theme file.
async fn list_themes(State(st): State<ThemesState>, Query(q): Query<ListQuery>) -> Response {
    let pool = match db_or_503(&st) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let mode = match q.mode.as_deref().unwrap_or("all") {
        m @ ("all" | "dark" | "light") => m.to_string(),
        _ => return err(StatusCode::BAD_REQUEST, "mode must be all, dark or light"),
    };
    let search = q.q.unwrap_or_default().trim().chars().take(100).collect::<String>();
    let pattern = format!("%{}%", search.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
    let per_page = q.per_page.unwrap_or(24).clamp(1, 60);
    let page = q.page.unwrap_or(1).max(1);
    let filter = "NOT hidden AND ($1 = 'all' OR modes LIKE '%' || $1 || '%')
                  AND ($2 = '' OR name ILIKE $3 OR author ILIKE $3 OR description ILIKE $3)";
    let rows = sqlx::query(&format!(
        "SELECT {THEME_COLUMNS} FROM themes WHERE {filter} ORDER BY created_at DESC, id DESC LIMIT $4 OFFSET $5"
    ))
    .bind(&mode)
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
    let total: i64 = match sqlx::query(&format!("SELECT COUNT(*) AS n FROM themes WHERE {filter}"))
        .bind(&mode)
        .bind(&search)
        .bind(&pattern)
        .fetch_one(pool)
        .await
        .and_then(|r| r.try_get("n"))
    {
        Ok(n) => n,
        Err(e) => return db_err(e),
    };
    let themes: Vec<Value> = rows.iter().map(theme_json).collect();
    Json(json!({ "themes": themes, "total": total, "page": page, "per_page": per_page })).into_response()
}

/// Fetch one visible theme row.
async fn fetch_theme(pool: &PgPool, id: i64) -> Result<Option<sqlx::postgres::PgRow>, sqlx::Error> {
    sqlx::query(&format!("SELECT {THEME_COLUMNS} FROM themes WHERE id = $1 AND NOT hidden"))
        .bind(id)
        .fetch_optional(pool)
        .await
}

/// `GET /api/themes/:id` — one theme with its file.
async fn get_theme(State(st): State<ThemesState>, Path(id): Path<i64>) -> Response {
    let pool = match db_or_503(&st) {
        Ok(p) => p,
        Err(r) => return r,
    };
    match fetch_theme(pool, id).await {
        Ok(Some(r)) => Json(theme_json(&r)).into_response(),
        Ok(None) => err(StatusCode::NOT_FOUND, "no such theme"),
        Err(e) => db_err(e),
    }
}

/// `GET /api/themes/:id/file` — the theme as a `.rustic-theme.json` download.
async fn get_theme_file(State(st): State<ThemesState>, Path(id): Path<i64>) -> Response {
    let pool = match db_or_503(&st) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let row = match fetch_theme(pool, id).await {
        Ok(Some(r)) => r,
        Ok(None) => return err(StatusCode::NOT_FOUND, "no such theme"),
        Err(e) => return db_err(e),
    };
    let name: String = row.try_get("name").unwrap_or_default();
    let pack: String = row.try_get("pack").unwrap_or_default();
    let pack = serde_json::from_str::<Value>(&pack).unwrap_or(Value::Null);
    let mut resp = to_file_text(&pack).into_response();
    let h = resp.headers_mut();
    for (k, v) in [
        (header::CONTENT_TYPE, "application/json; charset=utf-8".to_string()),
        (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{}{EXT}\"", slug(&name))),
        (header::CACHE_CONTROL, "public, max-age=86400".into()),
    ] {
        if let Ok(v) = HeaderValue::from_str(&v) {
            h.insert(k, v);
        }
    }
    resp
}

/// What storing a share came to.
#[derive(Debug, PartialEq)]
pub enum Stored {
    New(i64),
    /// The identical theme was shared before; this is its id.
    Existing(i64),
    /// The identical theme was shared before and has been removed.
    Removed,
}

/// Store a validated theme, or find the identical one already stored.
pub async fn store_theme(pool: &PgPool, p: &SharedPack) -> Result<Stored, sqlx::Error> {
    let pack = p.pack.to_string();
    // Look first: an insert that hits the unique hash would still use up an id.
    let existing = sqlx::query("SELECT id, hidden FROM themes WHERE content_hash = md5($1::jsonb::text)")
        .bind(&pack)
        .fetch_optional(pool)
        .await?;
    if let Some(row) = existing {
        if row.try_get::<bool, _>("hidden")? {
            return Ok(Stored::Removed);
        }
        return Ok(Stored::Existing(row.try_get("id")?));
    }
    let inserted = sqlx::query(
        "INSERT INTO themes (name, author, description, modes, pack, content_hash)
         VALUES ($1, $2, $3, $4, $5::jsonb, md5($5::jsonb::text))
         ON CONFLICT (content_hash) DO NOTHING RETURNING id",
    )
    .bind(&p.name)
    .bind(&p.author)
    .bind(&p.description)
    .bind(&p.modes)
    .bind(&pack)
    .fetch_optional(pool)
    .await?;
    match inserted {
        Some(row) => Ok(Stored::New(row.try_get("id")?)),
        // Two identical shares at the same moment: the other one won.
        None => {
            let row = sqlx::query("SELECT id FROM themes WHERE content_hash = md5($1::jsonb::text)")
                .bind(&pack)
                .fetch_one(pool)
                .await?;
            Ok(Stored::Existing(row.try_get("id")?))
        }
    }
}

/// `POST /api/themes` with `{ "pack": { …theme file… } }` — share a theme. Public and permanent.
async fn share_theme(
    State(st): State<ThemesState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Result<Json<Value>, JsonRejection>,
) -> Response {
    let pool = match db_or_503(&st) {
        Ok(p) => p.clone(),
        Err(r) => return r,
    };
    let ip = client_ip(&headers, addr);
    if !st.may_share(ip) {
        return err(StatusCode::TOO_MANY_REQUESTS, format!("rate limit: at most {RATE_LIMIT} shared themes per hour"));
    }
    let body = match body {
        Ok(Json(v)) => v,
        Err(e) => return err(StatusCode::BAD_REQUEST, e.body_text()),
    };
    // Spam trap, same as the tracker: looks accepted, nothing is stored.
    if body.get("website").and_then(Value::as_str).is_some_and(|w| !w.trim().is_empty()) {
        return (StatusCode::CREATED, Json(json!({ "id": 0 }))).into_response();
    }
    let Some(pack) = body.get("pack") else {
        return err(StatusCode::BAD_REQUEST, "send the theme as { \"pack\": { … } }");
    };
    let shared = match canonical_pack(pack) {
        Ok(p) => p,
        Err(msg) => return err(StatusCode::BAD_REQUEST, msg),
    };
    let (status, id, existing) = match store_theme(&pool, &shared).await {
        Ok(Stored::New(id)) => {
            st.record_share(ip);
            tracing::info!(id, name = %shared.name, modes = %shared.modes, "theme shared");
            (StatusCode::CREATED, id, false)
        }
        Ok(Stored::Existing(id)) => (StatusCode::OK, id, true),
        Ok(Stored::Removed) => return err(StatusCode::CONFLICT, "This theme was removed from Theme Park."),
        Err(e) => return db_err(e),
    };
    (
        status,
        Json(json!({ "id": id, "existing": existing, "url": format!("/theme-park/?id={id}"), "file": format!("/api/themes/{id}/file") })),
    )
        .into_response()
}

/// Folder of seed themes: `THEME_SEEDS_DIR` or the crate-local `seed-themes/`.
fn seeds_dir() -> PathBuf {
    std::env::var_os("THEME_SEEDS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("seed-themes"))
}

/// Read every valid `*.rustic-theme.json` in the seeds folder as (key, theme), sorted by key. Bad files are logged and skipped.
pub fn load_seeds() -> Vec<(String, SharedPack)> {
    let dir = seeds_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else { return Vec::new() };
    let mut out = Vec::new();
    for path in entries.flatten().map(|e| e.path()) {
        let Some(key) = path.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_suffix(EXT)) else { continue };
        let parsed = std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|t| serde_json::from_str::<Value>(&t).map_err(|e| e.to_string()))
            .and_then(|v| canonical_pack(&v));
        match parsed {
            Ok(p) => out.push((key.to_string(), p)),
            Err(e) => tracing::error!(path = %path.display(), %e, "seed theme skipped"),
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Insert seeds that are not there yet; returns how many were added. A seed is never updated or re-added once its key exists.
pub async fn insert_seeds(pool: &PgPool, seeds: &[(String, SharedPack)]) -> Result<usize, sqlx::Error> {
    let mut added = 0;
    for (key, p) in seeds {
        let pack = p.pack.to_string();
        added += sqlx::query(
            "INSERT INTO themes (name, author, description, modes, pack, content_hash, seed_key)
             SELECT $1, $2, $3, $4, $5::jsonb, md5($5::jsonb::text), $6
             WHERE NOT EXISTS (
                 SELECT 1 FROM themes WHERE seed_key = $6 OR content_hash = md5($5::jsonb::text)
             )",
        )
        .bind(&p.name)
        .bind(&p.author)
        .bind(&p.description)
        .bind(&p.modes)
        .bind(&pack)
        .bind(key)
        .execute(pool)
        .await?
        .rows_affected() as usize;
    }
    Ok(added)
}

/// One deploy-time moderation step from `theme-actions.json`.
#[derive(Deserialize, Debug)]
pub struct Action {
    /// Unique, never reused — each id is applied once.
    pub id: String,
    /// `hide` or `unhide`. A hidden theme stays stored so the same file cannot be shared again.
    pub op: String,
    pub theme: i64,
}

#[derive(Deserialize, Default)]
struct ActionsFile {
    #[serde(default)]
    actions: Vec<Action>,
}

/// Parse the moderation file (`THEME_ACTIONS_FILE` or the crate-local `theme-actions.json`); missing or invalid → no actions, logged.
pub fn load_actions() -> Vec<Action> {
    let path = std::env::var_os("THEME_ACTIONS_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("theme-actions.json"));
    let Ok(text) = std::fs::read_to_string(&path) else { return Vec::new() };
    match serde_json::from_str::<ActionsFile>(&text) {
        Ok(f) => f.actions,
        Err(e) => {
            tracing::error!(path = %path.display(), %e, "theme-actions.json is invalid — no moderation applied");
            Vec::new()
        }
    }
}

/// Apply every action whose id hasn't run yet, each in its own transaction; returns the count applied.
pub async fn apply_actions(pool: &PgPool, actions: &[Action]) -> Result<usize, sqlx::Error> {
    let mut applied = 0;
    for a in actions {
        let hidden = match a.op.as_str() {
            "hide" => true,
            "unhide" => false,
            other => {
                tracing::warn!(id = %a.id, op = other, "unknown theme action — skipped");
                continue;
            }
        };
        let mut tx = pool.begin().await?;
        let fresh = sqlx::query("INSERT INTO theme_actions_applied (action_id) VALUES ($1) ON CONFLICT DO NOTHING")
            .bind(&a.id)
            .execute(&mut *tx)
            .await?
            .rows_affected()
            == 1;
        if !fresh {
            continue;
        }
        let res = sqlx::query("UPDATE themes SET hidden = $2 WHERE id = $1")
            .bind(a.theme)
            .bind(hidden)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        tracing::info!(id = %a.id, op = %a.op, theme = a.theme, matched = res.rows_affected(), "theme action applied");
        applied += 1;
    }
    Ok(applied)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Value {
        json!({
            "format": 2,
            "name": "  Night   Garden ",
            "author": "Ada",
            "description": "Deep greens.\nSoft edges.",
            "dark": { "bg": "#101412", "fg": "oklch(0.9 0.02 150)", "accent": "", "primary": null },
            "style": { "radius": "0.5rem", "button": "pill" }
        })
    }

    #[test]
    fn colors_follow_the_app_rules() {
        for ok in ["#1a2b3c", "#fff", "#ffffff80", "oklch(0.5 0.1 200)", "rebeccapurple", "transparent",
                   "color-mix(in oklab, #fff 10%, transparent)", "rgb(1 2 3 / 50%)"] {
            assert!(is_safe_color(ok), "{ok}");
        }
        for bad in ["", "#12345", "#fff; background: url(http://x)", "red}body{display:none", "url(x)",
                    "rgb(1,2,3", "var(--x)", "expression(1)", "#fff !important", "@import x"] {
            assert!(!is_safe_color(bad), "{bad}");
        }
        assert!(!is_safe_color(&"a".repeat(81)));
    }

    #[test]
    fn rebuilds_a_theme_from_known_keys() {
        let mut input = sample();
        input["unknown"] = json!("dropped");
        input["dark"]["not_a_slot"] = json!("#000");
        input["style"]["made_up"] = json!("x");
        let p = canonical_pack(&input).unwrap();
        assert_eq!(p.name, "Night Garden");
        assert_eq!(p.modes, "dark");
        assert_eq!(p.description.as_deref(), Some("Deep greens. Soft edges."));
        assert_eq!(
            p.pack,
            json!({
                "format": 2, "name": "Night Garden", "author": "Ada", "description": "Deep greens. Soft edges.",
                "dark": { "bg": "#101412", "fg": "oklch(0.9 0.02 150)" },
                "style": { "radius": "0.5rem", "button": "pill" }
            })
        );
        // Sharing the result again changes nothing.
        assert_eq!(canonical_pack(&p.pack).unwrap(), p);
    }

    #[test]
    fn refuses_what_the_app_would_refuse_and_all_css() {
        let with = |f: fn(&mut Value)| {
            let mut v = sample();
            f(&mut v);
            canonical_pack(&v)
        };
        assert!(with(|v| v["css"] = json!("body{display:none}")).unwrap_err().contains("custom CSS"));
        assert!(with(|v| v["css"] = json!({ "a": 1 })).is_err());
        assert!(with(|v| v["css"] = json!("   ")).is_ok());
        assert!(with(|v| v["dark"]["bg"] = json!("red;} body{")).unwrap_err().contains("dark.bg"));
        assert!(with(|v| v["dark"]["bg"] = json!(12)).is_err());
        assert!(with(|v| v["dark"] = json!("#000")).is_err());
        assert!(with(|v| v["style"]["button"] = json!("huge")).unwrap_err().contains("style.button"));
        assert!(with(|v| v["style"]["font_sans"] = json!("x; } body { color: red")).is_err());
        assert!(with(|v| v["style"]["font_sans"] = json!("\"Inter\", sans-serif")).is_ok());
        assert!(with(|v| v["format"] = json!(3)).unwrap_err().contains("newer Rustic"));
        assert!(with(|v| v["name"] = json!("   ")).is_err());
        assert!(with(|v| v["name"] = json!("n".repeat(81))).is_err());
        assert!(with(|v| v["name"] = json!("é".repeat(41))).is_err(), "the app limits the name in bytes");
        assert!(with(|v| v["name"] = json!(7)).is_err());
        assert!(with(|v| v["description"] = json!("d".repeat(301))).is_err());
        assert!(with(|v| v["dark"] = json!({})).unwrap_err().contains("no colors"));
        assert!(canonical_pack(&json!([1, 2])).is_err());
        assert!(canonical_pack(&json!({ "name": "x", "light": { "bg": "#fff" }, "dark": { "bg": "#000" } }))
            .is_ok_and(|p| p.modes == "dark,light"));
    }

    #[test]
    fn every_seed_theme_is_accepted() {
        let seeds = load_seeds();
        let names: Vec<&str> = seeds.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(names, ["basalt", "grove", "hearth", "midnight", "neo-brutal", "neon-grid", "onyx", "petal"]);
        for (key, p) in &seeds {
            assert!(!p.modes.is_empty(), "{key}");
            assert!(p.pack.get("css").is_none(), "{key}");
            for mode in p.modes.split(',') {
                assert_eq!(p.pack[mode].as_object().map(Map::len), Some(COLOR_SLOTS.len()), "{key} {mode}");
            }
        }
    }

    #[test]
    fn file_text_is_ordered_and_parses_back() {
        let p = canonical_pack(&sample()).unwrap();
        let text = to_file_text(&p.pack);
        assert_eq!(serde_json::from_str::<Value>(&text).unwrap(), p.pack);
        let at = |needle: &str| text.find(needle).unwrap_or_else(|| panic!("{needle} missing in {text}"));
        assert!(at("\"format\"") < at("\"name\"") && at("\"name\"") < at("\"author\""));
        assert!(at("\"description\"") < at("\"dark\"") && at("\"dark\"") < at("\"style\""));
        assert!(at("\"bg\"") < at("\"fg\""), "slots keep file order, not alphabetical");
        assert!(text.ends_with("}\n"));
        assert!(to_file_text(&json!({ "format": 2, "name": "x", "dark": { "bg": "#000" }, "style": {} })).contains("\"style\": {}"));
    }

    #[test]
    fn slugs_file_names() {
        assert_eq!(slug("Night Garden"), "night-garden");
        assert_eq!(slug("  Neo/Brutal!! 2 "), "neo-brutal-2");
        assert_eq!(slug("日本語"), "theme");
        assert_eq!(slug("../../etc/passwd"), "etc-passwd");
        assert!(slug(&"a b ".repeat(40)).len() <= 60);
    }

    #[test]
    fn rate_limits_shares_per_ip() {
        let st = ThemesState::disabled();
        let ip: IpAddr = "203.0.113.7".parse().unwrap();
        for _ in 0..RATE_LIMIT {
            assert!(st.may_share(ip));
            st.record_share(ip);
        }
        assert!(!st.may_share(ip));
        assert!(st.may_share("203.0.113.8".parse().unwrap()));
    }

    /// Runs against a real Postgres when `TEST_DATABASE_URL` is set; skipped otherwise.
    #[tokio::test]
    async fn stores_lists_dedupes_and_hides() {
        let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
            eprintln!("TEST_DATABASE_URL not set — skipping the database test");
            return;
        };
        let pool = PgPool::connect(&url).await.expect("connect");
        for t in ["themes", "theme_actions_applied"] {
            sqlx::query(&format!("DROP TABLE IF EXISTS {t}")).execute(&pool).await.expect("drop");
        }
        let (seeded, applied) = prepare(&pool).await.expect("prepare");
        assert_eq!((seeded, applied), (8, 0));
        assert_eq!(prepare(&pool).await.expect("again").0, 0, "seeds are added once");
        let next_id = |pool: &PgPool| {
            let pool = pool.clone();
            async move {
                sqlx::query("SELECT COALESCE(MAX(id), 0) AS n FROM themes")
                    .fetch_one(&pool).await.expect("max").try_get::<i64, _>("n").unwrap()
            }
        };
        assert_eq!(next_id(&pool).await, 8);

        let mine = canonical_pack(&sample()).unwrap();
        let Stored::New(id) = store_theme(&pool, &mine).await.expect("store") else { panic!("expected a new row") };
        assert_eq!(id, 9, "restarts and repeat shares do not use up ids");
        // The same file with keys in another order is the same theme.
        let reordered: Value = serde_json::from_str(
            r##"{"style":{"button":"pill","radius":"0.5rem"},"dark":{"fg":"oklch(0.9 0.02 150)","bg":"#101412"},
                "description":"Deep greens. Soft edges.","author":"Ada","name":"Night Garden"}"##,
        )
        .unwrap();
        assert_eq!(store_theme(&pool, &canonical_pack(&reordered).unwrap()).await.expect("dup"), Stored::Existing(id));

        let row = fetch_theme(&pool, id).await.expect("fetch").expect("row");
        let got = theme_json(&row);
        assert_eq!(got["pack"], mine.pack);
        assert_eq!(got["modes"], json!(["dark"]));
        assert_eq!(got["seed"], json!(false));

        let hide = [Action { id: "t-hide".into(), op: "hide".into(), theme: id }];
        assert_eq!(apply_actions(&pool, &hide).await.expect("hide"), 1);
        assert_eq!(apply_actions(&pool, &hide).await.expect("hide again"), 0);
        assert!(fetch_theme(&pool, id).await.expect("fetch").is_none());
        assert_eq!(store_theme(&pool, &mine).await.expect("reshare"), Stored::Removed);

        // A hidden seed is not re-added by the next deploy.
        let seed_id: i64 = sqlx::query("SELECT id FROM themes WHERE seed_key = 'grove'")
            .fetch_one(&pool).await.expect("seed").try_get("id").unwrap();
        let hide_seed = [Action { id: "t-hide-seed".into(), op: "hide".into(), theme: seed_id }];
        apply_actions(&pool, &hide_seed).await.expect("hide seed");
        assert_eq!(prepare(&pool).await.expect("redeploy").0, 0);
        let visible: i64 = sqlx::query("SELECT COUNT(*) AS n FROM themes WHERE NOT hidden")
            .fetch_one(&pool).await.expect("count").try_get("n").unwrap();
        assert_eq!(visible, 7);
    }
}
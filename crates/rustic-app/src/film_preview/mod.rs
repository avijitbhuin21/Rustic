//! Film view: serves a project page that follows the `window.render(t)` contract
//! to a sandboxed frame, so the editor can scrub it.
//!
//! This is the one preview that runs a project's own JavaScript, so everything
//! here assumes the page is hostile:
//! - A page is reachable only through a random per-session id handed to the
//!   frame by `open`; the id is the whole credential (the frame is sandboxed to
//!   an opaque origin and carries no cookies).
//! - A session serves one folder, never above the project root, never a hidden
//!   (dot) file or folder, never through a symlink that leaves the folder, and
//!   never Rustic's own data.
//! - Every response carries a CSP that sandboxes the document and limits every
//!   fetch to the session itself; the shell document's `frame-src` keeps the
//!   page from navigating its frame anywhere else.
//! - Not contained: WebRTC. No CSP directive blocks it in Chromium, and although
//!   the bridge removes the constructors from the page, a child frame gets fresh
//!   ones (measured: STUN packets left the sandbox). A page can therefore send out
//!   what it can read, so the film view asks once per project before it loads one.

mod serve;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::sync_ext::MutexExt;

pub use serve::{percent_decode, FilmResponse};

/// URL scheme the desktop app serves film sessions on (`rustic-film://<id>/…`).
pub const SCHEME: &str = "rustic-film";

const MAX_SESSIONS: usize = 16;
const IDLE_LIMIT: Duration = Duration::from_secs(2 * 60 * 60);
const MAX_ROOT_NAMES: usize = 256;

/// Where the library writes a film's sound mix, relative to the page's folder.
const AUDIO_CANDIDATES: &[&str] = &["out/mix.wav", "mix.wav", "out/mix_en.wav", "mix_en.wav"];

/// How session URLs are laid out: the id as the URL host (desktop scheme), or
/// under `/film/<id>/` on the server's own host.
#[derive(Clone, Copy)]
pub enum Layout<'a> {
    Scheme,
    Path { host: &'a str },
}

/// What a session serves: a film page (fixed viewport, no outside network) or a
/// page in the live HTML preview (grows to its full height, may use `https:`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Film,
    Page,
}

#[derive(Clone)]
struct Session {
    kind: Kind,
    root: PathBuf,
    entry: String,
    names: Vec<String>,
    deny: Vec<PathBuf>,
    last_used: Instant,
    seq: u64,
}

/// What the frontend needs to show a film page.
#[derive(Debug, Serialize)]
pub struct FilmOpen {
    pub id: String,
    /// URL of the shell document to load in the sandboxed frame.
    pub url: String,
    /// Folder the session serves.
    pub root: String,
    /// Project root the page belongs to; the frontend keys its run-scripts consent on it.
    pub project: String,
    /// Sound mix found next to the page, if any.
    pub audio: Option<String>,
}

#[derive(Default)]
struct Inner {
    map: HashMap<String, Session>,
    seq: u64,
}

/// Live film sessions for this process.
#[derive(Default)]
pub struct FilmSessions {
    inner: Mutex<Inner>,
}

/// The process-wide session table shared by the command and the serving route.
pub fn sessions() -> &'static FilmSessions {
    static SESSIONS: OnceLock<FilmSessions> = OnceLock::new();
    SESSIONS.get_or_init(FilmSessions::default)
}

/// URL of a session's shell document.
pub fn shell_url(id: &str, scheme: bool) -> String {
    if scheme {
        format!("{SCHEME}://{id}/{}", serve::SHELL)
    } else {
        format!("/film/{id}/{}", serve::SHELL)
    }
}

impl FilmSessions {
    /// Start serving the HTML page `file`, which must sit inside one of `project_roots`.
    pub fn open(
        &self,
        file: &Path,
        project_roots: &[PathBuf],
        deny: &[PathBuf],
        scheme: bool,
    ) -> Result<FilmOpen, String> {
        self.open_kind(file, project_roots, deny, scheme, Kind::Film)
    }

    /// Start serving the HTML page `file` to the live HTML preview. The session serves
    /// the whole project so `../` links resolve, and the page may load from `https:`.
    pub fn open_page(
        &self,
        file: &Path,
        project_roots: &[PathBuf],
        deny: &[PathBuf],
        scheme: bool,
    ) -> Result<FilmOpen, String> {
        self.open_kind(file, project_roots, deny, scheme, Kind::Page)
    }

    fn open_kind(
        &self,
        file: &Path,
        project_roots: &[PathBuf],
        deny: &[PathBuf],
        scheme: bool,
        kind: Kind,
    ) -> Result<FilmOpen, String> {
        let view = match kind {
            Kind::Film => "Film view",
            Kind::Page => "The live preview",
        };
        let file =
            std::fs::canonicalize(file).map_err(|_| "The page no longer exists.".to_string())?;
        let is_html = file
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("html") || e.eq_ignore_ascii_case("htm"));
        if !file.is_file() || !is_html {
            return Err(format!("{view} only opens HTML pages."));
        }
        crate::path_scope::validate_readable_path(&file)?;
        let deny: Vec<PathBuf> = deny.iter().map(|d| canon_or_self(d)).collect();
        if deny.iter().any(|d| file.starts_with(d)) {
            return Err(format!(
                "Rustic's own data can't be opened in {}.",
                view.to_lowercase()
            ));
        }
        let project = project_roots
            .iter()
            .map(|r| canon_or_self(r))
            .filter(|r| file.starts_with(r))
            .max_by_key(|r| r.components().count())
            .ok_or(format!(
                "{view} only opens pages inside a project you have added to Rustic."
            ))?;
        let dir = file.parent().ok_or("The page has no folder.")?;
        let root = match kind {
            Kind::Film => pick_root(dir, &project),
            Kind::Page => project.clone(),
        };
        let entry = entry_url(&file, &root)?;
        let audio = match kind {
            Kind::Film => AUDIO_CANDIDATES
                .iter()
                .map(|c| dir.join(c))
                .find(|p| p.is_file())
                .map(|p| p.to_string_lossy().into_owned()),
            Kind::Page => None,
        };

        let id = uuid::Uuid::new_v4().simple().to_string();
        let mut inner = self.inner.lock_safe();
        let now = Instant::now();
        inner
            .map
            .retain(|_, s| now.duration_since(s.last_used) < IDLE_LIMIT);
        while inner.map.len() >= MAX_SESSIONS {
            let Some(oldest) = inner
                .map
                .iter()
                .min_by_key(|(_, s)| s.seq)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            inner.map.remove(&oldest);
        }
        inner.seq += 1;
        let seq = inner.seq;
        inner.map.insert(
            id.clone(),
            Session {
                kind,
                names: root_names(&root),
                root: root.clone(),
                entry,
                deny,
                last_used: now,
                seq,
            },
        );
        Ok(FilmOpen {
            url: shell_url(&id, scheme),
            id,
            root: root.to_string_lossy().into_owned(),
            project: project.to_string_lossy().into_owned(),
            audio,
        })
    }

    /// Stop serving a session. Unknown ids are ignored.
    pub fn close(&self, id: &str) {
        self.inner.lock_safe().map.remove(id);
    }

    /// Answer one request for `rel` (already percent-decoded) inside session `id`.
    pub fn respond(&self, id: &str, rel: &str, layout: Layout<'_>) -> FilmResponse {
        let session = {
            let mut inner = self.inner.lock_safe();
            inner.map.get_mut(id).map(|s| {
                s.last_used = Instant::now();
                s.clone()
            })
        };
        match session {
            Some(s) => serve::respond(id, &s, rel, layout),
            None => FilmResponse::error(404, "This film session has ended. Reload the film view."),
        }
    }
}

fn canon_or_self(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// The folder to serve: the nearest ancestor of the page (up to the project root)
/// that looks like a package root, so root-absolute URLs such as `/core/lib.js`
/// resolve; otherwise the page's own folder.
fn pick_root(dir: &Path, project: &Path) -> PathBuf {
    let mut cur = dir;
    loop {
        if cur.join("package.json").is_file() || cur.join("node_modules").is_dir() {
            return cur.to_path_buf();
        }
        if cur == project {
            break;
        }
        match cur.parent() {
            Some(p) if p.starts_with(project) => cur = p,
            _ => break,
        }
    }
    dir.to_path_buf()
}

/// The page's URL path below `root`, percent-encoded per segment.
fn entry_url(file: &Path, root: &Path) -> Result<String, String> {
    let rel = file
        .strip_prefix(root)
        .map_err(|_| "The page is outside the served folder.".to_string())?;
    let mut parts = Vec::new();
    for c in rel.components() {
        let seg = c
            .as_os_str()
            .to_str()
            .ok_or("The page's path is not valid UTF-8.")?;
        if seg.starts_with('.') {
            return Err(format!(
                "This page is in a hidden folder below {}, and hidden folders are never served.",
                root.display()
            ));
        }
        parts.push(serve::percent_encode(seg));
    }
    Ok(parts.join("/"))
}

/// Visible top-level names in `root`, used to re-point root-absolute URLs in path layout.
fn root_names(root: &Path) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    rd.flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| !n.starts_with('.'))
        .take(MAX_ROOT_NAMES)
        .collect()
}

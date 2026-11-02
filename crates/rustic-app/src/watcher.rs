use crate::context::{EventEmitter, EventEmitterExt};
use crate::sync_ext::MutexExt;
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use rustic_agent::WorkspaceRegistry;
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Directory segments filtered from watcher events to suppress build-artifact
/// noise (e.g. webpack/air triggering hundreds of events per second).
const WATCHER_SKIP_DIRS: &[&str] = &[
    ".git",
    ".rustic",
    "node_modules",
    "target",
    "dist",
    "build",
    ".next",
    ".nuxt",
    ".svelte-kit",
    "out",
    ".venv",
    "venv",
    "__pycache__",
    ".cache",
    ".turbo",
    ".parcel-cache",
    "tmp",
];

/// True when any path component (forward- or backslash-separated) of `s`
/// matches one of `WATCHER_SKIP_DIRS`. We test segment-by-segment rather
/// than `contains("/node_modules/")` so `tmp/` at the project root is caught
/// even when it's the very first segment after the root.
fn path_has_skip_segment(s: &str) -> bool {
    for seg in s.split(|c| c == '/' || c == '\\') {
        if WATCHER_SKIP_DIRS.iter().any(|d| *d == seg) {
            return true;
        }
    }
    false
}

/// Cap on `changed_paths` per flush. Keep in sync with CHANGED_PATHS_CAP in
/// src/lib/use-file-change.js — the frontend treats a full list as
/// non-exhaustive and falls back to matching parent dirs.
const CHANGED_PATHS_CAP: usize = 512;

/// Cap on queued symbol-index refreshes per flush (bursts beyond this are
/// picked up by the index's own staleness checks).
const INDEX_QUEUE_CAP: usize = 4096;

/// True when `rel` (a path relative to a `.git` directory, `/`-separated)
/// names a file whose change means the repo's status/branches/log moved:
/// the index (staging), HEAD (checkout), refs (commits, branch updates)
/// or merge/rebase markers. Everything else under `.git` (objects, logs,
/// lock files) is noise and stays filtered.
fn is_interesting_git_file(rel: &str) -> bool {
    matches!(
        rel,
        "index" | "HEAD" | "MERGE_HEAD" | "ORIG_HEAD" | "FETCH_HEAD" | "packed-refs"
    ) || rel.starts_with("refs/")
}

/// If `path` contains a `.git` segment, returns the remainder after it
/// (`/`-separated, e.g. `refs/heads/main`). Returns None for non-git paths.
fn git_relative_part(path: &str) -> Option<String> {
    let mut rest: Vec<&str> = Vec::new();
    let mut found = false;
    for seg in path.split(|c| c == '/' || c == '\\') {
        if found {
            rest.push(seg);
        } else if seg == ".git" {
            found = true;
        }
    }
    if found {
        Some(rest.join("/"))
    } else {
        None
    }
}

/// Payload emitted to the frontend when the file system changes.
#[derive(Clone, Serialize)]
pub struct FsChangeEvent {
    /// The project root this change belongs to.
    pub project_path: String,
    /// The specific paths that changed (parent directories, deduplicated).
    pub changed_dirs: Vec<String>,
    /// The individual files that changed (capped at CHANGED_PATHS_CAP; a
    /// full list may be non-exhaustive).
    pub changed_paths: Vec<String>,
    /// True when git metadata moved (.git/index, HEAD, refs, merge state) —
    /// staging, commits or checkouts done outside the app.
    pub git_changed: bool,
    /// True when events may have been lost (watcher error / OS buffer
    /// overflow / rescan hint). The frontend must re-read every cached
    /// directory and re-check open files instead of trusting the lists.
    pub rescan: bool,
}

/// Debounce accumulator shared between the notify callback and the flush
/// thread.
#[derive(Default)]
struct PendingChanges {
    dirs: HashMap<String, ()>,
    paths: HashMap<String, ()>,
    git_changed: bool,
    rescan: bool,
    /// Files whose symbol index must be refreshed (`true` = removed). Done
    /// by the flush thread, not the notify callback, so a burst of events
    /// isn't slowed down by parsing (issue #12, cause C).
    index: HashMap<String, bool>,
    last_event: Option<Instant>,
}

type UiHandler = Arc<dyn Fn(&Result<Event, notify::Error>) + Send + Sync>;
type FeedList = Arc<Mutex<Vec<rustic_agent::FileWatcherFeed>>>;

struct WatcherEntry {
    /// `None` while a background retry is pending (the retry thread owns the
    /// watcher once it starts; dropping this entry stops it via the flag).
    _watcher: Option<RecommendedWatcher>,
    flush_running: Arc<std::sync::atomic::AtomicBool>,
    has_ui: bool,
}

struct Subscribers {
    root: PathBuf,
    feeds: FeedList,
}

/// Background retry cadence for a watcher that failed to start.
const WATCH_RETRY_ATTEMPTS: u32 = 30;
const WATCH_RETRY_INTERVAL: Duration = Duration::from_secs(10);

/// Create a recursive watcher on `path` with the given callback.
fn start_watcher<F>(handler: F, path: &Path) -> Result<RecommendedWatcher, String>
where
    F: notify::EventHandler,
{
    let mut w = RecommendedWatcher::new(handler, Config::default()).map_err(|e| e.to_string())?;
    w.watch(path, RecursiveMode::Recursive)
        .map_err(|e| e.to_string())?;
    Ok(w)
}

impl Drop for WatcherEntry {
    fn drop(&mut self) {
        // Signal the per-project flush thread to exit so it doesn't linger as
        // a zombie when the project is closed. Without this the thread
        // wakes every 300ms forever on a flag it never sees flip.
        self.flush_running
            .store(false, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Push a synthetic watcher error to every live feed so file history falls back to a full walk.
fn signal_feeds_lost(feeds: &FeedList, root: &Path, why: &str) {
    let err: Result<Event, notify::Error> = Err(notify::Error::generic(why));
    feeds.lock_safe().retain(|f| f.feed(root, &err));
}

pub struct FileWatcherManager {
    watchers: HashMap<String, WatcherEntry>,
    subscribers: HashMap<String, Subscribers>,
}

impl FileWatcherManager {
    pub fn new() -> Self {
        Self {
            watchers: HashMap::new(),
            subscribers: HashMap::new(),
        }
    }

    /// Feed `project_root`'s raw events into a file-history pipeline, sharing (or starting) the project's single OS watcher.
    pub fn subscribe_file_history(
        &mut self,
        project_root: &Path,
        feed: rustic_agent::FileWatcherFeed,
    ) {
        let raw = normalize(&project_root.to_string_lossy());
        let norm = strip_verbatim(&raw).to_string();
        let key = watch_key(&norm);
        let subs = self.subscribers.entry(key.clone()).or_insert_with(|| Subscribers {
            root: native_path(&norm),
            feeds: Arc::new(Mutex::new(Vec::new())),
        });
        subs.feeds.lock_safe().push(feed);
        if !self.watchers.contains_key(&key) {
            let entry = Self::start_entry(
                &norm,
                native_path(&norm),
                subs.feeds.clone(),
                None,
                Arc::new(std::sync::atomic::AtomicBool::new(true)),
            );
            self.watchers.insert(key, entry);
        }
    }

    fn feeds_for(&mut self, key: &str) -> FeedList {
        self.subscribers
            .get(key)
            .map(|s| s.feeds.clone())
            .unwrap_or_else(|| Arc::new(Mutex::new(Vec::new())))
    }

    /// Start watching a project directory. Changes are debounced and emitted
    /// as `rustic:fs-change` Tauri events.
    ///
    /// `workspace_services` is the host-side registry of per-project
    /// `WorkspaceServices`. When supplied, the watcher callback also
    /// invalidates the tree-sitter cache and refreshes the symbol index
    /// for changed files — so an external edit (the IDE pane or `git pull`)
    /// keeps the agent's code-intel layer in sync without restart.
    pub fn watch_project(
        &mut self,
        project_path: &str,
        emitter: Arc<dyn EventEmitter>,
        workspace_services: Option<Arc<WorkspaceRegistry>>,
    ) {
        let norm = normalize(project_path);
        let key = watch_key(&norm);
        let restarted = match self.watchers.get(&key) {
            Some(e) if e.has_ui => return, // Already watching
            Some(_) => {
                // A file-history-only watcher exists; replace it so the UI's
                // own path string is the watch root (fs-change paths must
                // match what the frontend holds).
                self.watchers.remove(&key);
                true
            }
            None => false,
        };

        let project_path_owned = norm.clone();

        // Debounce state: collect changed parent dirs and flush periodically
        let pending: Arc<Mutex<PendingChanges>> = Arc::new(Mutex::new(PendingChanges::default()));

        let pending_clone = pending.clone();
        let emitter_clone = Arc::clone(&emitter);
        let project_for_timer = project_path_owned.clone();

        // Spawn a timer thread that flushes pending changes every 300ms
        let flush_running = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let flush_flag = flush_running.clone();
        let workspace_for_flush = workspace_services.clone();
        std::thread::spawn(move || {
            while flush_flag.load(std::sync::atomic::Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(300));
                let to_emit = {
                    let mut lock = pending_clone.lock_safe();
                    let ready = lock
                        .last_event
                        .is_some_and(|ts| ts.elapsed() >= Duration::from_millis(250))
                        && (!lock.dirs.is_empty()
                            || lock.git_changed
                            || lock.rescan
                            || !lock.index.is_empty());
                    if ready {
                        let dirs: Vec<String> = lock.dirs.keys().cloned().collect();
                        let paths: Vec<String> = lock.paths.keys().cloned().collect();
                        let git_changed = lock.git_changed;
                        let rescan = lock.rescan;
                        let index = std::mem::take(&mut lock.index);
                        *lock = PendingChanges::default();
                        Some((dirs, paths, git_changed, rescan, index))
                    } else {
                        None
                    }
                };
                if let Some((changed_dirs, changed_paths, git_changed, rescan, index)) = to_emit {
                    if !changed_dirs.is_empty() || git_changed || rescan {
                        emitter_clone.emit(
                            "rustic:fs-change",
                            FsChangeEvent {
                                project_path: project_for_timer.clone(),
                                changed_dirs,
                                changed_paths,
                                git_changed,
                                rescan,
                            },
                        );
                    }
                    if let Some(registry) = workspace_for_flush.as_ref() {
                        if !index.is_empty() {
                            let services = registry.get_or_create(Path::new(&project_for_timer));
                            for (p, removed) in index {
                                let path = Path::new(&p);
                                if removed {
                                    services.notify_file_deleted(path);
                                } else if path.is_file() {
                                    services.notify_file_changed(path);
                                }
                            }
                        }
                    }
                }
            }
        });

        let ui_handler: UiHandler = {
            let pending_for_handler = pending.clone();
            let workspace_for_handler = workspace_services.clone();
            let project_path_for_handler = project_path_owned.clone();
            Arc::new(move |res: &Result<Event, notify::Error>| {
                    let event = match res {
                        Ok(event) => event,
                        Err(e) => {
                            // Typically a Windows ReadDirectoryChangesW buffer
                            // overflow during a burst (venv / npm install /
                            // checkout): the whole batch is gone, so ask the
                            // frontend to rescan instead of silently going stale.
                            tracing::warn!(error = %e, "file watcher error; requesting rescan");
                            let mut lock = pending_for_handler.lock_safe();
                            lock.rescan = true;
                            lock.last_event = Some(Instant::now());
                            return;
                        }
                    };
                    if event.need_rescan() {
                        let mut lock = pending_for_handler.lock_safe();
                        lock.rescan = true;
                        lock.last_event = Some(Instant::now());
                        return;
                    }
                    {
                        // Only care about create/remove/modify/rename — skip access events
                        let is_remove = matches!(event.kind, EventKind::Remove(_));
                        match event.kind {
                            EventKind::Create(_) | EventKind::Remove(_) | EventKind::Modify(_) => {}
                            _ => return,
                        }

                        let mut lock = pending_for_handler.lock_safe();

                        for path in &event.paths {
                            // Use the parent directory as the key (that's what we refresh)
                            let parent = path
                                .parent()
                                .unwrap_or(path)
                                .to_string_lossy()
                                .replace('\\', "/");

                            let path_str = path.to_string_lossy().replace('\\', "/");

                            // Git metadata is filtered from the generic pipeline
                            // (WATCHER_SKIP_DIRS contains ".git") but staging,
                            // commits and checkouts done from a terminal move the
                            // repo state the SCM panel displays. Surface those as
                            // a `git_changed` flag instead of dir refreshes.
                            if let Some(rel) = git_relative_part(&path_str) {
                                if is_interesting_git_file(&rel) {
                                    lock.git_changed = true;
                                    lock.last_event = Some(Instant::now());
                                }
                                continue;
                            }

                            // Drop events INSIDE any WATCHER_SKIP_DIRS folder
                            // (node_modules, target, .venv, tmp, …). Without this, a
                            // Go `air` rebuild or `webpack` watch can fire hundreds
                            // of events per second from inside `tmp/`, `target/`
                            // or `node_modules/`. The folder ITSELF is still
                            // reported (its parent isn't skipped), so creating
                            // `.venv` or `build/` refreshes the tree (issue #12).
                            // The check runs on the path RELATIVE to the project
                            // root so a project that itself lives under a folder
                            // named `tmp`/`build`/`out` doesn't lose every event.
                            let rel_parent = parent
                                .strip_prefix(&project_path_for_handler)
                                .unwrap_or(&parent);
                            if path_has_skip_segment(rel_parent) {
                                continue;
                            }

                            lock.dirs.insert(parent, ());
                            if lock.paths.len() < CHANGED_PATHS_CAP {
                                lock.paths.insert(path_str.clone(), ());
                            }

                            // P1.2: keep the agent's workspace symbol index in
                            // sync with external file changes. Only QUEUE the
                            // work here; the flush thread applies it outside the
                            // lock so the notify thread keeps draining OS events.
                            if workspace_for_handler.is_some() && lock.index.len() < INDEX_QUEUE_CAP
                            {
                                lock.index.insert(path_str.clone(), is_remove);
                            }
                        }

                        lock.last_event = Some(Instant::now());
                    }
            })
        };

        let feeds = self.feeds_for(&key);
        let entry = Self::start_entry(
            &norm,
            native_path(&project_path_owned),
            feeds.clone(),
            Some(ui_handler),
            flush_running,
        );
        self.watchers.insert(key, entry);
        if restarted {
            signal_feeds_lost(&feeds, &native_path(&norm), "watcher restarted");
        }
    }

    /// Start the single OS watcher for one project, fanning raw events out to `feeds` before the optional UI handler.
    fn start_entry(
        label: &str,
        path: PathBuf,
        feeds: FeedList,
        ui: Option<UiHandler>,
        alive: Arc<std::sync::atomic::AtomicBool>,
    ) -> WatcherEntry {
        let has_ui = ui.is_some();
        // Handler factory: a fresh notify callback per start attempt, so a
        // failed start can be retried later (issue #12, cause D).
        let make_handler = {
            let feeds = feeds.clone();
            let root = path.clone();
            move || {
                let feeds = feeds.clone();
                let ui = ui.clone();
                let root = root.clone();
                move |res: Result<Event, notify::Error>| {
                    {
                        let mut list = feeds.lock_safe();
                        if !list.is_empty() {
                            list.retain(|f| f.feed(&root, &res));
                        }
                    }
                    if let Some(ui) = ui.as_ref() {
                        ui(&res);
                    }
                }
            }
        };

        match start_watcher(make_handler(), &path) {
            Ok(watcher) => WatcherEntry {
                _watcher: Some(watcher),
                flush_running: alive,
                has_ui,
            },
            Err(e) => {
                // Previously the project silently stayed unwatched until a
                // restart (e.g. root not ready yet after a cloud-sync
                // re-watch). Keep a placeholder entry and retry in the
                // background; the retry thread owns the watcher once it
                // starts and drops it when the entry is removed.
                tracing::warn!(
                    "[watcher] Failed to watch {}: {} — retrying in background",
                    label,
                    e
                );
                signal_feeds_lost(&feeds, &path, "watcher failed to start");
                let alive_retry = alive.clone();
                let project_for_retry = label.to_string();
                std::thread::spawn(move || {
                    use std::sync::atomic::Ordering;
                    let mut held: Option<RecommendedWatcher> = None;
                    for attempt in 1..=WATCH_RETRY_ATTEMPTS {
                        std::thread::sleep(WATCH_RETRY_INTERVAL);
                        if !alive_retry.load(Ordering::Relaxed) {
                            return;
                        }
                        match start_watcher(make_handler(), &path) {
                            Ok(w) => {
                                tracing::info!(
                                    attempt,
                                    "[watcher] started watching {} after retry",
                                    project_for_retry
                                );
                                held = Some(w);
                                signal_feeds_lost(&feeds, &path, "watcher started after retry");
                                break;
                            }
                            Err(e) => {
                                tracing::debug!(attempt, error = %e, "[watcher] retry failed for {}", project_for_retry);
                            }
                        }
                    }
                    if held.is_none() {
                        tracing::warn!(
                            "[watcher] giving up on {} after {} retries",
                            project_for_retry,
                            WATCH_RETRY_ATTEMPTS
                        );
                        return;
                    }
                    while alive_retry.load(Ordering::Relaxed) {
                        std::thread::sleep(Duration::from_millis(500));
                    }
                    drop(held);
                });
                WatcherEntry {
                    _watcher: None,
                    flush_running: alive,
                    has_ui,
                }
            }
        }
    }

    /// Stop watching a project directory.
    pub fn unwatch_project(&mut self, project_path: &str) {
        let key = watch_key(&normalize(project_path));
        // Dropping the WatcherEntry drops the watcher, stopping it
        self.watchers.remove(&key);
        // File history may still be live for this project; re-arm a fresh
        // watcher for it (cloud sync unwatches to re-establish after a folder
        // swap, so a new OS handle is the desired outcome either way).
        let Some(subs) = self.subscribers.get(&key) else {
            return;
        };
        let live = {
            let mut list = subs.feeds.lock_safe();
            list.retain(|f| f.is_alive());
            !list.is_empty()
        };
        if !live {
            self.subscribers.remove(&key);
            return;
        }
        let root = subs.root.clone();
        let feeds = subs.feeds.clone();
        let label = normalize(&root.to_string_lossy());
        let entry = Self::start_entry(
            &label,
            root.clone(),
            feeds.clone(),
            None,
            Arc::new(std::sync::atomic::AtomicBool::new(true)),
        );
        self.watchers.insert(key, entry);
        signal_feeds_lost(&feeds, &root, "watcher restarted");
    }
}

impl Default for FileWatcherManager {
    fn default() -> Self {
        Self::new()
    }
}

fn normalize(p: &str) -> String {
    p.replace('\\', "/")
}

/// Drop a Windows verbatim prefix (`//?/C:/x` → `C:/x`, `//?/UNC/s/x` → `//s/x`) from a normalized path.
fn strip_verbatim(norm: &str) -> std::borrow::Cow<'_, str> {
    if let Some(rest) = norm.strip_prefix("//?/UNC/") {
        std::borrow::Cow::Owned(format!("//{rest}"))
    } else if let Some(rest) = norm.strip_prefix("//?/") {
        std::borrow::Cow::Borrowed(rest)
    } else {
        std::borrow::Cow::Borrowed(norm)
    }
}

/// Map key identifying a project root regardless of separator, verbatim prefix, trailing slash, or (Windows) case.
fn watch_key(norm: &str) -> String {
    let s = strip_verbatim(norm);
    let s = s.trim_end_matches('/');
    #[cfg(windows)]
    {
        s.to_lowercase()
    }
    #[cfg(not(windows))]
    {
        s.to_string()
    }
}

/// Native watch path from a `/`-normalized project path.
fn native_path(norm: &str) -> PathBuf {
    // Windows notify wants native separators; POSIX paths must keep their
    // forward slashes — converting them there produced a bogus
    // `\home\user\project` path, so `watch()` always failed and the server
    // build never emitted a single fs-change event.
    #[cfg(windows)]
    {
        PathBuf::from(norm.replace('/', "\\"))
    }
    #[cfg(not(windows))]
    {
        PathBuf::from(norm)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct RecordingEmitter(Mutex<Vec<serde_json::Value>>);

    impl EventEmitter for RecordingEmitter {
        fn emit_json(&self, event: &str, payload: serde_json::Value) {
            if event == "rustic:fs-change" {
                self.0.lock_safe().push(payload);
            }
        }
    }

    #[test]
    fn watch_key_ignores_verbatim_separator_and_trailing_slash() {
        assert_eq!(watch_key("//?/C:/proj/"), watch_key("C:/proj"));
        assert_eq!(watch_key(&normalize("\\\\?\\UNC\\srv\\share")), watch_key("//srv/share"));
    }

    #[test]
    fn one_os_watcher_feeds_ui_and_file_history() {
        let dir = std::env::temp_dir().join(format!(
            "rustic-watch-fanout-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let root = dir.canonicalize().unwrap();

        let acc = Arc::new(rustic_agent::DirtyPathAccumulator::new(root.clone()));
        acc.set_active("t", "m");
        let (fh_watcher, feed) = rustic_agent::FileWatcher::attach(root.clone(), Arc::clone(&acc)).unwrap();

        let mut mgr = FileWatcherManager::new();
        // File history first: must work before the UI watcher exists.
        mgr.subscribe_file_history(&root, feed);
        assert_eq!(mgr.watchers.len(), 1);

        let emitter = Arc::new(RecordingEmitter(Mutex::new(Vec::new())));
        let ui_path = strip_verbatim(&normalize(&root.to_string_lossy())).to_string();
        mgr.watch_project(&ui_path, emitter.clone(), None);
        assert_eq!(mgr.watchers.len(), 1, "UI must reuse the project's single OS watcher");
        assert!(mgr.watchers.values().all(|e| e.has_ui));

        // The UI re-arm sends a rescan to file history; drain it so the
        // assertion below proves real events arrive.
        std::thread::sleep(Duration::from_millis(400));
        acc.drain("t", "m");

        std::fs::write(root.join("hello.txt"), b"hi").unwrap();

        let deadline = Instant::now() + Duration::from_secs(10);
        let (mut ui_seen, mut fh_seen) = (false, false);
        while Instant::now() < deadline && !(ui_seen && fh_seen) {
            std::thread::sleep(Duration::from_millis(100));
            ui_seen = emitter.0.lock_safe().iter().any(|p| {
                p["changed_paths"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|v| v.as_str().is_some_and(|s| s.ends_with("/hello.txt"))))
            });
            fh_seen = acc
                .peek_active()
                .is_some_and(|s| s.modified.contains("hello.txt"));
        }
        assert!(ui_seen, "fs-change batch should report hello.txt");
        assert!(fh_seen, "file-history accumulator should record hello.txt");

        drop(fh_watcher);
        mgr.unwatch_project(&ui_path);
        assert!(mgr.watchers.is_empty(), "dropped file-history consumer must not re-arm a watcher");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
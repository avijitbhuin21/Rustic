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
}

/// Debounce accumulator shared between the notify callback and the flush
/// thread.
#[derive(Default)]
struct PendingChanges {
    dirs: HashMap<String, ()>,
    paths: HashMap<String, ()>,
    git_changed: bool,
    last_event: Option<Instant>,
}

struct WatcherEntry {
    _watcher: RecommendedWatcher,
    flush_running: Arc<std::sync::atomic::AtomicBool>,
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

pub struct FileWatcherManager {
    watchers: HashMap<String, WatcherEntry>,
}

impl FileWatcherManager {
    pub fn new() -> Self {
        Self {
            watchers: HashMap::new(),
        }
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
        if self.watchers.contains_key(&norm) {
            return; // Already watching
        }

        let project_path_owned = norm.clone();

        // Debounce state: collect changed parent dirs and flush periodically
        let pending: Arc<Mutex<PendingChanges>> = Arc::new(Mutex::new(PendingChanges::default()));

        let pending_clone = pending.clone();
        let emitter_clone = Arc::clone(&emitter);
        let project_for_timer = project_path_owned.clone();

        // Spawn a timer thread that flushes pending changes every 300ms
        let flush_running = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let flush_flag = flush_running.clone();
        std::thread::spawn(move || {
            while flush_flag.load(std::sync::atomic::Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(300));
                let to_emit = {
                    let mut lock = pending_clone.lock_safe();
                    let ready = lock
                        .last_event
                        .is_some_and(|ts| ts.elapsed() >= Duration::from_millis(250))
                        && (!lock.dirs.is_empty() || lock.git_changed);
                    if ready {
                        let dirs: Vec<String> = lock.dirs.keys().cloned().collect();
                        let paths: Vec<String> = lock.paths.keys().cloned().collect();
                        let git_changed = lock.git_changed;
                        *lock = PendingChanges::default();
                        Some((dirs, paths, git_changed))
                    } else {
                        None
                    }
                };
                if let Some((changed_dirs, changed_paths, git_changed)) = to_emit {
                    emitter_clone.emit(
                        "rustic:fs-change",
                        FsChangeEvent {
                            project_path: project_for_timer.clone(),
                            changed_dirs,
                            changed_paths,
                            git_changed,
                        },
                    );
                }
            }
        });

        let pending_for_handler = pending.clone();
        let workspace_for_handler = workspace_services.clone();
        let project_path_for_handler = project_path_owned.clone();

        let mut watcher = match RecommendedWatcher::new(
            move |res: Result<Event, notify::Error>| {
                if let Ok(event) = res {
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

                        // Drop events whose path or parent dir contains any
                        // of WATCHER_SKIP_DIRS (node_modules, target, build
                        // artifacts, tmp, etc.). Without this, a Go `air`
                        // rebuild or `webpack` watch can fire hundreds of
                        // events per second from inside `tmp/`, `target/`
                        // or `node_modules/`, each one waking up the
                        // frontend's file-tree refresh and ballooning memory.
                        // The check runs on the path RELATIVE to the project
                        // root so a project that itself lives under a folder
                        // named `tmp`/`build`/`out` doesn't lose every event.
                        let rel_path = path_str
                            .strip_prefix(&project_path_for_handler)
                            .unwrap_or(&path_str);
                        let rel_parent = parent
                            .strip_prefix(&project_path_for_handler)
                            .unwrap_or(&parent);
                        if path_has_skip_segment(rel_parent) || path_has_skip_segment(rel_path) {
                            continue;
                        }

                        lock.dirs.insert(parent, ());
                        if lock.paths.len() < CHANGED_PATHS_CAP {
                            lock.paths.insert(path_str.clone(), ());
                        }

                        // P1.2: keep the agent's workspace symbol index in
                        // sync with external file changes. Look up the
                        // services for this project root and refresh the
                        // file's index entries (or drop them on delete).
                        if let Some(registry) = workspace_for_handler.as_ref() {
                            let services =
                                registry.get_or_create(Path::new(&project_path_for_handler));
                            if is_remove {
                                services.notify_file_deleted(path);
                            } else if path.is_file() {
                                services.notify_file_changed(path);
                            }
                        }
                    }

                    lock.last_event = Some(Instant::now());
                }
            },
            Config::default(),
        ) {
            Ok(w) => w,
            Err(e) => {
                tracing::warn!("[watcher] Failed to create watcher for {}: {}", norm, e);
                return;
            }
        };

        let path = PathBuf::from(&project_path_owned.replace('/', "\\"));
        if let Err(e) = watcher.watch(&path, RecursiveMode::Recursive) {
            tracing::warn!("[watcher] Failed to watch {}: {}", project_path_owned, e);
            return;
        }

        self.watchers.insert(
            norm,
            WatcherEntry {
                _watcher: watcher,
                flush_running,
            },
        );
    }

    /// Stop watching a project directory.
    pub fn unwatch_project(&mut self, project_path: &str) {
        let norm = normalize(project_path);
        // Dropping the WatcherEntry drops the watcher, stopping it
        self.watchers.remove(&norm);
    }
}

fn normalize(p: &str) -> String {
    p.replace('\\', "/")
}

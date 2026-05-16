use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher, Event, EventKind};
use rustic_agent::WorkspaceRegistry;
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

/// Directory segments we drop from watcher events without ever forwarding to
/// the frontend. `notify` on Windows uses `ReadDirectoryChangesW` recursively
/// on the project root, so when a tool like `air` rebuilds a Go binary or
/// `webpack` writes into `dist/`, hundreds of write events can fire per
/// second — every one of them previously triggered a full `read_dir` refresh
/// in the frontend, spiking memory and locking up the UI on big projects.
///
/// Set is hand-curated rather than reused from another module — the legacy
/// `SNAPSHOT_SKIP_DIRS` list it used to mirror was deleted along with the
/// file-mirror snapshot system, but the watcher's noise reduction has the
/// same shape: build artifacts and tooling caches we never want to forward.
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

/// Payload emitted to the frontend when the file system changes.
#[derive(Clone, Serialize)]
pub struct FsChangeEvent {
    /// The project root this change belongs to.
    pub project_path: String,
    /// The specific paths that changed (parent directories, deduplicated).
    pub changed_dirs: Vec<String>,
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
        app: AppHandle,
        workspace_services: Option<Arc<WorkspaceRegistry>>,
    ) {
        let norm = normalize(project_path);
        if self.watchers.contains_key(&norm) {
            return; // Already watching
        }

        let project_path_owned = norm.clone();

        // Debounce state: collect changed parent dirs and flush periodically
        let pending: Arc<Mutex<(HashMap<String, ()>, Option<Instant>)>> =
            Arc::new(Mutex::new((HashMap::new(), None)));

        let pending_clone = pending.clone();
        let app_clone = app.clone();
        let project_for_timer = project_path_owned.clone();

        // Spawn a timer thread that flushes pending changes every 300ms
        let flush_running = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let flush_flag = flush_running.clone();
        std::thread::spawn(move || {
            while flush_flag.load(std::sync::atomic::Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(300));
                let dirs_to_emit = {
                    let mut lock = pending_clone.lock().unwrap();
                    let (ref mut dirs, ref mut last_event) = *lock;
                    if let Some(ts) = last_event {
                        if ts.elapsed() >= Duration::from_millis(250) && !dirs.is_empty() {
                            let collected: Vec<String> = dirs.keys().cloned().collect();
                            dirs.clear();
                            *last_event = None;
                            Some(collected)
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                };
                if let Some(changed_dirs) = dirs_to_emit {
                    let _ = app_clone.emit(
                        "rustic:fs-change",
                        FsChangeEvent {
                            project_path: project_for_timer.clone(),
                            changed_dirs,
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
                        EventKind::Create(_)
                        | EventKind::Remove(_)
                        | EventKind::Modify(_) => {}
                        _ => return,
                    }

                    let mut lock = pending_for_handler.lock().unwrap();
                    let (ref mut dirs, ref mut last_event) = *lock;

                    for path in &event.paths {
                        // Use the parent directory as the key (that's what we refresh)
                        let parent = path
                            .parent()
                            .unwrap_or(path)
                            .to_string_lossy()
                            .replace('\\', "/");

                        // Drop events whose path or parent dir contains any
                        // of WATCHER_SKIP_DIRS (.git, node_modules, target,
                        // build artifacts, tmp, etc.). Without this, a Go
                        // `air` rebuild or `webpack` watch can fire hundreds
                        // of events per second from inside `tmp/`, `target/`
                        // or `node_modules/`, each one waking up the
                        // frontend's file-tree refresh and ballooning memory.
                        let path_str = path.to_string_lossy();
                        if path_has_skip_segment(&parent) || path_has_skip_segment(&path_str) {
                            continue;
                        }

                        dirs.insert(parent, ());

                        // P1.2: keep the agent's workspace symbol index in
                        // sync with external file changes. Look up the
                        // services for this project root and refresh the
                        // file's index entries (or drop them on delete).
                        if let Some(registry) = workspace_for_handler.as_ref() {
                            let services = registry.get_or_create(Path::new(&project_path_for_handler));
                            if is_remove {
                                services.notify_file_deleted(path);
                            } else if path.is_file() {
                                services.notify_file_changed(path);
                            }
                        }
                    }

                    *last_event = Some(Instant::now());
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

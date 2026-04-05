use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher, Event, EventKind};
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

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
    pub fn watch_project(&mut self, project_path: &str, app: AppHandle) {
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

        let mut watcher = match RecommendedWatcher::new(
            move |res: Result<Event, notify::Error>| {
                if let Ok(event) = res {
                    // Only care about create/remove/modify/rename — skip access events
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

                        // Skip .git directory changes and .rustic directory
                        if parent.contains("/.git/")
                            || parent.ends_with("/.git")
                            || parent.contains("/.rustic/")
                            || parent.ends_with("/.rustic")
                            || path.to_string_lossy().contains("/.git/")
                            || path.to_string_lossy().contains("\\.git\\")
                            || path.to_string_lossy().contains("/.rustic/")
                            || path.to_string_lossy().contains("\\.rustic\\")
                        {
                            continue;
                        }

                        dirs.insert(parent, ());
                    }

                    *last_event = Some(Instant::now());
                }
            },
            Config::default(),
        ) {
            Ok(w) => w,
            Err(e) => {
                eprintln!("[watcher] Failed to create watcher for {}: {}", norm, e);
                return;
            }
        };

        let path = PathBuf::from(&project_path_owned.replace('/', "\\"));
        if let Err(e) = watcher.watch(&path, RecursiveMode::Recursive) {
            eprintln!("[watcher] Failed to watch {}: {}", project_path_owned, e);
            return;
        }

        self.watchers.insert(
            norm,
            WatcherEntry {
                _watcher: watcher,
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

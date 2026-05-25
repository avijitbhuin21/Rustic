//! Dirty-path accumulator for the FS-watcher path.
//!
//! Receives filesystem events from `watcher.rs` and routes them to the
//! currently-active `(task_id, message_id)` so the sweep worker can drain
//! just the paths that actually changed since the last snapshot, instead of
//! re-walking the entire worktree. Design rationale in
//! [docs/educated-guesses/005-notify-integration-design.md](../../../../docs/educated-guesses/005-notify-integration-design.md).
//!
//! Two concepts:
//! - **Active key** — the `(task_id, message_id)` that incoming events get
//!   attributed to. Set by `tracker.open_snapshot`; cleared on project
//!   teardown. Only one key is active at a time per project.
//! - **Dirty set** — per-key collection of `(modified, removed, lost)`.
//!   Drained by the sweep worker when a `SweepJob` for that key fires.
//!
//! `lost` is sticky once set: the next sweep falls back to a full walk
//! regardless of how many specific paths the dirty set holds. This is the
//! correctness backstop for kernel-buffer overflow under heavy write load
//! (see doc 005 §"Lost-event handling").

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// One key's dirty state. Modified vs removed are tracked separately so
/// `shadow.track_paths` can do the right thing (re-hash vs drop-from-tree).
#[derive(Debug, Default, Clone)]
pub struct DirtySet {
    pub modified: HashSet<String>,
    pub removed: HashSet<String>,
    /// Set when the watcher reports any error (kernel overflow, watch
    /// registration failure, polling fallback skipped a tick). Once true,
    /// the sweep treats this key's tree as untrusted and falls back to a
    /// full `shadow.track()`.
    pub lost: bool,
}

impl DirtySet {
    /// True when the accumulator holds nothing of interest to the sweep:
    /// no paths AND no lost-event flag.
    pub fn is_empty(&self) -> bool {
        !self.lost && self.modified.is_empty() && self.removed.is_empty()
    }
}

/// Active routing key. Owning by value (no lifetimes) keeps the accumulator
/// `Send + Sync` without lifetime gymnastics.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ActiveKey {
    task_id: String,
    message_id: String,
}

#[derive(Debug, Default)]
struct Inner {
    /// `(task_id, message_id)` events should currently be attributed to.
    /// `None` between `set_active` and project startup, or after `clear_active`.
    active: Option<ActiveKey>,

    /// Per-key dirty sets. Keep historical entries around until the sweep
    /// explicitly drains them — that's how the watcher can record events
    /// that arrive *after* a sweep fires for a previous key.
    sets: HashMap<ActiveKey, DirtySet>,
}

/// Cheap to clone (Arc-internal). Safe to share across threads — the
/// internal mutex serialises all mutations.
#[derive(Debug, Default)]
pub struct DirtyPathAccumulator {
    inner: Mutex<Inner>,
    /// Worktree root used to translate watcher events (absolute paths) into
    /// the forward-slash relative form that shadow trees use as keys.
    project_root: PathBuf,
}

impl DirtyPathAccumulator {
    pub fn new(project_root: PathBuf) -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
            project_root,
        }
    }

    /// Update which `(task_id, message_id)` future events attribute to.
    /// Called from `tracker.open_snapshot` so the routing follows the same
    /// boundary the snapshot model already uses.
    pub fn set_active(&self, task_id: &str, message_id: &str) {
        let mut inner = match self.inner.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(), // mutex poisoned by a panicking thread; recover
        };
        inner.active = Some(ActiveKey {
            task_id: task_id.to_string(),
            message_id: message_id.to_string(),
        });
    }

    /// Forget the active key — events arriving after this land nowhere
    /// (silently dropped). Used on project teardown.
    pub fn clear_active(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.active = None;
        }
    }

    /// Record that `abs_path` was created or modified. Path is converted to
    /// the project-relative forward-slash form; events outside the project
    /// root are silently dropped.
    pub fn record_modified(&self, abs_path: &Path) {
        let Some(rel) = self.to_rel(abs_path) else {
            return;
        };
        self.with_active(|set| {
            set.removed.remove(&rel);
            set.modified.insert(rel);
        });
    }

    /// Record that `abs_path` was removed.
    pub fn record_removed(&self, abs_path: &Path) {
        let Some(rel) = self.to_rel(abs_path) else {
            return;
        };
        self.with_active(|set| {
            set.modified.remove(&rel);
            set.removed.insert(rel);
        });
    }

    /// Record a rename: `from` removed, `to` modified. If either side is
    /// outside the project root, the half that's inside is recorded as the
    /// appropriate single-sided event.
    pub fn record_rename(&self, from: &Path, to: &Path) {
        let from_rel = self.to_rel(from);
        let to_rel = self.to_rel(to);
        self.with_active(|set| {
            if let Some(f) = from_rel {
                set.modified.remove(&f);
                set.removed.insert(f);
            }
            if let Some(t) = to_rel {
                set.removed.remove(&t);
                set.modified.insert(t);
            }
        });
    }

    /// Flag the active key as having dropped events. Sticky until drained.
    pub fn mark_lost(&self) {
        self.with_active(|set| set.lost = true);
    }

    /// Atomically take the dirty set for `(task_id, message_id)` and reset
    /// it to empty. Called by the sweep worker before deciding between
    /// targeted (`shadow.track_paths`) and full (`shadow.track`) tracking.
    pub fn drain(&self, task_id: &str, message_id: &str) -> DirtySet {
        let key = ActiveKey {
            task_id: task_id.to_string(),
            message_id: message_id.to_string(),
        };
        let mut inner = match self.inner.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        inner.sets.remove(&key).unwrap_or_default()
    }

    /// Peek without draining — useful for tests / diagnostics. Returns a
    /// clone of the current dirty set for the active key, or `None` if
    /// nothing is active.
    pub fn peek_active(&self) -> Option<DirtySet> {
        let inner = self.inner.lock().ok()?;
        let active = inner.active.clone()?;
        Some(inner.sets.get(&active).cloned().unwrap_or_default())
    }

    /// Take the dirty set for the currently active key, if any. Used when
    /// the sweep wants to flush whatever is active without having to know
    /// the key out-of-band.
    pub fn drain_active(&self) -> Option<(String, String, DirtySet)> {
        let mut inner = match self.inner.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        let active = inner.active.clone()?;
        let set = inner.sets.remove(&active).unwrap_or_default();
        Some((active.task_id, active.message_id, set))
    }

    // ---------- internal helpers ----------

    fn with_active(&self, f: impl FnOnce(&mut DirtySet)) {
        let mut inner = match self.inner.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        if let Some(key) = inner.active.clone() {
            let set = inner.sets.entry(key).or_default();
            f(set);
        }
        // No active key → drop the event silently. The next sweep that
        // follows an `open_snapshot` will do a full walk anyway, so we lose
        // no correctness — only the watcher acceleration.
    }

    /// Convert an absolute event path into the project-relative
    /// forward-slash form the shadow store uses. Returns `None` for paths
    /// outside the project root.
    fn to_rel(&self, abs_path: &Path) -> Option<String> {
        let stripped = abs_path.strip_prefix(&self.project_root).ok()?;
        let s = stripped.to_string_lossy().replace('\\', "/");
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make(project_root: &str) -> DirtyPathAccumulator {
        DirtyPathAccumulator::new(PathBuf::from(project_root))
    }

    #[cfg(unix)]
    const ROOT: &str = "/tmp/proj";
    #[cfg(windows)]
    const ROOT: &str = "C:\\tmp\\proj";

    fn rel(p: &str) -> PathBuf {
        let mut path = PathBuf::from(ROOT);
        for part in p.split('/') {
            path.push(part);
        }
        path
    }

    #[test]
    fn record_modified_after_set_active_lands_in_dirty_set() {
        let acc = make(ROOT);
        acc.set_active("t1", "m1");
        acc.record_modified(&rel("src/foo.rs"));

        let set = acc.drain("t1", "m1");
        assert!(set.modified.contains("src/foo.rs"));
        assert!(set.removed.is_empty());
        assert!(!set.lost);
    }

    #[test]
    fn record_without_active_key_is_dropped() {
        let acc = make(ROOT);
        // No set_active call — events should be silently dropped.
        acc.record_modified(&rel("src/foo.rs"));
        let set = acc.drain("t1", "m1");
        assert!(set.is_empty());
    }

    #[test]
    fn modified_then_removed_lands_in_removed_only() {
        let acc = make(ROOT);
        acc.set_active("t", "m");
        acc.record_modified(&rel("file.rs"));
        acc.record_removed(&rel("file.rs"));
        let set = acc.drain("t", "m");
        assert!(!set.modified.contains("file.rs"));
        assert!(set.removed.contains("file.rs"));
    }

    #[test]
    fn removed_then_modified_lands_in_modified_only() {
        let acc = make(ROOT);
        acc.set_active("t", "m");
        acc.record_removed(&rel("file.rs"));
        acc.record_modified(&rel("file.rs"));
        let set = acc.drain("t", "m");
        assert!(set.modified.contains("file.rs"));
        assert!(!set.removed.contains("file.rs"));
    }

    #[test]
    fn rename_records_removal_of_from_and_modification_of_to() {
        let acc = make(ROOT);
        acc.set_active("t", "m");
        acc.record_rename(&rel("old.rs"), &rel("new.rs"));
        let set = acc.drain("t", "m");
        assert!(set.removed.contains("old.rs"));
        assert!(set.modified.contains("new.rs"));
    }

    #[test]
    fn paths_outside_project_root_dropped() {
        let acc = make(ROOT);
        acc.set_active("t", "m");
        #[cfg(unix)]
        let outside = PathBuf::from("/etc/passwd");
        #[cfg(windows)]
        let outside = PathBuf::from("C:\\Windows\\System32\\config");
        acc.record_modified(&outside);
        let set = acc.drain("t", "m");
        assert!(set.is_empty());
    }

    #[test]
    fn mark_lost_persists_until_drained() {
        let acc = make(ROOT);
        acc.set_active("t", "m");
        acc.record_modified(&rel("a.rs"));
        acc.mark_lost();
        acc.record_modified(&rel("b.rs"));

        let set = acc.drain("t", "m");
        assert!(set.lost);
        assert!(set.modified.contains("a.rs"));
        assert!(set.modified.contains("b.rs"));

        // Subsequent drain returns a fresh empty set, no longer lost.
        let set2 = acc.drain("t", "m");
        assert!(set2.is_empty());
    }

    #[test]
    fn drain_returns_empty_for_unknown_key() {
        let acc = make(ROOT);
        let set = acc.drain("nonexistent", "msg");
        assert!(set.is_empty());
    }

    #[test]
    fn switching_active_key_keeps_old_set_intact() {
        let acc = make(ROOT);
        acc.set_active("t1", "m1");
        acc.record_modified(&rel("a.rs"));

        acc.set_active("t1", "m2");
        acc.record_modified(&rel("b.rs"));

        // Drain m1 first — should only have a.rs
        let s1 = acc.drain("t1", "m1");
        assert!(s1.modified.contains("a.rs"));
        assert!(!s1.modified.contains("b.rs"));

        // Drain m2 — only b.rs
        let s2 = acc.drain("t1", "m2");
        assert!(s2.modified.contains("b.rs"));
        assert!(!s2.modified.contains("a.rs"));
    }

    #[test]
    fn drain_active_drains_currently_active_key() {
        let acc = make(ROOT);
        acc.set_active("task-X", "msg-Y");
        acc.record_modified(&rel("hello.rs"));

        let (task, msg, set) = acc.drain_active().expect("active key present");
        assert_eq!(task, "task-X");
        assert_eq!(msg, "msg-Y");
        assert!(set.modified.contains("hello.rs"));
    }

    #[test]
    fn clear_active_then_record_drops_silently() {
        let acc = make(ROOT);
        acc.set_active("t", "m");
        acc.record_modified(&rel("first.rs"));
        acc.clear_active();
        acc.record_modified(&rel("second.rs"));

        let set = acc.drain("t", "m");
        assert!(set.modified.contains("first.rs"));
        assert!(!set.modified.contains("second.rs"));
    }
}

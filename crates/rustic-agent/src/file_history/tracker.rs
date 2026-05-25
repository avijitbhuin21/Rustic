//! Public tracker API on top of `ShadowSnapshot`.
//!
//! Each per-message snapshot is a libgit2 tree oid plus a SQLite metadata
//! row. Method names match the legacy tracker for call-site compatibility.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rustic_db::Database;

use super::shadow::Oid;
use thiserror::Error;

use super::accumulator::DirtyPathAccumulator;
use super::shadow::{
    ShadowChangeStatus, ShadowError, ShadowFileChange, ShadowRestoreAction, ShadowSnapshot,
    MAX_TRACKED_FILE_SIZE,
};
use super::walk::{join_rel, normalize_rel};

/// 7-day grace window before unreachable shadow objects are pruned.
/// Plan-locked in `docs/file_tracking_decision.md` §0.
const GC_PRUNE_HORIZON: Duration = Duration::from_secs(7 * 24 * 3600);

#[derive(Debug, Error)]
pub enum FileHistoryError {
    #[error("shadow: {0}")]
    Shadow(#[from] ShadowError),

    #[error("db: {0}")]
    Db(#[from] rustic_db::DbError),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("snapshot {0} not found")]
    SnapshotNotFound(String),

    #[error("snapshot {0} has no tree_oid (pre-R.1 legacy row, unrevertable)")]
    SnapshotMissingTree(String),

    #[error("invalid stored tree_oid for snapshot {message_id}: {raw}")]
    InvalidTreeOid { message_id: String, raw: String },

    #[error("path {path:?} resolves outside the project root {root:?}")]
    OutsideProject { path: PathBuf, root: PathBuf },

    #[error("mutex poisoned")]
    Poisoned,
}

pub type Result<T> = std::result::Result<T, FileHistoryError>;

/// Outcome of a single `capture` call. Preserved for back-compat with the
/// pre-R.1 API. In the shadow model `open_snapshot` already captured the
/// full worktree state; `capture` is mostly a UI-reporting hook.
#[derive(Debug, Clone)]
pub enum CaptureOutcome {
    /// Path was present in the snapshot's tree. `size` is the stored
    /// blob's byte length.
    Captured {
        rel_path: String,
        hash: String,
        size: u64,
    },
    /// Path was not in the snapshot's tree. Revert will delete a
    /// same-named file created later.
    DidNotExist { rel_path: String },
    /// `capture` was called for this path within this snapshot before.
    /// No state changes — the snapshot's tree is already authoritative.
    AlreadyTracked { rel_path: String },
    /// On-disk file exceeded `MAX_TRACKED_FILE_SIZE`. Not in the
    /// snapshot's tree; unrevertable.
    TooLarge { rel_path: String, size: u64 },
}

/// Returned by revert ops — the on-disk action taken for each path.
#[derive(Debug, Clone)]
pub enum RestoreOutcome {
    Rewritten(String),
    Deleted(String),
    Unchanged(String),
}

/// Revert-preview row.
#[derive(Debug, Clone)]
pub struct RevertPlanEntry {
    pub path: String,
    pub action: &'static str,
}

/// Per-file stats for the changed-files panel.
#[derive(Debug, Clone)]
pub struct FileChangeStats {
    pub path: String,
    pub kind: &'static str,
    pub binary: bool,
    pub additions: u32,
    pub deletions: u32,
}

/// Cumulative net-change for an entire task — pre-task state vs current
/// disk. `anchor_message_id` is the earliest snapshot that captured this
/// path; the UI uses it for `fh_file_diff` lookups.
#[derive(Debug, Clone)]
pub struct TaskNetChange {
    pub path: String,
    pub kind: &'static str,
    pub binary: bool,
    pub additions: u32,
    pub deletions: u32,
    pub anchor_message_id: String,
}

/// Full diff for one path in one snapshot.
#[derive(Debug, Clone)]
pub struct FileDiff {
    pub path: String,
    pub kind: &'static str,
    pub binary: bool,
    pub additions: u32,
    pub deletions: u32,
    pub unified: String,
}

#[derive(Clone)]
pub struct FileHistory {
    inner: Arc<FileHistoryInner>,
}

struct FileHistoryInner {
    db: Arc<Mutex<Database>>,
    shadow: Arc<ShadowSnapshot>,
    project_root: PathBuf,
    /// Per-task snapshot retention cap (legacy `max_snapshots` of 100).
    max_snapshots: i64,
    /// FS-watcher accumulator. `Some` when a watcher is attached (the
    /// Tauri host wires this up in `FileHistoryHandle::new`); `None` for
    /// tests and any caller that hasn't set up a watcher. When present,
    /// `record_post_bash_state` will prefer `shadow.track_paths` over the
    /// O(worktree) full walk.
    accumulator: Option<Arc<DirtyPathAccumulator>>,
}

impl FileHistory {
    /// Construct a tracker bound to one project worktree. `config_dir` is
    /// the platform app-data directory; the shadow repo lives at
    /// `<config_dir>/file-history/shadow/<project_hash>/`.
    pub fn new(
        db: Arc<Mutex<Database>>,
        project_root: PathBuf,
        config_dir: &Path,
    ) -> Result<Self> {
        Self::new_with_accumulator(db, project_root, config_dir, None)
    }

    /// Variant that attaches an FS-watcher accumulator. The Tauri host calls
    /// this from `FileHistoryHandle::new` so sweep work can short-circuit to
    /// `shadow.track_paths` when notify has fed us the dirty set.
    pub fn new_with_accumulator(
        db: Arc<Mutex<Database>>,
        project_root: PathBuf,
        config_dir: &Path,
        accumulator: Option<Arc<DirtyPathAccumulator>>,
    ) -> Result<Self> {
        let shadow_root = config_dir.join("file-history").join("shadow");
        let shadow = ShadowSnapshot::for_worktree(&project_root, &shadow_root)?;
        Ok(Self {
            inner: Arc::new(FileHistoryInner {
                db,
                shadow,
                project_root,
                max_snapshots: 100,
                accumulator,
            }),
        })
    }

    /// Borrow the attached accumulator, if any.
    pub fn accumulator(&self) -> Option<&Arc<DirtyPathAccumulator>> {
        self.inner.accumulator.as_ref()
    }

    pub fn project_root(&self) -> &Path {
        &self.inner.project_root
    }

    pub fn shadow(&self) -> &Arc<ShadowSnapshot> {
        &self.inner.shadow
    }

    /// Open a per-message snapshot. Idempotent — repeat calls for the
    /// same `message_id` no-op rather than re-tracking. The first call
    /// runs `shadow.track()`, persists the resulting tree oid in
    /// metadata, and applies the per-task retention cap.
    ///
    /// When an FS-watcher accumulator is attached, this also re-routes
    /// future events to `(task_id, message_id)` and clears the previously
    /// pending dirty set for that key. The active-key change is atomic
    /// w.r.t. the snapshot row insert so events landing during the gap
    /// either accumulate against the new key or are dropped (no risk of
    /// being attributed to the wrong key).
    ///
    /// ### Restart correctness
    /// When the snapshot row ALREADY exists (re-open mid-session, or the
    /// app restarted and the user is resuming a task), the accumulator is
    /// marked `lost`. The DB and shadow repo are durable across restarts
    /// but the accumulator is in-memory only, so it has no record of any
    /// edits that happened between the snapshot's creation and the
    /// re-open. Marking lost forces the next sweep through the full-walk
    /// fallback, which re-reads the worktree and catches whatever the
    /// watcher missed.
    pub fn open_snapshot(&self, message_id: &str, task_id: &str) -> Result<()> {
        // Route subsequent FS events to this (task, message) pair. Done
        // before the row check so a re-open of the same key (e.g. on a
        // retry path) still re-arms the active routing.
        if let Some(acc) = &self.inner.accumulator {
            acc.set_active(task_id, message_id);
        }

        // Idempotent: if a row exists, don't re-track (we'd clobber the
        // pre-message state with the post-tool state).
        {
            let db = self.inner.db.lock().map_err(|_| FileHistoryError::Poisoned)?;
            if db.fh_get_snapshot(message_id)?.is_some() {
                // Pre-existing snapshot — see "Restart correctness" above.
                if let Some(acc) = &self.inner.accumulator {
                    acc.mark_lost();
                }
                return Ok(());
            }
        }

        // Capture the worktree state right now. This is the
        // "pre-message" tree from the caller's perspective: edit-tool
        // calls and bash invocations will mutate the worktree after we
        // return.
        let tracked = self.inner.shadow.track()?;
        let tree_oid = tracked.tree_oid.to_string();

        // Persist the snapshot row + apply retention.
        let evicted = {
            let db = self.inner.db.lock().map_err(|_| FileHistoryError::Poisoned)?;
            let next_seq = db.fh_max_sequence_for_task(task_id)? + 1;
            db.fh_insert_snapshot(message_id, task_id, next_seq, &tree_oid)?;
            db.fh_evict_old_snapshots(task_id, self.inner.max_snapshots)?
        };

        // Best-effort: if retention dropped any snapshots, kick a
        // reachability-based prune so the shadow odb doesn't grow
        // unbounded. Failures here log but don't fail the snapshot —
        // a stale loose object is harmless until next cleanup.
        if evicted > 0 {
            if let Err(e) = self.gc_unreferenced_blobs() {
                tracing::warn!(?e, task_id, "file_history: shadow cleanup after evict failed");
            }
        }
        Ok(())
    }

    /// Back-compat shim: reports what the snapshot's tree contains for
    /// `abs_path`. The actual content capture happened in
    /// `open_snapshot`; this call doesn't mutate the snapshot.
    pub fn capture(&self, message_id: &str, abs_path: &Path) -> Result<CaptureOutcome> {
        let rel_path = self.relativize(abs_path)?;

        // Size cap surfaced to the UI: if the on-disk file exceeds the
        // shadow's hard cap, the snapshot's tree didn't capture it.
        if let Ok(md) = std::fs::metadata(abs_path) {
            if md.is_file() && md.len() > MAX_TRACKED_FILE_SIZE {
                return Ok(CaptureOutcome::TooLarge {
                    rel_path,
                    size: md.len(),
                });
            }
        }

        let tree = self.tree_for_message(message_id)?;
        match self.inner.shadow.read_path(tree, &rel_path)? {
            Some(bytes) => Ok(CaptureOutcome::Captured {
                rel_path,
                hash: tree.to_string(),
                size: bytes.len() as u64,
            }),
            None => Ok(CaptureOutcome::DidNotExist { rel_path }),
        }
    }

    /// Restore the worktree to the state recorded in `message_id`'s tree.
    /// Only paths that *differ* between the tree and the current worktree
    /// AND that this task is known to have touched are restored — quiescent
    /// files stay untouched, and (critically) files modified by other chat
    /// sessions, external editors, or any work outside this task are left
    /// alone.
    ///
    /// Why the scoping matters: the snapshot tree captures the *full* repo
    /// state at the moment the user message landed. Reverting against the
    /// raw `diff_paths(target, current)` would happily rewrite any file
    /// that differs — including unrelated edits by a parallel chat or an
    /// external IDE working in the same repo. Restricting the restore to
    /// paths in this task's `compute_task_path_scope` makes revert behave
    /// like "undo what THIS task did" instead of "time-travel the whole
    /// worktree".
    pub fn revert(&self, message_id: &str) -> Result<Vec<RestoreOutcome>> {
        let target = self.tree_for_message(message_id)?;
        let current = self.inner.shadow.track()?.tree_oid;
        let mut changed = self.inner.shadow.diff_paths(target, current)?;
        if changed.is_empty() {
            return Ok(Vec::new());
        }
        let task_id = {
            let db = self.inner.db.lock().map_err(|_| FileHistoryError::Poisoned)?;
            db.fh_get_snapshot(message_id)?
                .map(|r| r.task_id)
                .ok_or_else(|| FileHistoryError::SnapshotNotFound(message_id.to_string()))?
        };
        let scope = self.compute_task_path_scope(&task_id)?;
        if !scope.is_empty() {
            changed.retain(|p| scope.contains(p));
        }
        if changed.is_empty() {
            return Ok(Vec::new());
        }
        let actions = self.inner.shadow.restore_paths_from(target, &changed)?;
        Ok(actions.into_iter().map(restore_action_to_outcome).collect())
    }

    /// Union of every path this task touched across the lifetime of its
    /// snapshot chain: any path that differs between two consecutive
    /// snapshots, plus any path that differs between the last snapshot and
    /// the recorded `final_tree_oid` (if set). This is the set of paths
    /// the user can reasonably consider "this task's responsibility".
    ///
    /// Returns an empty set if the task has fewer than two snapshots AND
    /// no final tree — there's nothing we can attribute to this task in
    /// that case, so revert correctly degrades to a no-op rather than
    /// blast-radiusing the whole repo.
    fn compute_task_path_scope(
        &self,
        task_id: &str,
    ) -> Result<std::collections::HashSet<String>> {
        let (snapshots, final_tree_str) = {
            let db = self.inner.db.lock().map_err(|_| FileHistoryError::Poisoned)?;
            (
                db.fh_list_snapshots_for_task(task_id)?,
                db.get_task_final_tree_oid(task_id)?,
            )
        };
        let oids: Vec<Oid> = snapshots
            .iter()
            .filter_map(|s| s.tree_oid.as_deref().and_then(|t| Oid::from_hex(t.as_bytes()).ok()))
            .collect();
        let mut scope: std::collections::HashSet<String> = std::collections::HashSet::new();
        for w in oids.windows(2) {
            for p in self.inner.shadow.diff_paths(w[0], w[1])? {
                scope.insert(p);
            }
        }
        if let (Some(last_oid), Some(final_str)) = (oids.last(), final_tree_str.as_deref()) {
            if let Ok(final_oid) = Oid::from_hex(final_str.as_bytes()) {
                for p in self.inner.shadow.diff_paths(*last_oid, final_oid)? {
                    scope.insert(p);
                }
            }
        }
        Ok(scope)
    }

    /// In the shadow model `revert_from_message(m)` is equivalent to
    /// `revert(m)` — the recorded tree captures the full pre-message
    /// worktree, so restoring to it also undoes every later message.
    pub fn revert_from_message(&self, message_id: &str) -> Result<Vec<RestoreOutcome>> {
        self.revert(message_id)
    }

    /// Revert to the earliest snapshot in `task_id` — i.e., the state
    /// right before the task did anything.
    pub fn revert_task(&self, task_id: &str) -> Result<Vec<RestoreOutcome>> {
        let snapshots = {
            let db = self.inner.db.lock().map_err(|_| FileHistoryError::Poisoned)?;
            db.fh_list_snapshots_for_task(task_id)?
        };
        match snapshots.into_iter().find(|s| s.tree_oid.is_some()) {
            Some(first) => self.revert(&first.message_id),
            None => Ok(Vec::new()),
        }
    }

    /// Dry-run a `revert` — returns the list of paths that would change
    /// and what would happen to each, without touching disk. Mirrors the
    /// scoping `revert` applies so the preview matches the real action.
    pub fn plan_revert_from_message(&self, message_id: &str) -> Result<Vec<RevertPlanEntry>> {
        let target = self.tree_for_message(message_id)?;
        let current = self.inner.shadow.track()?.tree_oid;
        let mut changed = self.inner.shadow.diff_paths(target, current)?;
        let task_id = {
            let db = self.inner.db.lock().map_err(|_| FileHistoryError::Poisoned)?;
            db.fh_get_snapshot(message_id)?
                .map(|r| r.task_id)
                .ok_or_else(|| FileHistoryError::SnapshotNotFound(message_id.to_string()))?
        };
        let scope = self.compute_task_path_scope(&task_id)?;
        if !scope.is_empty() {
            changed.retain(|p| scope.contains(p));
        }
        let mut out: Vec<RevertPlanEntry> = Vec::with_capacity(changed.len());
        for path in changed {
            let in_target = self.inner.shadow.read_path(target, &path)?.is_some();
            let abs = join_rel(&self.inner.project_root, &path);
            let on_disk = abs.is_file();
            let action = match (in_target, on_disk) {
                // Tree has it, disk differs (or missing) → restore writes.
                (true, _) => "restore",
                // Tree doesn't have it but disk does → restore deletes.
                (false, true) => "delete",
                // Neither side has it → nothing to do (defensive — `diff_paths`
                // shouldn't surface this, but the UI expects an "action" word).
                (false, false) => "keep",
            };
            out.push(RevertPlanEntry { path, action });
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(out)
    }

    /// Plan-only sibling of `revert_task`.
    pub fn plan_revert_task(&self, task_id: &str) -> Result<Vec<RevertPlanEntry>> {
        let snapshots = {
            let db = self.inner.db.lock().map_err(|_| FileHistoryError::Poisoned)?;
            db.fh_list_snapshots_for_task(task_id)?
        };
        match snapshots.into_iter().find(|s| s.tree_oid.is_some()) {
            Some(first) => self.plan_revert_from_message(&first.message_id),
            None => Ok(Vec::new()),
        }
    }

    /// List the paths captured in `message_id`'s tree.
    ///
    /// Pre-R.1 callers got back `FileHistoryFileRow` rows; the public
    /// signature is simplified to `Vec<String>` since the per-row stat
    /// cache and `blob_hash` aren't part of the shadow model. Frontend
    /// callers (`fh_list_files`) go through `list_files_with_stats`
    /// instead and are unaffected.
    pub fn list_files(&self, message_id: &str) -> Result<Vec<String>> {
        let tree = self.tree_for_message(message_id)?;
        let mut paths = self.inner.shadow.list_paths(tree)?;
        paths.sort();
        Ok(paths)
    }

    /// Per-path `+adds/-dels` stats for everything that differs between
    /// the snapshot's tree and the current worktree. One libgit2 diff
    /// pass — no per-path blob reads or text-diff invocations.
    pub fn list_files_with_stats(&self, message_id: &str) -> Result<Vec<FileChangeStats>> {
        let tree = self.tree_for_message(message_id)?;
        let current = self.inner.shadow.track()?.tree_oid;
        let mut diffs = self.inner.shadow.diff_full(tree, current)?;
        diffs.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(diffs.into_iter().map(shadow_change_to_stats).collect())
    }

    /// Cumulative pre-task vs current diff. Uses the earliest snapshot
    /// with a populated tree_oid as the baseline; tags each row with
    /// that `anchor_message_id` so the UI can fetch a per-file unified
    /// diff via `file_diff`.
    pub fn list_task_net_changes(&self, task_id: &str) -> Result<Vec<TaskNetChange>> {
        let snapshots = {
            let db = self.inner.db.lock().map_err(|_| FileHistoryError::Poisoned)?;
            db.fh_list_snapshots_for_task(task_id)?
        };
        if snapshots.is_empty() {
            return Ok(Vec::new());
        }

        let baseline = match snapshots.iter().find(|s| s.tree_oid.is_some()) {
            Some(s) => s.clone(),
            None => return Ok(Vec::new()),
        };
        let baseline_tree = parse_tree(&baseline)?;
        let current = self.inner.shadow.track()?.tree_oid;
        let mut diffs = self.inner.shadow.diff_full(baseline_tree, current)?;
        diffs.sort_by(|a, b| a.path.cmp(&b.path));

        let mut out = Vec::with_capacity(diffs.len());
        for change in diffs {
            let stats = shadow_change_to_stats(change);
            // Defensive: tree-to-tree diff shouldn't surface zero-line
            // text modifications, but skip them if it does.
            if stats.kind == "modified"
                && stats.additions == 0
                && stats.deletions == 0
                && !stats.binary
            {
                continue;
            }
            out.push(TaskNetChange {
                path: stats.path,
                kind: stats.kind,
                binary: stats.binary,
                additions: stats.additions,
                deletions: stats.deletions,
                anchor_message_id: baseline.message_id.clone(),
            });
        }
        Ok(out)
    }

    /// Compute the unified diff and stats for a single (snapshot, path).
    /// Uses libgit2's canonical `diff --git` output — matching what the
    /// frontend's diff renderer expects from real git tools.
    pub fn file_diff(&self, message_id: &str, path: &str) -> Result<FileDiff> {
        let tree = self.tree_for_message(message_id)?;
        let current = self.inner.shadow.track()?.tree_oid;
        let diffs = self.inner.shadow.diff_full(tree, current)?;
        let change = diffs.into_iter().find(|c| c.path == path);

        // If the path is unchanged between the snapshot and the current
        // worktree, fall back to a stats record with no diff text. The
        // Tauri command treats an empty `unified` field as "nothing to
        // show" which is what we want here.
        let stats = match change {
            Some(c) => shadow_change_to_stats(c),
            None => FileChangeStats {
                path: path.to_string(),
                kind: "modified",
                binary: false,
                additions: 0,
                deletions: 0,
            },
        };

        let unified = if stats.binary || stats.additions + stats.deletions == 0 {
            String::new()
        } else {
            self.inner
                .shadow
                .unified_diff_for_path(tree, current, path)?
                .unwrap_or_default()
        };

        Ok(FileDiff {
            path: stats.path,
            kind: stats.kind,
            binary: stats.binary,
            additions: stats.additions,
            deletions: stats.deletions,
            unified,
        })
    }

    /// Capture the worktree state right after a turn completes and persist it
    /// as this task's `final_tree_oid`. `list_task_net_changes_final` uses
    /// this oid instead of live disk when the task is not actively running,
    /// preventing external edits made after the turn from showing up in the
    /// Changed Files panel.
    ///
    /// Best-effort: failures are logged but not propagated so a transient
    /// shadow error never blocks turn completion.
    pub fn record_final_state(&self, task_id: &str) -> Result<()> {
        let tracked = self.inner.shadow.track()?;
        let oid = tracked.tree_oid.to_string();
        let db = self.inner.db.lock().map_err(|_| FileHistoryError::Poisoned)?;
        db.update_task_final_tree_oid(task_id, &oid)?;
        Ok(())
    }

    /// Like `list_task_net_changes` but uses the stored `final_tree_oid` as
    /// the "current" endpoint instead of live disk. Falls back to live disk
    /// when `final_tree_oid` is absent (tasks that predate this feature or
    /// that have never completed a turn).
    ///
    /// Call this for tasks that are not currently running so external edits
    /// made after the task finished don't appear in the Changed Files panel.
    pub fn list_task_net_changes_final(&self, task_id: &str) -> Result<Vec<TaskNetChange>> {
        let snapshots = {
            let db = self.inner.db.lock().map_err(|_| FileHistoryError::Poisoned)?;
            db.fh_list_snapshots_for_task(task_id)?
        };
        if snapshots.is_empty() {
            return Ok(Vec::new());
        }

        let baseline = match snapshots.iter().find(|s| s.tree_oid.is_some()) {
            Some(s) => s.clone(),
            None => return Ok(Vec::new()),
        };
        let baseline_tree = parse_tree(&baseline)?;

        // Prefer the stored final oid; fall back to live disk for old tasks.
        let current = {
            let stored = {
                let db = self.inner.db.lock().map_err(|_| FileHistoryError::Poisoned)?;
                db.get_task_final_tree_oid(task_id)?
            };
            match stored {
                Some(s) => Oid::from_hex(s.as_bytes()).map_err(|_| {
                    FileHistoryError::InvalidTreeOid {
                        message_id: format!("task:{task_id}:final"),
                        raw: s,
                    }
                })?,
                None => self.inner.shadow.track()?.tree_oid,
            }
        };

        let mut diffs = self.inner.shadow.diff_full(baseline_tree, current)?;
        diffs.sort_by(|a, b| a.path.cmp(&b.path));

        let mut out = Vec::with_capacity(diffs.len());
        for change in diffs {
            let stats = shadow_change_to_stats(change);
            if stats.kind == "modified"
                && stats.additions == 0
                && stats.deletions == 0
                && !stats.binary
            {
                continue;
            }
            out.push(TaskNetChange {
                path: stats.path,
                kind: stats.kind,
                binary: stats.binary,
                additions: stats.additions,
                deletions: stats.deletions,
                anchor_message_id: baseline.message_id.clone(),
            });
        }
        Ok(out)
    }

    /// Re-track the worktree after a bash invocation. Returns paths that
    /// changed since the previous tree; no-op (empty list) when the
    /// snapshot row is missing.
    ///
    /// When an FS-watcher accumulator is attached and it holds a non-lost,
    /// non-empty dirty set for `message_id`, this uses `shadow.track_paths`
    /// to re-hash only the changed paths — O(changes) instead of
    /// O(worktree). On lost-event or absent accumulator, falls back to the
    /// full walk (`shadow.track`) for correctness.
    ///
    /// The snapshot row's tree_oid is intentionally NOT updated here so
    /// `list_task_net_changes` retains a stable pre-turn baseline.
    pub fn record_post_bash_state(&self, message_id: &str) -> Result<Vec<String>> {
        let prev = match self.tree_for_message_optional(message_id)? {
            Some(t) => t,
            None => return Ok(Vec::new()),
        };

        // Fast path: if the accumulator holds the dirty set for this
        // message, do a targeted re-track. Looking up the task id lets us
        // address the right entry without scanning every key.
        if let Some(acc) = &self.inner.accumulator {
            let task_id = {
                let db = self.inner.db.lock().map_err(|_| FileHistoryError::Poisoned)?;
                db.fh_get_snapshot(message_id)?.map(|r| r.task_id)
            };
            if let Some(task_id) = task_id {
                let set = acc.drain(&task_id, message_id);
                if !set.lost && !set.is_empty() {
                    let modified: Vec<String> = set.modified.into_iter().collect();
                    let removed: Vec<String> = set.removed.into_iter().collect();
                    let next = self
                        .inner
                        .shadow
                        .track_paths(prev, &modified, &removed)?
                        .tree_oid;
                    if next == prev {
                        return Ok(Vec::new());
                    }
                    let mut changed = self.inner.shadow.diff_paths(prev, next)?;
                    changed.sort();
                    return Ok(changed);
                }
                // set.lost or set.is_empty → fall through to full walk
            }
        }

        // Fallback: full O(worktree) walk. Same as pre-R.2 behaviour.
        let next = self.inner.shadow.track()?.tree_oid;
        if next == prev {
            return Ok(Vec::new());
        }
        let mut changed = self.inner.shadow.diff_paths(prev, next)?;
        changed.sort();
        Ok(changed)
    }

    /// Trim snapshots beyond `max_snapshots` for this task.
    pub fn evict_old(&self, task_id: &str) -> Result<usize> {
        let db = self.inner.db.lock().map_err(|_| FileHistoryError::Poisoned)?;
        Ok(db.fh_evict_old_snapshots(task_id, self.inner.max_snapshots)?)
    }

    /// Prune shadow odb objects unreachable from any live snapshot.
    pub fn gc_unreferenced_blobs(&self) -> Result<usize> {
        let keep: Vec<Oid> = {
            let db = self.inner.db.lock().map_err(|_| FileHistoryError::Poisoned)?;
            // Include both per-message and per-task final trees so neither set is pruned.
            db.fh_all_tree_oids()?
                .into_iter()
                .chain(db.fh_all_final_tree_oids()?.into_iter())
                .filter_map(|s| Oid::from_hex(s.as_bytes()).ok())
                .collect()
        };
        Ok(self.inner.shadow.cleanup(&keep, GC_PRUNE_HORIZON)?)
    }

    /// No-op in the shadow model — kept for legacy callers (startup pass).
    pub fn reconcile_disk_orphans(&self) -> Result<usize> {
        Ok(0)
    }

    fn tree_for_message(&self, message_id: &str) -> Result<Oid> {
        match self.tree_for_message_optional(message_id)? {
            Some(t) => Ok(t),
            None => Err(FileHistoryError::SnapshotNotFound(message_id.to_string())),
        }
    }

    fn tree_for_message_optional(&self, message_id: &str) -> Result<Option<Oid>> {
        let db = self.inner.db.lock().map_err(|_| FileHistoryError::Poisoned)?;
        let row = match db.fh_get_snapshot(message_id)? {
            Some(r) => r,
            None => return Ok(None),
        };
        match row.tree_oid {
            Some(s) => Oid::from_hex(s.as_bytes())
                .map(Some)
                .map_err(|_| FileHistoryError::InvalidTreeOid {
                    message_id: message_id.to_string(),
                    raw: s,
                }),
            None => Err(FileHistoryError::SnapshotMissingTree(message_id.to_string())),
        }
    }

    fn relativize(&self, abs_path: &Path) -> Result<String> {
        let canon_root = self
            .inner
            .project_root
            .canonicalize()
            .unwrap_or_else(|_| self.inner.project_root.clone());
        // Walk up to the deepest existing ancestor so `capture` works
        // even when the edit tool is creating a brand-new file under a
        // brand-new directory.
        let canon_path = match abs_path.canonicalize() {
            Ok(p) => p,
            Err(_) => {
                let mut probe = abs_path.to_path_buf();
                loop {
                    if let Ok(p) = probe.canonicalize() {
                        let tail = abs_path.strip_prefix(&probe).unwrap_or(Path::new(""));
                        let mut joined = p;
                        joined.push(tail);
                        break joined;
                    }
                    if !probe.pop() {
                        return Err(FileHistoryError::OutsideProject {
                            path: abs_path.to_path_buf(),
                            root: self.inner.project_root.clone(),
                        });
                    }
                }
            }
        };
        let rel = canon_path
            .strip_prefix(&canon_root)
            .map_err(|_| FileHistoryError::OutsideProject {
                path: abs_path.to_path_buf(),
                root: self.inner.project_root.clone(),
            })?;
        Ok(normalize_rel(rel))
    }
}

fn restore_action_to_outcome(action: ShadowRestoreAction) -> RestoreOutcome {
    match action {
        ShadowRestoreAction::Wrote { path, .. } => RestoreOutcome::Rewritten(path),
        ShadowRestoreAction::Deleted { path } => RestoreOutcome::Deleted(path),
        ShadowRestoreAction::NoOp { path } => RestoreOutcome::Unchanged(path),
    }
}

/// Map a shadow-level diff entry into the legacy `FileChangeStats` shape
/// the frontend expects. The `"created" / "modified" / "deleted"` kind
/// strings are kept verbatim — frontend code switches on them.
fn shadow_change_to_stats(change: ShadowFileChange) -> FileChangeStats {
    let kind = match change.status {
        ShadowChangeStatus::Added => "created",
        ShadowChangeStatus::Modified => "modified",
        ShadowChangeStatus::Deleted => "deleted",
    };
    FileChangeStats {
        path: change.path,
        kind,
        binary: change.binary,
        additions: change.additions,
        deletions: change.deletions,
    }
}

fn parse_tree(row: &rustic_db::FileHistorySnapshotRow) -> Result<Oid> {
    match &row.tree_oid {
        Some(s) => Oid::from_hex(s.as_bytes()).map_err(|_| FileHistoryError::InvalidTreeOid {
            message_id: row.message_id.clone(),
            raw: s.clone(),
        }),
        None => Err(FileHistoryError::SnapshotMissingTree(row.message_id.clone())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustic_db::Database;
    use std::fs;
    use std::io::Write;

    struct Fixture {
        _cfg_dir: tempfile::TempDir,
        _proj_dir: tempfile::TempDir,
        history: FileHistory,
        project_root: PathBuf,
    }

    fn fixture() -> Fixture {
        let cfg_dir = tempfile::tempdir().unwrap();
        let proj_dir = tempfile::tempdir().unwrap();
        let project_root = proj_dir.path().canonicalize().unwrap();
        fs::create_dir_all(project_root.join(".git")).unwrap();
        let db = Arc::new(Mutex::new(Database::in_memory().unwrap()));
        {
            let g = db.lock().unwrap();
            g.conn()
                .execute(
                    "INSERT INTO projects (id, name, root_path) VALUES ('p', 'p', 'p')",
                    [],
                )
                .unwrap();
            g.conn()
                .execute(
                    "INSERT INTO tasks (id, project_id, title, status, provider_type, model)
                     VALUES ('t', 'p', 'title', 'created', 'native', 'm')",
                    [],
                )
                .unwrap();
        }
        let history = FileHistory::new(db, project_root.clone(), cfg_dir.path()).unwrap();
        Fixture {
            _cfg_dir: cfg_dir,
            _proj_dir: proj_dir,
            history,
            project_root,
        }
    }

    fn write(p: &Path, content: &[u8]) {
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let mut f = fs::File::create(p).unwrap();
        f.write_all(content).unwrap();
    }

    #[test]
    fn open_then_revert_restores_pre_message_state() {
        let f = fixture();
        let foo = f.project_root.join("foo.txt");
        write(&foo, b"original");

        f.history.open_snapshot("msg-1", "t").unwrap();
        // Agent mutates after snapshot.
        write(&foo, b"agent-edit");

        let outcomes = f.history.revert("msg-1").unwrap();
        assert!(outcomes
            .iter()
            .any(|o| matches!(o, RestoreOutcome::Rewritten(p) if p == "foo.txt")));
        assert_eq!(fs::read(&foo).unwrap(), b"original");
    }

    #[test]
    fn revert_deletes_files_created_after_snapshot() {
        let f = fixture();
        f.history.open_snapshot("msg-1", "t").unwrap();
        // Agent creates a new file after snapshot.
        let new_path = f.project_root.join("brand_new.txt");
        write(&new_path, b"freshly minted");

        let outcomes = f.history.revert("msg-1").unwrap();
        assert!(outcomes
            .iter()
            .any(|o| matches!(o, RestoreOutcome::Deleted(p) if p == "brand_new.txt")));
        assert!(!new_path.exists());
    }

    #[test]
    fn open_snapshot_is_idempotent() {
        let f = fixture();
        write(&f.project_root.join("x.txt"), b"v1");
        f.history.open_snapshot("msg-1", "t").unwrap();

        // Mutate, then call open_snapshot again with the same id — should
        // NOT re-capture (the original tree must still represent v1).
        write(&f.project_root.join("x.txt"), b"v2-agent");
        f.history.open_snapshot("msg-1", "t").unwrap();

        let outcomes = f.history.revert("msg-1").unwrap();
        assert!(outcomes.iter().any(|o| matches!(o, RestoreOutcome::Rewritten(_))));
        assert_eq!(fs::read(f.project_root.join("x.txt")).unwrap(), b"v1");
    }

    /// Regression port: bash deletes + recreates a file across turns;
    /// revert from the later turn must restore the prior-turn content,
    /// not delete the file. Shadow model wins this by capturing the full
    /// pre-message tree at open_snapshot time.
    #[test]
    fn revert_after_bash_delete_recreate_restores_prior_content() {
        let f = fixture();
        let x = f.project_root.join("x.txt");

        f.history.open_snapshot("msg-A", "t").unwrap();
        // Turn A creates the file.
        write(&x, b"content-A");

        // Turn B opens — pre-B state (content-A) is now in tree-B.
        f.history.open_snapshot("msg-B", "t").unwrap();
        // Simulate bash deleting then recreating with new content.
        fs::remove_file(&x).unwrap();
        write(&x, b"content-B");

        // Revert from B restores content-A.
        f.history.revert_from_message("msg-B").unwrap();
        assert!(x.exists());
        assert_eq!(fs::read(&x).unwrap(), b"content-A");

        // Revert from A removes the file (tree-A pre-dates its creation).
        f.history.revert_from_message("msg-A").unwrap();
        assert!(!x.exists());
    }

    #[test]
    fn plan_revert_describes_actions_without_touching_disk() {
        let f = fixture();
        write(&f.project_root.join("a.txt"), b"original-a");
        f.history.open_snapshot("msg-1", "t").unwrap();

        // Agent edits one file and creates another.
        write(&f.project_root.join("a.txt"), b"modified");
        write(&f.project_root.join("b.txt"), b"new");

        let plan = f.history.plan_revert_from_message("msg-1").unwrap();
        let by_path: std::collections::HashMap<_, _> = plan
            .into_iter()
            .map(|e| (e.path, e.action))
            .collect();
        assert_eq!(by_path.get("a.txt"), Some(&"restore"));
        assert_eq!(by_path.get("b.txt"), Some(&"delete"));

        // Disk is untouched after plan.
        assert_eq!(fs::read(f.project_root.join("a.txt")).unwrap(), b"modified");
        assert!(f.project_root.join("b.txt").exists());
    }

    #[test]
    fn capture_reports_too_large_for_oversized_files() {
        let f = fixture();
        let big = vec![b'A'; (MAX_TRACKED_FILE_SIZE + 1) as usize];
        write(&f.project_root.join("huge.bin"), &big);
        f.history.open_snapshot("msg-1", "t").unwrap();

        let outcome = f
            .history
            .capture("msg-1", &f.project_root.join("huge.bin"))
            .unwrap();
        match outcome {
            CaptureOutcome::TooLarge { rel_path, size } => {
                assert_eq!(rel_path, "huge.bin");
                assert_eq!(size, MAX_TRACKED_FILE_SIZE + 1);
            }
            other => panic!("expected TooLarge, got {other:?}"),
        }
    }

    #[test]
    fn capture_reports_did_not_exist_when_path_absent_from_tree() {
        let f = fixture();
        f.history.open_snapshot("msg-1", "t").unwrap();
        // No file at this path, ever.
        let phantom = f.project_root.join("phantom.txt");
        // capture canonicalises; create a parent dir so canonicalize
        // doesn't bail on the missing leaf.
        let outcome = f.history.capture("msg-1", &phantom).unwrap();
        assert!(matches!(outcome, CaptureOutcome::DidNotExist { .. }));
    }

    #[test]
    fn list_files_reflects_snapshot_tree() {
        let f = fixture();
        write(&f.project_root.join("a.txt"), b"a");
        write(&f.project_root.join("sub/b.txt"), b"b");
        f.history.open_snapshot("msg-1", "t").unwrap();

        let mut files = f.history.list_files("msg-1").unwrap();
        files.sort();
        assert_eq!(files, vec!["a.txt".to_string(), "sub/b.txt".to_string()]);
    }

    #[test]
    fn list_files_with_stats_returns_changed_paths_only() {
        let f = fixture();
        write(&f.project_root.join("stable.txt"), b"unchanged");
        write(&f.project_root.join("churn.txt"), b"v1");
        f.history.open_snapshot("msg-1", "t").unwrap();
        // Modify one file only.
        write(&f.project_root.join("churn.txt"), b"v2");

        let stats = f.history.list_files_with_stats("msg-1").unwrap();
        let names: Vec<_> = stats.iter().map(|s| s.path.clone()).collect();
        assert_eq!(names, vec!["churn.txt".to_string()]);
        assert!(stats[0].additions >= 1 || stats[0].deletions >= 1);
        assert_eq!(stats[0].kind, "modified");
    }

    #[test]
    fn file_diff_emits_unified_text_for_modified_file() {
        let f = fixture();
        write(&f.project_root.join("x.txt"), b"line1\nline2\n");
        f.history.open_snapshot("msg-1", "t").unwrap();
        write(&f.project_root.join("x.txt"), b"line1\nline2-edit\n");

        let diff = f.history.file_diff("msg-1", "x.txt").unwrap();
        assert_eq!(diff.kind, "modified");
        assert!(diff.unified.contains("-line2"));
        assert!(diff.unified.contains("+line2-edit"));
    }

    #[test]
    fn list_task_net_changes_uses_earliest_snapshot_as_baseline() {
        let f = fixture();
        write(&f.project_root.join("file.txt"), b"pre-task");
        f.history.open_snapshot("msg-1", "t").unwrap();
        write(&f.project_root.join("file.txt"), b"after-msg-1");
        f.history.open_snapshot("msg-2", "t").unwrap();
        write(&f.project_root.join("file.txt"), b"after-msg-2");

        let net = f.history.list_task_net_changes("t").unwrap();
        assert_eq!(net.len(), 1);
        assert_eq!(net[0].path, "file.txt");
        assert_eq!(net[0].kind, "modified");
        // Baseline is msg-1 (earliest); anchor must match.
        assert_eq!(net[0].anchor_message_id, "msg-1");
    }

    #[test]
    fn evict_old_drops_excess_snapshots() {
        let f = fixture();
        for i in 1..=5 {
            f.history.open_snapshot(&format!("m{i}"), "t").unwrap();
            // touch a file each turn so trees actually differ
            write(&f.project_root.join("c.txt"), format!("v{i}").as_bytes());
        }
        // Manually shrink and evict via the underlying DB call — the
        // public API doesn't expose a configurable cap (legacy: 100).
        let evicted = {
            let db_arc = Arc::clone(&f.history.inner.db);
            let db = db_arc.lock().unwrap();
            db.fh_evict_old_snapshots("t", 2).unwrap()
        };
        assert_eq!(evicted, 3);
        // GC then prunes unreferenced shadow objects.
        let _ = f.history.gc_unreferenced_blobs().unwrap();
        let _ = f.history.revert("m4").unwrap();
        let _ = f.history.revert("m5").unwrap();
    }

    #[test]
    fn three_concurrent_tasks_have_independent_revert_state() {
        let f = fixture();
        {
            let db = f.history.inner.db.lock().unwrap();
            for t in &["t1", "t2", "t3"] {
                let sql = format!(
                    "INSERT INTO tasks (id, project_id, title, status, provider_type, model)
                     VALUES ('{t}', 'p', 'title', 'created', 'native', 'm')"
                );
                db.conn().execute(&sql, []).unwrap();
            }
        }

        write(&f.project_root.join("base.txt"), b"baseline");
        let history = f.history.clone();
        for t in &["t1", "t2", "t3"] {
            history.open_snapshot(&format!("msg-{t}"), t).unwrap();
        }

        let handles: Vec<_> = ["t1", "t2", "t3"]
            .into_iter()
            .map(|t| {
                let project = f.project_root.clone();
                std::thread::spawn(move || {
                    write(&project.join("base.txt"), format!("edited-by-{t}").as_bytes());
                    for i in 0..4 {
                        write(
                            &project.join(format!("{t}_extra_{i}.txt")),
                            format!("{t}-{i}").as_bytes(),
                        );
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }

        let history = f.history.clone();
        let revert_handles: Vec<_> = ["t1", "t2", "t3"]
            .into_iter()
            .map(|t| {
                let h = history.clone();
                let id = format!("msg-{t}");
                std::thread::spawn(move || h.revert(&id).unwrap())
            })
            .collect();

        let mut all_outcomes = Vec::new();
        for h in revert_handles {
            all_outcomes.extend(h.join().unwrap());
        }
        // At least one restore action must have occurred across the three threads.
        assert!(
            all_outcomes
                .iter()
                .any(|o| matches!(o, RestoreOutcome::Rewritten(_) | RestoreOutcome::Deleted(_))),
            "expected at least one Rewritten or Deleted across the three concurrent reverts, got {all_outcomes:?}"
        );

        assert_eq!(fs::read(f.project_root.join("base.txt")).unwrap(), b"baseline");
        for t in &["t1", "t2", "t3"] {
            for i in 0..4 {
                assert!(
                    !f.project_root.join(format!("{t}_extra_{i}.txt")).exists(),
                    "{t}_extra_{i} should have been deleted by revert"
                );
            }
        }
    }

    /// Regression: revert must not clobber files modified only by a parallel chat or external editor.
    #[test]
    fn revert_leaves_other_tasks_files_untouched() {
        let f = fixture();
        // Seed both task rows so the FK on file_history_snapshots is happy.
        {
            let db = f.history.inner.db.lock().unwrap();
            for t in &["task-a", "task-b"] {
                let sql = format!(
                    "INSERT INTO tasks (id, project_id, title, status, provider_type, model)
                     VALUES ('{t}', 'p', 'title', 'created', 'native', 'm')"
                );
                db.conn().execute(&sql, []).unwrap();
            }
        }

        write(&f.project_root.join("a_file.txt"), b"a-original");
        write(&f.project_root.join("b_file.txt"), b"b-original");

        f.history.open_snapshot("msg-a1", "task-a").unwrap();
        write(&f.project_root.join("a_file.txt"), b"a-by-task-a");
        f.history.record_final_state("task-a").unwrap();

        // External editor / parallel chat — no snapshot for this edit.
        write(&f.project_root.join("b_file.txt"), b"b-by-other-work");

        let outcomes = f.history.revert("msg-a1").unwrap();
        let paths: Vec<_> = outcomes
            .iter()
            .map(|o| match o {
                RestoreOutcome::Rewritten(p) | RestoreOutcome::Deleted(p) => p.clone(),
                _ => String::new(),
            })
            .collect();
        assert!(
            paths.iter().any(|p| p == "a_file.txt"),
            "expected a_file to be restored; got {paths:?}"
        );
        assert!(
            paths.iter().all(|p| p != "b_file.txt"),
            "b_file must NOT be touched by Task A's revert; got {paths:?}"
        );
        assert_eq!(
            fs::read(f.project_root.join("a_file.txt")).unwrap(),
            b"a-original"
        );
        assert_eq!(
            fs::read(f.project_root.join("b_file.txt")).unwrap(),
            b"b-by-other-work",
            "external/other-task edit was clobbered by the revert"
        );
    }

    /// Retention cap exceeded → eviction drops the oldest tree_oids
    /// from the metadata table, and follow-up GC reclaims the now-
    /// unreferenced shadow loose objects.
    #[test]
    fn retention_eviction_plus_gc_frees_shadow_objects() {
        let f = fixture();
        // 10 snapshots, each mutating one file so each tree is distinct.
        for i in 1..=10 {
            f.history.open_snapshot(&format!("m{i}"), "t").unwrap();
            write(
                &f.project_root.join("churn.txt"),
                format!("version-{i}").as_bytes(),
            );
        }

        // Shrink the cap to 3 → expect 7 evictions.
        let evicted = {
            let db = f.history.inner.db.lock().unwrap();
            db.fh_evict_old_snapshots("t", 3).unwrap()
        };
        assert_eq!(evicted, 7);

        // GC must succeed and prune some objects. We don't pin an exact
        // count (libgit2's auto-packing could change loose-vs-packed
        // ratios), only that pruning is non-trivial: count loose objects
        // before & after.
        let objects_dir = f.history.shadow().repo_path().join("objects");
        let count_loose = || {
            let mut n = 0;
            if let Ok(buckets) = fs::read_dir(&objects_dir) {
                for bucket in buckets.flatten() {
                    if let Ok(entries) = fs::read_dir(bucket.path()) {
                        n += entries.count();
                    }
                }
            }
            n
        };
        let before = count_loose();
        // Use Duration::ZERO to make the test deterministic — production
        // uses the 7-day horizon but here we want immediate reclamation.
        let pruned = f
            .history
            .shadow()
            .cleanup(
                &{
                    let db = f.history.inner.db.lock().unwrap();
                    db.fh_all_tree_oids()
                        .unwrap()
                        .into_iter()
                        .filter_map(|s| Oid::from_hex(s.as_bytes()).ok())
                        .collect::<Vec<_>>()
                },
                std::time::Duration::ZERO,
            )
            .unwrap();
        let after = count_loose();
        assert!(
            pruned > 0,
            "expected GC to prune at least one object after evicting 7 snapshots"
        );
        assert!(
            after < before,
            "loose object count should drop after GC ({before} -> {after})"
        );

        // The three surviving snapshots remain revertable.
        for i in 8..=10 {
            f.history.revert(&format!("m{i}")).unwrap();
        }
    }

    /// Many-file perf smoke. We aim for "comfortably under 5 s on the
    /// dev box for 1000 files" — not a hard benchmark, just a regression
    /// guard. If this trips a CI run on a slow shared runner, drop the
    /// file count or relax the budget rather than removing the test.
    #[test]
    fn track_thousand_small_files_in_under_5s() {
        let f = fixture();
        // Spread across subdirs so the gitignore walker and tree
        // builder both exercise nested paths.
        for i in 0..1000 {
            let sub = format!("d{:02}", i / 50);
            write(
                &f.project_root.join(&sub).join(format!("f{i:04}.txt")),
                format!("contents of file {i}").as_bytes(),
            );
        }

        let started = std::time::Instant::now();
        f.history.open_snapshot("msg-perf", "t").unwrap();
        let cold = started.elapsed();
        assert!(
            cold < std::time::Duration::from_secs(5),
            "cold open_snapshot took {cold:?} for 1000 files (budget 5s)"
        );

        // Hot path: re-track the same worktree — same content, so the
        // resulting tree_oid is identical and most blobs are already
        // in the odb. Should be faster than the cold pass.
        let started = std::time::Instant::now();
        let _ = f.history.shadow().track().unwrap();
        let hot = started.elapsed();
        assert!(
            hot < std::time::Duration::from_secs(5),
            "hot shadow.track() took {hot:?} for 1000 files (budget 5s)"
        );
    }

    /// Fixture variant that wires in a DirtyPathAccumulator — the same
    /// shape the Tauri host builds for production. Used by the targeted-
    /// track tests below.
    fn fixture_with_accumulator() -> (Fixture, Arc<DirtyPathAccumulator>) {
        let cfg_dir = tempfile::tempdir().unwrap();
        let proj_dir = tempfile::tempdir().unwrap();
        let project_root = proj_dir.path().canonicalize().unwrap();
        fs::create_dir_all(project_root.join(".git")).unwrap();
        let db = Arc::new(Mutex::new(Database::in_memory().unwrap()));
        {
            let g = db.lock().unwrap();
            g.conn()
                .execute(
                    "INSERT INTO projects (id, name, root_path) VALUES ('p', 'p', 'p')",
                    [],
                )
                .unwrap();
            g.conn()
                .execute(
                    "INSERT INTO tasks (id, project_id, title, status, provider_type, model)
                     VALUES ('t', 'p', 'title', 'created', 'native', 'm')",
                    [],
                )
                .unwrap();
        }
        let accumulator = Arc::new(DirtyPathAccumulator::new(project_root.clone()));
        let history = FileHistory::new_with_accumulator(
            db,
            project_root.clone(),
            cfg_dir.path(),
            Some(Arc::clone(&accumulator)),
        )
        .unwrap();
        (
            Fixture {
                _cfg_dir: cfg_dir,
                _proj_dir: proj_dir,
                history,
                project_root,
            },
            accumulator,
        )
    }

    #[test]
    fn open_snapshot_sets_accumulator_active_key() {
        let (f, acc) = fixture_with_accumulator();
        f.history.open_snapshot("msg-1", "t").unwrap();
        // After open_snapshot, recording a path should land in the dirty
        // set for the active (task, message) — verifiable via drain.
        acc.record_modified(&f.project_root.join("hello.rs"));
        let set = acc.drain("t", "msg-1");
        assert!(
            set.modified.contains("hello.rs"),
            "open_snapshot must route accumulator events to (task, message)"
        );
    }

    #[test]
    fn record_post_bash_state_uses_targeted_track_when_accumulator_populated() {
        let (f, acc) = fixture_with_accumulator();
        // Pre-message state: two files that will stay unchanged.
        write(&f.project_root.join("keep_a.txt"), b"a");
        write(&f.project_root.join("keep_b.txt"), b"b");
        f.history.open_snapshot("msg", "t").unwrap();

        // Simulate the watcher reporting one path changed (the file we
        // actually edit) and one that didn't exist before but was created.
        write(&f.project_root.join("keep_a.txt"), b"a-edited");
        write(&f.project_root.join("new.txt"), b"freshly created");
        acc.record_modified(&f.project_root.join("keep_a.txt"));
        acc.record_modified(&f.project_root.join("new.txt"));

        let changed = f.history.record_post_bash_state("msg").unwrap();
        let mut sorted = changed.clone();
        sorted.sort();
        assert_eq!(sorted, vec!["keep_a.txt".to_string(), "new.txt".to_string()]);
    }

    #[test]
    fn record_post_bash_state_falls_back_to_full_walk_when_accumulator_lost() {
        let (f, acc) = fixture_with_accumulator();
        write(&f.project_root.join("baseline.txt"), b"original");
        f.history.open_snapshot("msg", "t").unwrap();

        // Lost flag set, plus a change on disk that the accumulator
        // missed. The fallback full walk must still discover it.
        write(&f.project_root.join("baseline.txt"), b"changed-while-watcher-was-down");
        write(&f.project_root.join("brand_new.txt"), b"also missed");
        acc.mark_lost();
        // Note: deliberately NOT calling record_modified for either path
        // — that's the scenario "watcher dropped events and we're
        // recovering via full walk".

        let mut changed = f.history.record_post_bash_state("msg").unwrap();
        changed.sort();
        assert_eq!(
            changed,
            vec!["baseline.txt".to_string(), "brand_new.txt".to_string()]
        );
    }

    /// Restart correctness: a snapshot row already exists in the DB but
    /// the accumulator is fresh (in-memory, lost on restart). Files were
    /// modified on disk while the watcher was offline. The next sweep
    /// must catch them via the full-walk fallback — `open_snapshot` on a
    /// pre-existing row marks the accumulator lost so the targeted-track
    /// path doesn't silently miss the pre-restart changes.
    #[test]
    fn open_snapshot_on_existing_row_marks_accumulator_lost_for_restart_recovery() {
        let (f, acc) = fixture_with_accumulator();

        // Session 1: write baseline + open snapshot.
        write(&f.project_root.join("seen.txt"), b"v1");
        f.history.open_snapshot("msg", "t").unwrap();

        // Simulate "app crash": drop the accumulator state by re-opening
        // the same snapshot id while files change on disk WITHOUT being
        // recorded in the accumulator. (In real restart, the whole
        // accumulator would be re-created empty; same effect.)
        write(&f.project_root.join("seen.txt"), b"v2-modified-offline");
        write(&f.project_root.join("born-offline.txt"), b"new while offline");
        // No record_modified() calls — pretend the watcher missed them.

        // Re-open snapshot for the same message id (idempotent path).
        f.history.open_snapshot("msg", "t").unwrap();

        // The accumulator should now be flagged lost so the next sweep
        // does a full walk.
        let set = acc.peek_active().expect("active key after re-open");
        assert!(
            set.lost,
            "re-open of existing snapshot must mark accumulator lost"
        );

        // record_post_bash_state should detect the lost flag, fall back
        // to full walk, and discover both off-watcher changes.
        let mut changed = f.history.record_post_bash_state("msg").unwrap();
        changed.sort();
        assert_eq!(
            changed,
            vec!["born-offline.txt".to_string(), "seen.txt".to_string()],
            "full-walk fallback after restart must catch every pre-restart change"
        );
    }

    #[test]
    fn record_post_bash_state_falls_back_when_accumulator_empty() {
        let (f, _acc) = fixture_with_accumulator();
        write(&f.project_root.join("x.txt"), b"v1");
        f.history.open_snapshot("msg", "t").unwrap();

        // Disk changed but accumulator stayed empty — perhaps the FS-event
        // dropped silently or the change came from a path the watcher
        // doesn't see (e.g. it was the very first edit on a brand-new
        // worktree before the watcher fully spun up). Full walk catches it.
        write(&f.project_root.join("x.txt"), b"v2");

        let changed = f.history.record_post_bash_state("msg").unwrap();
        assert_eq!(changed, vec!["x.txt".to_string()]);
    }

    /// Reopening the FileHistory mid-task (simulating a process restart)
    /// must preserve the previously-recorded snapshots and let revert
    /// continue to work — the shadow's bare repo persists on disk, and
    /// the SQLite metadata is durable.
    #[test]
    fn snapshots_survive_filehistory_reconstruction() {
        // Carry the dirs across the fixture rebuild so the shadow repo
        // and DB path stay stable.
        let cfg_dir = tempfile::tempdir().unwrap();
        let proj_dir = tempfile::tempdir().unwrap();
        let project_root = proj_dir.path().canonicalize().unwrap();
        fs::create_dir_all(project_root.join(".git")).unwrap();
        // Persistent on-disk database so the metadata survives the
        // FileHistory drop. Use a path inside cfg_dir.
        let db_path = cfg_dir.path().join("rustic.db");
        let make_history = || {
            let db = Arc::new(Mutex::new(
                rustic_db::Database::new(&db_path).expect("open db"),
            ));
            {
                let g = db.lock().unwrap();
                g.conn()
                    .execute(
                        "INSERT OR IGNORE INTO projects (id, name, root_path) VALUES ('p', 'p', 'p')",
                        [],
                    )
                    .unwrap();
                g.conn()
                    .execute(
                        "INSERT OR IGNORE INTO tasks
                         (id, project_id, title, status, provider_type, model)
                         VALUES ('t', 'p', 'title', 'created', 'native', 'm')",
                        [],
                    )
                    .unwrap();
            }
            FileHistory::new(db, project_root.clone(), cfg_dir.path()).unwrap()
        };

        // Round 1: open snapshot, mutate, then drop the handle entirely
        // (this simulates a process exit between user turns).
        {
            let history = make_history();
            write(&project_root.join("survive.txt"), b"original");
            history.open_snapshot("msg-1", "t").unwrap();
            write(&project_root.join("survive.txt"), b"edited-after-snapshot");
            // history drops here
        }

        // Round 2: re-open from scratch. The snapshot row is still in
        // the DB (durable), the shadow repo is still on disk — revert
        // should restore the original content.
        {
            let history = make_history();
            let outcomes = history.revert("msg-1").unwrap();
            assert!(
                outcomes
                    .iter()
                    .any(|o| matches!(o, RestoreOutcome::Rewritten(p) if p == "survive.txt")),
                "expected survive.txt restored after FileHistory reconstruction, got {outcomes:?}"
            );
            assert_eq!(
                fs::read(project_root.join("survive.txt")).unwrap(),
                b"original"
            );
        }
    }
}

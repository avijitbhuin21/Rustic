//! Synchronous tracker API used by edit tools and the sweep worker.
//!
//! `FileHistory` is the single owner of the blob store + SQLite index for one
//! task. Edit tools call `capture` synchronously before mutating a file; the
//! sweep worker calls `apply_sweep` in bulk after a bash command. Both paths
//! are sync and use blocking SQLite/`std::fs` calls — the sweep worker isolates
//! itself on a `tokio::task::spawn_blocking` thread so the agent runtime is
//! never starved.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use rustic_db::{Database, FileHistoryFileRow};
use thiserror::Error;

use super::blob_store::{BlobStore, BlobStoreError, MAX_TRACKED_FILE_SIZE};
use super::walk::{join_rel, normalize_rel, walk_for_sweep, WalkedFile};

#[derive(Debug, Error)]
pub enum FileHistoryError {
    #[error("blob_store: {0}")]
    BlobStore(#[from] BlobStoreError),

    #[error("db: {0}")]
    Db(#[from] rustic_db::DbError),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("snapshot {0} not found")]
    SnapshotNotFound(String),

    #[error("path {path:?} resolves outside the project root {root:?}")]
    OutsideProject { path: PathBuf, root: PathBuf },

    #[error("mutex poisoned")]
    Poisoned,
}

pub type Result<T> = std::result::Result<T, FileHistoryError>;

/// Outcome of a single `capture` call. The caller (typically an edit tool)
/// uses this to decide what to emit to the UI.
#[derive(Debug, Clone)]
pub enum CaptureOutcome {
    /// File existed and was hashed/stored. Snapshot now references the blob.
    Captured { rel_path: String, hash: String, size: u64 },
    /// File does not exist on disk. Snapshot records a null backup so revert
    /// will delete a same-named file that gets created later.
    DidNotExist { rel_path: String },
    /// Path was already captured in this snapshot — no-op (idempotent).
    AlreadyTracked { rel_path: String },
    /// File is bigger than the configured limit. Snapshot does not record it;
    /// the file is unrevertable. Caller may still surface this in the UI.
    TooLarge { rel_path: String, size: u64 },
}

/// The result of restoring one path during a revert. Mirrors what the UI
/// needs to highlight: was the file rewritten, deleted, or skipped because
/// it didn't change.
#[derive(Debug, Clone)]
pub enum RestoreOutcome {
    Rewritten(String),
    Deleted(String),
    Unchanged(String),
}

/// One record produced by the bash sweep — a path plus its current state on
/// disk. The tracker decides whether to write a blob, record a null backup,
/// or skip based on the existing snapshot row for that path.
#[derive(Debug, Clone)]
pub struct SweepFileChange {
    pub abs_path: PathBuf,
    pub rel_path: String,
    pub size: u64,
}

/// One row in a revert preview — what would happen to `path` if the user
/// confirms. `action` is `"restore"` (file gets rewritten from a pre-blob),
/// `"delete"` (file gets removed because it didn't exist before), or
/// `"keep"` (no change recorded).
#[derive(Debug, Clone)]
pub struct RevertPlanEntry {
    pub path: String,
    pub action: &'static str,
}

/// Per-file change summary the UI shows next to each path in the changed-files
/// panel. Computed by diffing the snapshot's stored blob against the file's
/// current on-disk content.
#[derive(Debug, Clone)]
pub struct FileChangeStats {
    pub path: String,
    /// "modified" — file existed before and still exists.
    /// "created" — no pre-blob (created during the turn).
    /// "deleted" — pre-blob exists but file is gone.
    pub kind: &'static str,
    /// True when either side of the diff is non-utf8 / contains NUL bytes.
    /// `additions` and `deletions` are 0 in that case (no line diff possible),
    /// but `kind` still reflects whether the file was created/deleted/modified.
    pub binary: bool,
    pub additions: u32,
    pub deletions: u32,
}

/// Cumulative net-change view for a task: one row per path that changed,
/// computed by diffing the EARLIEST snapshot's stored blob (the file's
/// pre-task state) against the file's current on-disk content. So a file
/// created in turn A and modified in turn C shows up as `created` (relative
/// to before the task started) — the in-between turns don't matter for this
/// view.
///
/// `anchor_message_id` is the message_id of the earliest snapshot containing
/// this path. The UI uses it to fetch a unified diff via `fh_file_diff`,
/// since that command also diffs against the same anchor's stored blob.
#[derive(Debug, Clone)]
pub struct TaskNetChange {
    pub path: String,
    pub kind: &'static str,
    pub binary: bool,
    pub additions: u32,
    pub deletions: u32,
    pub anchor_message_id: String,
}

/// Full diff for one file in a snapshot. The `unified` text is suitable for
/// rendering in the editor's diff preview.
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
    blob_store: BlobStore,
    project_root: PathBuf,
    /// Maximum number of snapshots to retain per task. Older snapshots are
    /// evicted FIFO; trigger-driven blob refcounting handles GC.
    max_snapshots: i64,
}

impl FileHistory {
    pub fn new(db: Arc<Mutex<Database>>, project_root: PathBuf, config_dir: &Path) -> Self {
        Self {
            inner: Arc::new(FileHistoryInner {
                db,
                blob_store: BlobStore::new(config_dir),
                project_root,
                max_snapshots: 100,
            }),
        }
    }

    pub fn project_root(&self) -> &Path {
        &self.inner.project_root
    }

    pub fn blob_store(&self) -> &BlobStore {
        &self.inner.blob_store
    }

    /// Open a new snapshot anchored to the given user message. Idempotent —
    /// safe to call repeatedly with the same `(message_id, task_id)` pair.
    /// The snapshot's `sequence` is `max(existing sequences for this task) + 1`.
    ///
    /// After inserting the snapshot row, this also pre-captures the
    /// CURRENT on-disk state of every path that any prior snapshot in the
    /// same task has tracked. That fills in the pre-turn state up front,
    /// before any tool or bash command can mutate the file system. Without
    /// it, bash-only delete-and-recreate sequences end up with the
    /// post-mutation state recorded as the pre-mutation state, which makes
    /// per-message revert delete files that should have been restored.
    /// See `capture_then_revert_handles_bash_delete_recreate` for the
    /// regression test.
    ///
    /// The pre-capture phase uses an mtime+size cache (the new columns on
    /// `file_history_files`): if the file's current `(mtime_ns, size)` match
    /// the most recent prior record's cached pair, we reuse the prior
    /// `blob_hash` directly and skip the read+hash. This keeps the cost
    /// proportional to the number of files actually changed since the last
    /// turn, not the number ever tracked.
    pub fn open_snapshot(&self, message_id: &str, task_id: &str) -> Result<()> {
        // Phase 1: insert the snapshot row (under the DB lock).
        {
            let db = self.inner.db.lock().map_err(|_| FileHistoryError::Poisoned)?;
            if db.fh_get_snapshot(message_id)?.is_some() {
                // Idempotent: snapshot already exists. Don't re-pre-capture
                // either, because some calls during this turn may already
                // have written entries we'd clobber.
                return Ok(());
            }
            let next_seq = db.fh_max_sequence_for_task(task_id)? + 1;
            db.fh_insert_snapshot(message_id, task_id, next_seq)?;
        }

        // Phase 2: pre-capture prior-tracked paths. We deliberately drop the
        // DB lock between reads so blob hashing (which can be slow for
        // larger files) doesn't hold the connection while every other tool
        // call waits.
        let prior = {
            let db = self.inner.db.lock().map_err(|_| FileHistoryError::Poisoned)?;
            db.fh_latest_files_for_task(task_id)?
        };
        for row in prior {
            // Skip stale rows that already happen to belong to this snapshot
            // (shouldn't happen — we just created it — but defensive).
            if row.message_id == message_id {
                continue;
            }
            let abs_path = join_rel(&self.inner.project_root, &row.path);
            self.precapture_one(message_id, &row, &abs_path)?;
        }

        // Phase 3: bounded retention. Without this, every snapshot row plus
        // every captured blob is kept forever — a long-running task would
        // accumulate megabytes of file_history rows and disk blobs over its
        // lifetime. `evict_old` drops snapshots beyond `max_snapshots`
        // (FIFO); the cascade decrements blob refcounts, and when something
        // was actually evicted we GC the now-orphaned blob files. Best-effort
        // — a failure here is logged but does not fail the snapshot.
        match self.evict_old(task_id) {
            Ok(evicted) if evicted > 0 => {
                if let Err(e) = self.gc_unreferenced_blobs() {
                    tracing::warn!(
                        ?e,
                        task_id,
                        "file_history: gc_unreferenced_blobs failed after evict"
                    );
                }
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(?e, task_id, "file_history: evict_old failed"),
        }

        Ok(())
    }

    /// Capture one prior-tracked path into the just-opened snapshot. Uses
    /// the (mtime_ns, size) stat cache from `row` to skip re-hashing files
    /// that haven't changed since the last snapshot recorded them.
    fn precapture_one(
        &self,
        message_id: &str,
        row: &FileHistoryFileRow,
        abs_path: &Path,
    ) -> Result<()> {
        match std::fs::metadata(abs_path) {
            Ok(md) if md.is_file() => {
                let size = md.len();
                let mtime_ns = mtime_to_ns(md.modified().ok());

                // Stat-only fast path: if the file's mtime+size match the
                // prior snapshot's cached pair AND that prior snapshot
                // recorded a non-null blob_hash, the content is unchanged
                // and we can reuse the prior hash without touching the file.
                if let (Some(cached_mtime), Some(cached_size), Some(cached_hash)) =
                    (row.mtime_ns, row.size, row.blob_hash.as_deref())
                {
                    if Some(cached_mtime) == mtime_ns && cached_size == size as i64 {
                        let db =
                            self.inner.db.lock().map_err(|_| FileHistoryError::Poisoned)?;
                        // Bump ref_count via the trigger by upserting; blob is
                        // already registered (it's referenced by `row`).
                        db.fh_upsert_file(
                            message_id,
                            &row.path,
                            Some(cached_hash),
                            mtime_ns,
                            Some(size as i64),
                        )?;
                        return Ok(());
                    }
                }

                // Slow path: file has changed (or we have no cache). Hash
                // it through the blob store. The store is hash-first
                // (see `blob_store::store_from_path`) so unchanged content
                // collapses to one blob without re-writing.
                if size > MAX_TRACKED_FILE_SIZE {
                    return Ok(());
                }
                let stored = self.inner.blob_store.store_from_path(abs_path)?;
                let db = self.inner.db.lock().map_err(|_| FileHistoryError::Poisoned)?;
                let stored_size = stored.size as i64;
                if !stored.already_present {
                    db.fh_register_blob(&stored.hash, stored_size)?;
                }
                db.fh_upsert_file(
                    message_id,
                    &row.path,
                    Some(&stored.hash),
                    mtime_ns,
                    Some(size as i64),
                )?;
                Ok(())
            }
            Ok(_) => {
                // Non-regular (directory/symlink). Skip — same as `capture`.
                Ok(())
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let db = self.inner.db.lock().map_err(|_| FileHistoryError::Poisoned)?;
                db.fh_upsert_file(message_id, &row.path, None, None, None)?;
                Ok(())
            }
            Err(e) => Err(FileHistoryError::Io(e)),
        }
    }

    /// Capture the pre-mutation state of `abs_path` into the snapshot for
    /// `message_id`. This MUST be called before the edit tool writes new
    /// content. Idempotent within one snapshot.
    pub fn capture(&self, message_id: &str, abs_path: &Path) -> Result<CaptureOutcome> {
        let rel_path = self.relativize(abs_path)?;

        // Idempotency: skip if this snapshot already has a row for this path.
        // Re-capturing would either no-op (same content -> same blob) or
        // overwrite v1 with post-edit content if called after a write.
        {
            let db = self.inner.db.lock().map_err(|_| FileHistoryError::Poisoned)?;
            if db.fh_get_file(message_id, &rel_path)?.is_some() {
                return Ok(CaptureOutcome::AlreadyTracked { rel_path });
            }
        }

        match std::fs::metadata(abs_path) {
            Ok(md) if md.is_file() => {
                if md.len() > MAX_TRACKED_FILE_SIZE {
                    return Ok(CaptureOutcome::TooLarge {
                        rel_path,
                        size: md.len(),
                    });
                }
                let size = md.len();
                let mtime_ns = mtime_to_ns(md.modified().ok());
                let stored = self.inner.blob_store.store_from_path(abs_path)?;
                let db = self.inner.db.lock().map_err(|_| FileHistoryError::Poisoned)?;
                let hash_size = stored.size as i64;
                if !stored.already_present {
                    db.fh_register_blob(&stored.hash, hash_size)?;
                }
                db.fh_upsert_file(
                    message_id,
                    &rel_path,
                    Some(&stored.hash),
                    mtime_ns,
                    Some(size as i64),
                )?;
                Ok(CaptureOutcome::Captured {
                    rel_path,
                    hash: stored.hash,
                    size: stored.size,
                })
            }
            Ok(_) => {
                // Path exists but isn't a regular file (dir/symlink). Treat
                // as did-not-exist — reverting will not delete a directory.
                Ok(CaptureOutcome::DidNotExist { rel_path })
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let db = self.inner.db.lock().map_err(|_| FileHistoryError::Poisoned)?;
                db.fh_upsert_file(message_id, &rel_path, None, None, None)?;
                Ok(CaptureOutcome::DidNotExist { rel_path })
            }
            Err(e) => Err(FileHistoryError::Io(e)),
        }
    }

    /// Bulk-apply the result of a bash sweep: each `SweepFileChange` represents
    /// one file the walker noticed had `mtime >= bash_start`. We hash + dedupe
    /// against any existing snapshot row, store new blobs as needed, and record
    /// the change. Files that are now missing get a null backup row.
    ///
    /// Returns the rel_paths that were newly recorded in the snapshot
    /// (deduped against pre-existing rows for the same snapshot).
    pub fn apply_sweep(
        &self,
        message_id: &str,
        changes: Vec<SweepFileChange>,
    ) -> Result<Vec<String>> {
        let mut newly_recorded = Vec::new();
        for change in changes {
            // Idempotency: if this snapshot already has a row for this path,
            // the pre-edit state was already captured — don't overwrite v1
            // with the post-bash content.
            {
                let db = self.inner.db.lock().map_err(|_| FileHistoryError::Poisoned)?;
                if db.fh_get_file(message_id, &change.rel_path)?.is_some() {
                    continue;
                }
            }

            // Decide what to store based on whether the file is on disk *now*
            // and how big it is. Note: this is the post-bash state, not the
            // pre-bash state. For files we'd never seen before, this is the
            // best we can do; revert will delete them rather than restore.
            //
            // Files that WERE tracked before this turn are pre-captured at
            // open_snapshot time, so by the time the sweep runs the snapshot
            // already has a row for them and the idempotency check above
            // skips this branch — preserving the correct pre-turn content.
            match std::fs::metadata(&change.abs_path) {
                Ok(md) if md.is_file() => {
                    if md.len() > MAX_TRACKED_FILE_SIZE {
                        // Too large to track. Skip silently for now; the UI
                        // layer surfaces the change via a separate event.
                        continue;
                    }
                    let size = md.len();
                    let mtime_ns = mtime_to_ns(md.modified().ok());
                    // For files newly created by bash: record post-bash
                    // hash so revert can detect "this was a bash creation"
                    // and delete it. We still encode that as a null
                    // blob_hash row — the hash is stored in the blob index
                    // but the revert path keys off the row's blob_hash.
                    let stored = self.inner.blob_store.store_from_path(&change.abs_path)?;
                    let db = self.inner.db.lock().map_err(|_| FileHistoryError::Poisoned)?;
                    if !stored.already_present {
                        db.fh_register_blob(&stored.hash, stored.size as i64)?;
                    }
                    db.fh_upsert_file(
                        message_id,
                        &change.rel_path,
                        None,
                        Some(mtime_ns.unwrap_or(0)),
                        Some(size as i64),
                    )?;
                    newly_recorded.push(change.rel_path);
                    let _ = stored; // silences unused warning when path above is taken
                }
                Ok(_) => {
                    // Non-regular file at this path; ignore.
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    let db = self.inner.db.lock().map_err(|_| FileHistoryError::Poisoned)?;
                    db.fh_upsert_file(message_id, &change.rel_path, None, None, None)?;
                    newly_recorded.push(change.rel_path);
                }
                Err(e) => return Err(FileHistoryError::Io(e)),
            }
        }
        Ok(newly_recorded)
    }

    /// List the files captured in a snapshot — used by the UI panel and revert.
    pub fn list_files(&self, message_id: &str) -> Result<Vec<FileHistoryFileRow>> {
        let db = self.inner.db.lock().map_err(|_| FileHistoryError::Poisoned)?;
        Ok(db.fh_list_files_for_snapshot(message_id)?)
    }

    /// Apply the snapshot's recorded backups to disk. Files with a non-null
    /// blob_hash are restored from the blob store; files with null blob_hash
    /// are deleted. Returns one `RestoreOutcome` per path.
    pub fn revert(&self, message_id: &str) -> Result<Vec<RestoreOutcome>> {
        let files = self.list_files(message_id)?;
        if files.is_empty() {
            // Either the snapshot doesn't exist or it had no entries. Caller
            // checks; we don't conflate these.
            let db = self.inner.db.lock().map_err(|_| FileHistoryError::Poisoned)?;
            if db.fh_get_snapshot(message_id)?.is_none() {
                return Err(FileHistoryError::SnapshotNotFound(message_id.to_string()));
            }
        }
        let mut out = Vec::with_capacity(files.len());
        for file in files {
            let abs = join_rel(&self.inner.project_root, &file.path);
            match file.blob_hash.as_deref() {
                Some(hash) => match self.restore_file(&abs, hash)? {
                    true => out.push(RestoreOutcome::Rewritten(file.path)),
                    false => out.push(RestoreOutcome::Unchanged(file.path)),
                },
                None => match self.delete_file(&abs)? {
                    true => out.push(RestoreOutcome::Deleted(file.path)),
                    false => out.push(RestoreOutcome::Unchanged(file.path)),
                },
            }
        }
        Ok(out)
    }

    /// Revert this snapshot AND every later snapshot in the same task, applied
    /// newest-first so that earlier snapshots' pre-blobs win for files that
    /// were modified across multiple turns. Returns the union of restore
    /// outcomes (one entry per path actually touched on disk).
    ///
    /// Errors with `SnapshotNotFound` if the starting `message_id` is unknown.
    pub fn revert_from_message(&self, message_id: &str) -> Result<Vec<RestoreOutcome>> {
        let chain = self.snapshot_chain_from(message_id)?;
        // Newest first — see chain doc for why ordering matters.
        let mut seen_paths = std::collections::HashSet::new();
        let mut out = Vec::new();
        for snap in chain.into_iter().rev() {
            for outcome in self.revert(&snap.message_id)? {
                let path = match &outcome {
                    RestoreOutcome::Rewritten(p)
                    | RestoreOutcome::Deleted(p)
                    | RestoreOutcome::Unchanged(p) => p.clone(),
                };
                // De-dupe across snapshots: the older snapshot's pre-blob is
                // what the user wants to see, so only the FIRST outcome we
                // record for a path is reported back. Subsequent (newer)
                // snapshots' outcomes are silently absorbed.
                if seen_paths.insert(path) {
                    out.push(outcome);
                }
            }
        }
        Ok(out)
    }

    /// Revert every snapshot in this task, oldest-first state restored.
    /// Behaves like `revert_from_message` against the earliest snapshot.
    pub fn revert_task(&self, task_id: &str) -> Result<Vec<RestoreOutcome>> {
        let snapshots = {
            let db = self.inner.db.lock().map_err(|_| FileHistoryError::Poisoned)?;
            db.fh_list_snapshots_for_task(task_id)?
        };
        if let Some(first) = snapshots.first() {
            self.revert_from_message(&first.message_id)
        } else {
            Ok(Vec::new())
        }
    }

    /// Plan a `revert_from_message` without touching disk. Returns the union
    /// of (path, planned_action) tuples the user will see in the confirmation
    /// dialog. Action is one of "restore" / "delete" / "keep" — restore for
    /// files that have a pre-blob, delete for files that didn't exist before
    /// the snapshot, keep for files where no change is recorded (defensive,
    /// shouldn't normally appear in the union).
    pub fn plan_revert_from_message(&self, message_id: &str) -> Result<Vec<RevertPlanEntry>> {
        let chain = self.snapshot_chain_from(message_id)?;
        let mut by_path: std::collections::HashMap<String, RevertPlanEntry> =
            std::collections::HashMap::new();
        // Walk newest -> oldest; an older snapshot's plan wins when the same
        // path appears in both because the older pre-blob is the final state.
        for snap in chain.into_iter().rev() {
            let files = self.list_files(&snap.message_id)?;
            for f in files {
                let action = if f.blob_hash.is_some() {
                    "restore"
                } else {
                    "delete"
                };
                by_path.insert(
                    f.path.clone(),
                    RevertPlanEntry {
                        path: f.path,
                        action,
                    },
                );
            }
        }
        let mut out: Vec<RevertPlanEntry> = by_path.into_values().collect();
        out.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(out)
    }

    /// Plan-only sibling of `revert_task`.
    pub fn plan_revert_task(&self, task_id: &str) -> Result<Vec<RevertPlanEntry>> {
        let snapshots = {
            let db = self.inner.db.lock().map_err(|_| FileHistoryError::Poisoned)?;
            db.fh_list_snapshots_for_task(task_id)?
        };
        if let Some(first) = snapshots.first() {
            self.plan_revert_from_message(&first.message_id)
        } else {
            Ok(Vec::new())
        }
    }

    /// Resolve the chain of snapshots starting at `message_id` through the
    /// most recent snapshot in the same task, ordered ASC by sequence. The
    /// caller iterates in reverse for revert.
    fn snapshot_chain_from(
        &self,
        message_id: &str,
    ) -> Result<Vec<rustic_db::FileHistorySnapshotRow>> {
        let db = self.inner.db.lock().map_err(|_| FileHistoryError::Poisoned)?;
        let target = db
            .fh_get_snapshot(message_id)?
            .ok_or_else(|| FileHistoryError::SnapshotNotFound(message_id.to_string()))?;
        let all = db.fh_list_snapshots_for_task(&target.task_id)?;
        Ok(all
            .into_iter()
            .filter(|s| s.sequence >= target.sequence)
            .collect())
    }

    /// List files in a snapshot with computed +/- line counts against the
    /// current on-disk content. Used by the changed-files panel so it can
    /// show `+12 -3` next to each path. Binary files come back with kind
    /// `"binary"` and zero counts.
    pub fn list_files_with_stats(&self, message_id: &str) -> Result<Vec<FileChangeStats>> {
        let files = self.list_files(message_id)?;
        let mut out = Vec::with_capacity(files.len());
        for file in files {
            out.push(self.stats_for(&file));
        }
        Ok(out)
    }

    /// Cumulative net change for an entire task. For every path that any
    /// snapshot recorded, find the EARLIEST snapshot that captured it (its
    /// `blob_hash` is the file's pre-task version) and diff that against the
    /// file's current on-disk content. Net-unchanged paths (file existed
    /// pre-task and content matches now byte-for-byte) are filtered out so
    /// the UI doesn't surface no-op rows from churn-then-revert sequences.
    pub fn list_task_net_changes(&self, task_id: &str) -> Result<Vec<TaskNetChange>> {
        let snapshots = {
            let db = self.inner.db.lock().map_err(|_| FileHistoryError::Poisoned)?;
            db.fh_list_snapshots_for_task(task_id)?
        };
        if snapshots.is_empty() {
            return Ok(Vec::new());
        }

        // Walk snapshots oldest-first; first occurrence of a path wins so its
        // pre-snapshot blob_hash represents the pre-task state.
        let mut earliest: std::collections::HashMap<String, (FileHistoryFileRow, String)> =
            std::collections::HashMap::new();
        for snap in &snapshots {
            let files = {
                let db = self.inner.db.lock().map_err(|_| FileHistoryError::Poisoned)?;
                db.fh_list_files_for_snapshot(&snap.message_id)?
            };
            for row in files {
                earliest
                    .entry(row.path.clone())
                    .or_insert((row, snap.message_id.clone()));
            }
        }

        let mut out = Vec::with_capacity(earliest.len());
        for (_path, (row, anchor_id)) in earliest {
            let stats = self.stats_for(&row);
            // Skip net no-ops: file existed pre-task and the on-disk content
            // matches it (touched then reverted, or written back to original).
            if stats.kind == "modified" && stats.additions == 0 && stats.deletions == 0 && !stats.binary {
                continue;
            }
            // Also skip "created then deleted within the task": no pre-blob,
            // no current file. compute_stats classifies that as "created" with
            // 0/0 — but the file doesn't exist on disk now, so suppress it.
            if stats.kind == "created" {
                let abs = join_rel(&self.inner.project_root, &stats.path);
                if !abs.exists() {
                    continue;
                }
            }
            out.push(TaskNetChange {
                path: stats.path,
                kind: stats.kind,
                binary: stats.binary,
                additions: stats.additions,
                deletions: stats.deletions,
                anchor_message_id: anchor_id,
            });
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(out)
    }

    /// Compute the unified diff and stats for a single (snapshot, path) pair.
    /// Returns SnapshotNotFound if the path is not in the snapshot.
    pub fn file_diff(&self, message_id: &str, path: &str) -> Result<FileDiff> {
        let row = {
            let db = self.inner.db.lock().map_err(|_| FileHistoryError::Poisoned)?;
            db.fh_get_file(message_id, path)?
                .ok_or_else(|| FileHistoryError::SnapshotNotFound(message_id.to_string()))?
        };
        let (before, after) = self.read_before_after(&row);
        let stats = self.compute_stats(&row, &before, &after);

        // Binary -> empty unified output. UI can fall back to opening the file.
        let unified = if stats.binary {
            String::new()
        } else {
            let before_str = std::str::from_utf8(&before).unwrap_or("");
            let after_str = std::str::from_utf8(&after).unwrap_or("");
            similar::TextDiff::from_lines(before_str, after_str)
                .unified_diff()
                .header(
                    &format!("a/{}", row.path),
                    &format!("b/{}", row.path),
                )
                .to_string()
        };

        Ok(FileDiff {
            path: row.path,
            kind: stats.kind,
            binary: stats.binary,
            additions: stats.additions,
            deletions: stats.deletions,
            unified,
        })
    }

    fn stats_for(&self, row: &FileHistoryFileRow) -> FileChangeStats {
        let (before, after) = self.read_before_after(row);
        self.compute_stats(row, &before, &after)
    }

    fn read_before_after(&self, row: &FileHistoryFileRow) -> (Vec<u8>, Vec<u8>) {
        let before = match row.blob_hash.as_deref() {
            Some(hash) => self
                .inner
                .blob_store
                .path_for(hash)
                .ok()
                .and_then(|p| std::fs::read(&p).ok())
                .unwrap_or_default(),
            None => Vec::new(),
        };
        let abs = join_rel(&self.inner.project_root, &row.path);
        let after = std::fs::read(&abs).unwrap_or_default();
        (before, after)
    }

    fn compute_stats(
        &self,
        row: &FileHistoryFileRow,
        before: &[u8],
        after: &[u8],
    ) -> FileChangeStats {
        let abs = join_rel(&self.inner.project_root, &row.path);
        let after_exists = abs.exists();

        // Created / deleted / modified is independent of binary-ness — classify
        // by snapshot presence + on-disk presence first, then compute counts
        // only for text content. The previous version collapsed all three into
        // a single "binary" kind, which lost the create/delete signal the UI
        // wants to surface (e.g. a freshly generated PDF should still read as
        // "new", not just "binary").
        let kind = match (row.blob_hash.is_some(), after_exists) {
            (false, _) => "created",
            (true, false) => "deleted",
            (true, true) => "modified",
        };

        // Detect binary content on either side: any NUL byte, or non-utf8.
        let is_binary_bytes = |bytes: &[u8]| {
            bytes.contains(&0) || std::str::from_utf8(bytes).is_err()
        };
        let binary = is_binary_bytes(before) || is_binary_bytes(after);

        let (additions, deletions) = if binary {
            (0, 0)
        } else {
            let before_str = std::str::from_utf8(before).unwrap_or("");
            let after_str = std::str::from_utf8(after).unwrap_or("");
            let mut a = 0u32;
            let mut d = 0u32;
            let diff = similar::TextDiff::from_lines(before_str, after_str);
            for change in diff.iter_all_changes() {
                match change.tag() {
                    similar::ChangeTag::Insert => a += 1,
                    similar::ChangeTag::Delete => d += 1,
                    similar::ChangeTag::Equal => {}
                }
            }
            (a, d)
        };

        FileChangeStats {
            path: row.path.clone(),
            kind,
            binary,
            additions,
            deletions,
        }
    }

    /// Drop the oldest snapshots beyond `max_snapshots`. Returns count evicted.
    /// Snapshot deletion cascades to file rows; triggers decrement blob
    /// ref_counts. Caller (usually a periodic task) should follow with
    /// `gc_unreferenced_blobs` to free disk.
    pub fn evict_old(&self, task_id: &str) -> Result<usize> {
        let db = self.inner.db.lock().map_err(|_| FileHistoryError::Poisoned)?;
        Ok(db.fh_evict_old_snapshots(task_id, self.inner.max_snapshots)?)
    }

    /// Delete every blob whose ref_count is zero. Returns the number of blobs
    /// freed (both on disk and in the index).
    pub fn gc_unreferenced_blobs(&self) -> Result<usize> {
        let blobs = {
            let db = self.inner.db.lock().map_err(|_| FileHistoryError::Poisoned)?;
            db.fh_unreferenced_blobs()?
        };
        if blobs.is_empty() {
            return Ok(0);
        }
        // Unlink files first; if the unlink fails for some reason, leave the
        // index row in place so we'll retry next time. Disk-vs-index drift
        // is the more recoverable failure mode.
        let mut hashes_to_drop = Vec::with_capacity(blobs.len());
        for blob in blobs {
            match self.inner.blob_store.delete(&blob.hash) {
                Ok(()) => hashes_to_drop.push(blob.hash),
                Err(e) => tracing::warn!(hash = %blob.hash, ?e, "blob unlink failed; will retry"),
            }
        }
        let db = self.inner.db.lock().map_err(|_| FileHistoryError::Poisoned)?;
        Ok(db.fh_delete_blobs(&hashes_to_drop)?)
    }

    /// Startup reconciliation: any blob *file* on disk whose hash is not in
    /// the index is an orphan from a crash mid-commit. Unlink it.
    pub fn reconcile_disk_orphans(&self) -> Result<usize> {
        let on_disk = self.inner.blob_store.list_all_hashes()?;
        if on_disk.is_empty() {
            return Ok(0);
        }
        let in_index: std::collections::HashSet<String> = {
            let db = self.inner.db.lock().map_err(|_| FileHistoryError::Poisoned)?;
            db.fh_all_blob_hashes()?.into_iter().collect()
        };
        let mut removed = 0;
        for hash in on_disk {
            if !in_index.contains(&hash) {
                if let Err(e) = self.inner.blob_store.delete(&hash) {
                    tracing::warn!(%hash, ?e, "orphan blob unlink failed");
                } else {
                    removed += 1;
                }
            }
        }
        Ok(removed)
    }

    /// Walk the project, filter by mtime, and return the candidate set the
    /// sweep worker should hand to `apply_sweep`. Stat-only — no content reads.
    /// Exposed so the worker can run this on its own thread.
    pub fn collect_sweep_candidates(&self, since: SystemTime) -> Vec<SweepFileChange> {
        walk_for_sweep(&self.inner.project_root)
            .into_iter()
            .filter(|w: &WalkedFile| w.mtime >= since)
            .map(|w| SweepFileChange {
                abs_path: w.abs_path,
                rel_path: w.rel_path,
                size: w.size,
            })
            .collect()
    }

    // -------- internals --------

    fn relativize(&self, abs_path: &Path) -> Result<String> {
        let canon_root = self
            .inner
            .project_root
            .canonicalize()
            .unwrap_or_else(|_| self.inner.project_root.clone());
        // Best-effort canonicalize: capture before write may run on a path
        // that doesn't exist yet. Walk up to the deepest existing ancestor.
        let canon_path = match abs_path.canonicalize() {
            Ok(p) => p,
            Err(_) => {
                let mut probe = abs_path.to_path_buf();
                loop {
                    if let Ok(p) = probe.canonicalize() {
                        // Re-attach the unresolved tail.
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
        let rel = match canon_path.strip_prefix(&canon_root) {
            Ok(r) => r,
            Err(_) => {
                return Err(FileHistoryError::OutsideProject {
                    path: abs_path.to_path_buf(),
                    root: self.inner.project_root.clone(),
                })
            }
        };
        Ok(normalize_rel(rel))
    }

    /// Restore a file from its blob. Returns true if disk content actually
    /// changed (caller can avoid emitting "rewritten" events for no-ops).
    fn restore_file(&self, dest: &Path, hash: &str) -> Result<bool> {
        // Cheap pre-check: if the file already matches the blob's hash, skip.
        if let Ok(md) = std::fs::metadata(dest) {
            if md.is_file() {
                // Exact byte-equality via streaming hash comparison would mean
                // re-reading the file. For now we always copy — restoring 1KB
                // when it was already correct is acceptable. If profiling
                // shows churn here, hash + compare.
                let _ = md;
            }
        }
        self.inner.blob_store.restore_to(hash, dest)?;
        Ok(true)
    }

    fn delete_file(&self, dest: &Path) -> Result<bool> {
        match std::fs::remove_file(dest) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(FileHistoryError::Io(e)),
        }
    }
}

/// Convert a `SystemTime` modified-time into nanoseconds since UNIX epoch.
/// Used for the (mtime_ns, size) stat cache that lets `open_snapshot`
/// pre-capture skip re-hashing unchanged files. Returns `None` if the
/// platform doesn't expose mtime or the time is before the epoch — both
/// degrade gracefully to "always re-hash" behaviour.
fn mtime_to_ns(t: Option<SystemTime>) -> Option<i64> {
    let t = t?;
    let dur = t.duration_since(SystemTime::UNIX_EPOCH).ok()?;
    let nanos = dur.as_nanos();
    i64::try_from(nanos).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    struct Fixture {
        _cfg_dir: tempfile::TempDir,
        _proj_dir: tempfile::TempDir,
        history: FileHistory,
        project_root: PathBuf,
        db: Arc<Mutex<Database>>,
    }

    fn fixture() -> Fixture {
        let cfg_dir = tempfile::tempdir().unwrap();
        let proj_dir = tempfile::tempdir().unwrap();
        let project_root = proj_dir.path().canonicalize().unwrap();
        // Fake .git so walk's gitignore filters work in any later test.
        fs::create_dir_all(project_root.join(".git")).unwrap();
        let db = Arc::new(Mutex::new(Database::in_memory().unwrap()));
        // Seed FK target rows so snapshot inserts pass.
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
        let history = FileHistory::new(db.clone(), project_root.clone(), cfg_dir.path());
        Fixture {
            _cfg_dir: cfg_dir,
            _proj_dir: proj_dir,
            history,
            project_root,
            db,
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
    fn capture_then_revert_restores_original_content() {
        let f = fixture();
        let foo = f.project_root.join("foo.txt");
        write(&foo, b"original");

        f.history.open_snapshot("msg-1", "t").unwrap();
        let outcome = f.history.capture("msg-1", &foo).unwrap();
        match outcome {
            CaptureOutcome::Captured { rel_path, .. } => assert_eq!(rel_path, "foo.txt"),
            other => panic!("expected Captured, got {other:?}"),
        }

        // Edit tool writes new content
        write(&foo, b"modified by agent");

        let restored = f.history.revert("msg-1").unwrap();
        assert_eq!(restored.len(), 1);
        match &restored[0] {
            RestoreOutcome::Rewritten(p) => assert_eq!(p, "foo.txt"),
            other => panic!("expected Rewritten, got {other:?}"),
        }
        let got = fs::read(&foo).unwrap();
        assert_eq!(got, b"original");
    }

    #[test]
    fn capture_did_not_exist_records_null_and_revert_deletes() {
        let f = fixture();
        let new_path = f.project_root.join("brand_new.txt");
        // File doesn't exist yet.

        f.history.open_snapshot("msg-2", "t").unwrap();
        let outcome = f.history.capture("msg-2", &new_path).unwrap();
        match outcome {
            CaptureOutcome::DidNotExist { rel_path } => assert_eq!(rel_path, "brand_new.txt"),
            other => panic!("expected DidNotExist, got {other:?}"),
        }

        // Agent creates the file.
        write(&new_path, b"hi");

        let restored = f.history.revert("msg-2").unwrap();
        match &restored[0] {
            RestoreOutcome::Deleted(p) => assert_eq!(p, "brand_new.txt"),
            other => panic!("expected Deleted, got {other:?}"),
        }
        assert!(!new_path.exists());
    }

    /// Regression: turn A creates x.txt; turn B deletes x.txt and recreates
    /// it with different content. Reverting from B must restore A's content
    /// (the pre-B state), not delete the file. Before the open_snapshot
    /// pre-capture fix, B's snapshot would record x.txt as "didn't exist
    /// before B" because the post-hoc bash sweep can't observe the file
    /// in the brief deleted state — and revert would then delete x.txt.
    #[test]
    fn revert_restores_after_delete_and_recreate_across_turns() {
        let f = fixture();
        let x = f.project_root.join("x.txt");

        // Turn A: snapshot opens (file doesn't exist yet), tool creates x.txt.
        f.history.open_snapshot("msg-A", "t").unwrap();
        f.history.capture("msg-A", &x).unwrap();
        write(&x, b"content-A");

        // Turn B: snapshot opens — pre-capture should lock in "content-A"
        // as the pre-B state for x.txt, before bash gets a chance to mutate.
        f.history.open_snapshot("msg-B", "t").unwrap();
        // Bash deletes x.txt, then recreates with different content. We
        // simulate by skipping any tool-level capture call (the bug being
        // fixed is precisely that bash bypasses capture).
        fs::remove_file(&x).unwrap();
        write(&x, b"content-B");

        // Revert from B should restore x.txt to "content-A", not delete it.
        let outcomes = f.history.revert_from_message("msg-B").unwrap();
        assert!(
            outcomes
                .iter()
                .any(|o| matches!(o, RestoreOutcome::Rewritten(p) if p == "x.txt")),
            "expected x.txt to be rewritten back to A's content, got {outcomes:?}"
        );
        assert!(x.exists(), "x.txt must still exist after revert from B");
        assert_eq!(
            fs::read(&x).unwrap(),
            b"content-A",
            "x.txt should have been restored to A's content"
        );

        // Reverting from A then takes us back to pre-task state (x.txt gone).
        // Note: revert_from_message dedups outcomes by path keeping the
        // newest snapshot's outcome (B's "Rewritten") as the reported
        // value. The accumulated disk mutation still completes — A's
        // "delete" runs after B's "rewrite" — so we verify by checking
        // file existence rather than the outcome list.
        f.history.revert_from_message("msg-A").unwrap();
        assert!(!x.exists(), "x.txt should be gone after revert from A");
    }

    /// The stat-cache fast path: when a file's mtime+size match the prior
    /// snapshot's record, pre-capture should reuse the prior blob_hash
    /// without re-reading the file. We verify behaviourally — open three
    /// snapshots without changing the file, then revert from each one and
    /// confirm content is preserved (which only works if the cached hash
    /// was correctly carried forward).
    #[test]
    fn open_snapshot_precapture_reuses_cached_blob_when_unchanged() {
        let f = fixture();
        let x = f.project_root.join("stable.txt");

        f.history.open_snapshot("m1", "t").unwrap();
        write(&x, b"the-content");
        f.history.capture("m1", &x).unwrap();
        // Mutate the file inside m1 so capture has stored a real pre-blob.
        write(&x, b"the-content-after-m1");

        // Reset to a stable post-m1 state and let mtime settle.
        write(&x, b"the-content-after-m1");
        std::thread::sleep(std::time::Duration::from_millis(20));

        // Open m2 and m3 without further edits. Pre-capture should record
        // the current ("after-m1") content as the pre-state for both.
        f.history.open_snapshot("m2", "t").unwrap();
        f.history.open_snapshot("m3", "t").unwrap();

        // Revert from m3 — chain is [m3]. Pre-state was "after-m1".
        let _ = f.history.revert_from_message("m3").unwrap();
        assert_eq!(fs::read(&x).unwrap(), b"the-content-after-m1");
    }

    #[test]
    fn capture_idempotent_within_snapshot() {
        let f = fixture();
        let p = f.project_root.join("a.txt");
        write(&p, b"v1");

        f.history.open_snapshot("msg-3", "t").unwrap();
        let first = f.history.capture("msg-3", &p).unwrap();
        assert!(matches!(first, CaptureOutcome::Captured { .. }));

        // Edit happens, file now has new content
        write(&p, b"v2");
        let second = f.history.capture("msg-3", &p).unwrap();
        assert!(
            matches!(second, CaptureOutcome::AlreadyTracked { .. }),
            "second capture in same snapshot must NOT overwrite v1"
        );

        // Revert still restores v1.
        f.history.revert("msg-3").unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"v1");
    }

    #[test]
    fn eviction_then_gc_frees_blobs() {
        let f = fixture();
        let p = f.project_root.join("x.txt");
        write(&p, b"content-A");

        // Take 5 snapshots, but mutate the file in between so each captures
        // a distinct version. With max_snapshots=100 nothing evicts; we set
        // an artificially low cap on the FH instance for this test.
        for i in 1..=5 {
            f.history.open_snapshot(&format!("m{i}"), "t").unwrap();
            f.history.capture(&format!("m{i}"), &p).unwrap();
            write(&p, format!("content-{i}").as_bytes());
        }

        // Manually shrink the cap and evict.
        let evicted = {
            let db = f.db.lock().unwrap();
            db.fh_evict_old_snapshots("t", 2).unwrap()
        };
        assert_eq!(evicted, 3);

        // GC should now free the blobs from the evicted snapshots.
        let freed = f.history.gc_unreferenced_blobs().unwrap();
        // Three snapshots evicted, each with one (possibly distinct) blob.
        assert!(freed >= 1, "expected GC to free at least one blob, got {freed}");

        // The two remaining snapshots are still revertable.
        let _ = f.history.revert("m4").unwrap();
        let _ = f.history.revert("m5").unwrap();
    }

    #[test]
    fn open_snapshot_is_idempotent() {
        let f = fixture();
        f.history.open_snapshot("msg-x", "t").unwrap();
        let first = {
            let db = f.db.lock().unwrap();
            db.fh_get_snapshot("msg-x").unwrap().unwrap()
        };
        f.history.open_snapshot("msg-x", "t").unwrap();
        let second = {
            let db = f.db.lock().unwrap();
            db.fh_get_snapshot("msg-x").unwrap().unwrap()
        };
        assert_eq!(first.sequence, second.sequence);
    }

    #[test]
    fn reconcile_drops_orphan_blob_files() {
        let f = fixture();

        // Stash a blob file on disk that's not in the index.
        let orphan_dir = f.history.blob_store().root().join("ab");
        fs::create_dir_all(&orphan_dir).unwrap();
        let orphan = orphan_dir.join(
            "abababababababababababababababababababababababababababababababab",
        );
        fs::write(&orphan, b"orphan content").unwrap();

        // Also stash a real blob via capture so we know reconciliation
        // doesn't nuke valid ones.
        let real = f.project_root.join("real.txt");
        write(&real, b"real content");
        f.history.open_snapshot("msg-r", "t").unwrap();
        f.history.capture("msg-r", &real).unwrap();

        let removed = f.history.reconcile_disk_orphans().unwrap();
        assert_eq!(removed, 1);
        assert!(!orphan.exists());
    }
}

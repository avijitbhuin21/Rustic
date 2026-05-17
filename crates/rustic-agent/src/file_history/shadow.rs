//! Libgit2-backed shadow snapshot store.
//!
//! Bare git repo at `<shadow_root>/<project_hash>/` storing tree+blob objects
//! only (no commits, no refs). Callers record per-message tree oids in SQLite
//! and use this module to diff/restore against them.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use git2::{IndexEntry, IndexTime, Oid, Repository};
use sha2::{Digest, Sha256};
use thiserror::Error;

use super::walk::{join_rel, walk_for_sweep};

/// Hard cap for tracked files. Anything larger is skipped and surfaced to
/// the caller via `TrackResult::files_too_large`. Plan-locked value from
/// `docs/file_tracking_decision.md` §0.
pub const MAX_TRACKED_FILE_SIZE: u64 = 50 * 1024 * 1024; // 50 MiB

/// Soft threshold for the synchronous capture tier. The shadow itself
/// captures both small and medium files; this constant exists for
/// upper-layer code (tracker) to decide which calls to push onto a
/// blocking thread so the agent path stays under ~5 ms.
pub const SYNC_CAPTURE_SOFT_LIMIT: u64 = 5 * 1024 * 1024; // 5 MiB

#[derive(Debug, Error)]
pub enum ShadowError {
    #[error("git: {0}")]
    Git(#[from] git2::Error),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("shadow repo mutex poisoned")]
    Poisoned,
}

pub type Result<T> = std::result::Result<T, ShadowError>;

/// Result of a single `track()` call. `tree_oid` is the checkpoint id;
/// `files_too_large` lists paths that exceeded the size cap so the caller
/// can surface them to the UI (today's tracker has the same `TooLarge`
/// signal in `CaptureOutcome`).
#[derive(Debug, Clone)]
pub struct TrackResult {
    pub tree_oid: Oid,
    pub files_tracked: usize,
    pub files_too_large: Vec<(String, u64)>,
}

/// Shadow snapshot for one canonical worktree. Cheap to clone (`Arc`-wrapped
/// internally is the expected pattern — see `for_worktree`). All operations
/// serialize on a per-shadow mutex; the mutex covers the libgit2 repository
/// handle which is itself `!Sync`.
pub struct ShadowSnapshot {
    repo: Mutex<Repository>,
    worktree_root: PathBuf,
    repo_path: PathBuf,
}

impl ShadowSnapshot {
    /// Open or initialize the shadow repo for `worktree_root` under
    /// `shadow_root`. Returns an `Arc` so callers can share the snapshot
    /// across tasks (the registry pattern in §6.1 of the decision doc).
    ///
    /// The repo lives at `<shadow_root>/<project_hash>/` where
    /// `project_hash` is a 32-char hex slice derived from the canonical
    /// worktree path (case-folded on Windows to keep `C:\Foo` and
    /// `c:\foo` colliding into one entry).
    pub fn for_worktree(worktree_root: &Path, shadow_root: &Path) -> Result<Arc<Self>> {
        let canon_root = worktree_root
            .canonicalize()
            .unwrap_or_else(|_| worktree_root.to_path_buf());
        let project_hash = compute_project_hash(&canon_root);
        let repo_path = shadow_root.join(&project_hash);
        fs::create_dir_all(&repo_path)?;
        let repo = match Repository::open_bare(&repo_path) {
            Ok(r) => r,
            Err(_) => Repository::init_bare(&repo_path)?,
        };
        Ok(Arc::new(Self {
            repo: Mutex::new(repo),
            worktree_root: canon_root,
            repo_path,
        }))
    }

    pub fn worktree_root(&self) -> &Path {
        &self.worktree_root
    }

    pub fn repo_path(&self) -> &Path {
        &self.repo_path
    }

    /// Walk the worktree, write a blob for every non-excluded file, build
    /// an index, and persist it as a tree. Returns the tree oid plus
    /// bookkeeping. Files above `MAX_TRACKED_FILE_SIZE` are skipped and
    /// reported in `files_too_large`.
    ///
    /// This is synchronous and holds the shadow's mutex for the duration
    /// of the walk + blob writes. Concurrent callers serialize behind it.
    /// For a 50k-file repo with mostly stat-clean cache hits the cost is
    /// dominated by the walk (libgit2's odb dedups identical content into
    /// the same packfile entry on the next `gc`, so re-writing unchanged
    /// blobs is cheap).
    pub fn track(&self) -> Result<TrackResult> {
        let walked = walk_for_sweep(&self.worktree_root);

        let repo_guard = self.repo.lock().map_err(|_| ShadowError::Poisoned)?;
        let repo = &*repo_guard;
        // Use the bare repo's index (libgit2 happily synthesises one even
        // for bare repos) and clear it at the start of each track(). A
        // standalone `Index::new()` is rejected by `add_frombuffer`
        // ("Index is not backed up by an existing repository") because
        // the index needs an odb to spill the blob to.
        let mut index = repo.index()?;
        index.clear()?;
        let mut tracked = 0usize;
        let mut too_large = Vec::new();

        for file in walked {
            if file.size > MAX_TRACKED_FILE_SIZE {
                too_large.push((file.rel_path.clone(), file.size));
                continue;
            }
            let content = match fs::read(&file.abs_path) {
                Ok(c) => c,
                Err(e) => {
                    tracing::debug!(
                        path = %file.abs_path.display(),
                        ?e,
                        "shadow track: read failed; skipping"
                    );
                    continue;
                }
            };
            let entry = make_index_entry(&file.rel_path, content.len() as u32);
            index.add_frombuffer(&entry, &content)?;
            tracked += 1;
        }

        let tree_oid = index.write_tree()?;
        Ok(TrackResult {
            tree_oid,
            files_tracked: tracked,
            files_too_large: too_large,
        })
    }

    /// Convenience for "what's changed in the worktree since tree `from`?".
    /// Builds the current tree via `track()` and diffs path-only.
    pub fn patch(&self, from: Oid) -> Result<Vec<String>> {
        let current = self.track()?;
        self.diff_paths(from, current.tree_oid)
    }

    /// List the paths that differ between two stored trees. Cheap — no
    /// blob reads, just walks the diff entry headers.
    pub fn diff_paths(&self, from: Oid, to: Oid) -> Result<Vec<String>> {
        let repo = self.repo.lock().map_err(|_| ShadowError::Poisoned)?;
        let from_tree = repo.find_tree(from)?;
        let to_tree = repo.find_tree(to)?;
        let diff = repo.diff_tree_to_tree(Some(&from_tree), Some(&to_tree), None)?;
        let mut paths = Vec::new();
        for delta in diff.deltas() {
            let p = delta
                .new_file()
                .path()
                .or_else(|| delta.old_file().path())
                .map(|p| p.to_string_lossy().into_owned());
            if let Some(p) = p {
                paths.push(p);
            }
        }
        Ok(paths)
    }

    /// Unified-diff text between two stored trees. Suitable for handing
    /// to the diff renderer in the chat-view's file pane.
    pub fn diff(&self, from: Oid, to: Oid) -> Result<String> {
        let repo = self.repo.lock().map_err(|_| ShadowError::Poisoned)?;
        let from_tree = repo.find_tree(from)?;
        let to_tree = repo.find_tree(to)?;
        let diff = repo.diff_tree_to_tree(Some(&from_tree), Some(&to_tree), None)?;
        let mut out = String::new();
        diff.print(git2::DiffFormat::Patch, |_delta, _hunk, line| {
            match line.origin() {
                '+' | '-' | ' ' => out.push(line.origin()),
                _ => {}
            }
            out.push_str(std::str::from_utf8(line.content()).unwrap_or(""));
            true
        })?;
        Ok(out)
    }

    /// Per-path stats for the diff between two stored trees: status
    /// (Added/Modified/Deleted), binary flag, added/deleted line counts.
    /// Replaces the per-path `similar`-on-materialized-bytes loop the
    /// tracker used to run — same observable output, but one libgit2
    /// pass instead of N blob reads + N text-diff invocations.
    pub fn diff_full(&self, from: Oid, to: Oid) -> Result<Vec<ShadowFileChange>> {
        let repo = self.repo.lock().map_err(|_| ShadowError::Poisoned)?;
        let from_tree = repo.find_tree(from)?;
        let to_tree = repo.find_tree(to)?;
        let diff = repo.diff_tree_to_tree(Some(&from_tree), Some(&to_tree), None)?;

        let mut out = Vec::with_capacity(diff.deltas().count());
        for idx in 0..diff.deltas().count() {
            let delta = match diff.get_delta(idx) {
                Some(d) => d,
                None => continue,
            };
            let path = delta
                .new_file()
                .path()
                .or_else(|| delta.old_file().path())
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default();
            let status = match delta.status() {
                git2::Delta::Added | git2::Delta::Untracked => ShadowChangeStatus::Added,
                git2::Delta::Deleted => ShadowChangeStatus::Deleted,
                _ => ShadowChangeStatus::Modified,
            };

            // libgit2 marks BINARY only after parsing the patch — building
            // it gives us both the binary flag and the line counts in one
            // pass. A failed Patch::from_diff (rare; only on malformed
            // trees) falls back to zero counts and non-binary so the UI
            // still gets a row.
            let (binary, additions, deletions) = match git2::Patch::from_diff(&diff, idx)? {
                Some(patch) => {
                    let is_binary = patch.delta().flags().contains(git2::DiffFlags::BINARY);
                    if is_binary {
                        (true, 0, 0)
                    } else {
                        let (_ctx, adds, dels) = patch.line_stats()?;
                        (false, adds as u32, dels as u32)
                    }
                }
                None => (
                    delta.flags().contains(git2::DiffFlags::BINARY),
                    0,
                    0,
                ),
            };

            out.push(ShadowFileChange {
                path,
                status,
                binary,
                additions,
                deletions,
            });
        }
        Ok(out)
    }

    /// Unified-diff text for one path between two trees. Returns
    /// `Ok(None)` if the path is unchanged or absent in both. The output
    /// includes the canonical `diff --git ... / --- a/path / +++ b/path`
    /// header so frontend diff renderers behave the same as they do on
    /// real git output.
    pub fn unified_diff_for_path(
        &self,
        from: Oid,
        to: Oid,
        path: &str,
    ) -> Result<Option<String>> {
        let repo = self.repo.lock().map_err(|_| ShadowError::Poisoned)?;
        let from_tree = repo.find_tree(from)?;
        let to_tree = repo.find_tree(to)?;
        let mut opts = git2::DiffOptions::new();
        opts.pathspec(path);
        let diff =
            repo.diff_tree_to_tree(Some(&from_tree), Some(&to_tree), Some(&mut opts))?;

        let mut out = String::new();
        let mut had_content = false;
        for idx in 0..diff.deltas().count() {
            let mut patch = match git2::Patch::from_diff(&diff, idx)? {
                Some(p) => p,
                None => continue,
            };
            let buf = patch.to_buf()?;
            if let Ok(text) = std::str::from_utf8(&buf) {
                out.push_str(text);
                had_content = true;
            }
        }
        Ok(if had_content { Some(out) } else { None })
    }

    /// Materialize the content of `path` as it was recorded in `tree`.
    /// Returns the blob bytes or `None` if the path is absent from that
    /// tree. Used by the higher-level `file_diff` to render before/after.
    pub fn read_path(&self, tree: Oid, path: &str) -> Result<Option<Vec<u8>>> {
        let repo = self.repo.lock().map_err(|_| ShadowError::Poisoned)?;
        let tree_obj = repo.find_tree(tree)?;
        let entry = match tree_obj.get_path(Path::new(path)) {
            Ok(e) => e,
            Err(e) if e.code() == git2::ErrorCode::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let blob = repo.find_blob(entry.id())?;
        Ok(Some(blob.content().to_vec()))
    }

    /// List every path stored in `tree`. Used by `TaskNetChange` and
    /// revert plans to enumerate the per-snapshot file set.
    pub fn list_paths(&self, tree: Oid) -> Result<Vec<String>> {
        let repo = self.repo.lock().map_err(|_| ShadowError::Poisoned)?;
        let tree_obj = repo.find_tree(tree)?;
        let mut paths = Vec::new();
        tree_obj.walk(git2::TreeWalkMode::PreOrder, |dir, entry| {
            if entry.kind() == Some(git2::ObjectType::Blob) {
                let name = entry.name().unwrap_or("");
                let full = if dir.is_empty() {
                    name.to_string()
                } else {
                    format!("{dir}{name}")
                };
                paths.push(full);
            }
            git2::TreeWalkResult::Ok
        })?;
        Ok(paths)
    }

    /// Restore `path` to its state in `tree`. If `tree` contains the path,
    /// write the recorded blob to disk (creating parent dirs as needed).
    /// If `tree` does not contain the path, delete the on-disk file when
    /// present. The returned `ShadowRestoreAction` mirrors what the UI
    /// surfaces in revert previews.
    pub fn restore_path(&self, tree: Oid, path: &str) -> Result<ShadowRestoreAction> {
        let blob_bytes = self.read_path(tree, path)?;
        self.apply_restore(path, blob_bytes)
    }

    /// Batched variant of `restore_path` — opens `tree` once, walks all
    /// requested paths, applies each. Cheaper than N calls when the
    /// caller (revert) is restoring many paths from the same snapshot.
    pub fn restore_paths_from(
        &self,
        tree: Oid,
        paths: &[String],
    ) -> Result<Vec<ShadowRestoreAction>> {
        let blobs: Vec<(String, Option<Vec<u8>>)> = {
            let repo = self.repo.lock().map_err(|_| ShadowError::Poisoned)?;
            let tree_obj = repo.find_tree(tree)?;
            let mut out = Vec::with_capacity(paths.len());
            for path in paths {
                let bytes = match tree_obj.get_path(Path::new(path)) {
                    Ok(entry) => Some(repo.find_blob(entry.id())?.content().to_vec()),
                    Err(e) if e.code() == git2::ErrorCode::NotFound => None,
                    Err(e) => return Err(e.into()),
                };
                out.push((path.clone(), bytes));
            }
            out
        };
        // Drop the repo lock before touching disk — these writes can be
        // arbitrarily slow and serializing on the mutex would block any
        // other shadow op (notably the sweep worker calling `track()`).
        blobs
            .into_iter()
            .map(|(path, bytes)| self.apply_restore(&path, bytes))
            .collect()
    }

    fn apply_restore(
        &self,
        path: &str,
        bytes: Option<Vec<u8>>,
    ) -> Result<ShadowRestoreAction> {
        let abs = join_rel(&self.worktree_root, path);
        match bytes {
            Some(content) => {
                // Skip the write if the disk content already matches the
                // tree's blob byte-for-byte. Cheap optimisation that turns
                // a no-op revert from N writes into N stats.
                if let Ok(disk) = fs::read(&abs) {
                    if disk == content {
                        return Ok(ShadowRestoreAction::NoOp {
                            path: path.to_string(),
                        });
                    }
                }
                if let Some(parent) = abs.parent() {
                    fs::create_dir_all(parent)?;
                }
                let bytes_written = content.len() as u64;
                fs::write(&abs, &content)?;
                Ok(ShadowRestoreAction::Wrote {
                    path: path.to_string(),
                    bytes: bytes_written,
                })
            }
            None => match fs::remove_file(&abs) {
                Ok(()) => Ok(ShadowRestoreAction::Deleted {
                    path: path.to_string(),
                }),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    Ok(ShadowRestoreAction::NoOp {
                        path: path.to_string(),
                    })
                }
                Err(e) => Err(ShadowError::Io(e)),
            },
        }
    }

    /// Reachability-based GC. `keep_trees` is the set of tree oids the
    /// caller wants preserved — typically every `tree_oid` in the
    /// `file_history_snapshots_v2` metadata table. Loose objects in the
    /// shadow's ODB that are unreachable from any keep tree AND whose
    /// loose file is older than `prune_horizon` are removed.
    ///
    /// Pass `prune_horizon = Duration::ZERO` to prune everything
    /// unreachable regardless of age (useful for tests / explicit
    /// "delete everything for this task" flows). Production callers
    /// should pass `Duration::from_secs(7 * 24 * 3600)` per the
    /// 7-day grace window locked in `docs/file_tracking_decision.md` §0.
    ///
    /// Returns the count of objects pruned.
    pub fn cleanup(
        &self,
        keep_trees: &[Oid],
        prune_horizon: Duration,
    ) -> Result<usize> {
        let repo = self.repo.lock().map_err(|_| ShadowError::Poisoned)?;

        let mut reachable: HashSet<Oid> = HashSet::new();
        for &tree_oid in keep_trees {
            collect_reachable(&repo, tree_oid, &mut reachable)?;
        }

        let cutoff = SystemTime::now()
            .checked_sub(prune_horizon)
            .unwrap_or(SystemTime::UNIX_EPOCH);

        let objects_dir = self.repo_path.join("objects");
        let mut pruned = 0usize;
        // Loose object layout: objects/<aa>/<bbbb...> — first two hex chars
        // are the bucket directory. Skip `info/`, `pack/`, and anything
        // that doesn't look like a bucket.
        let bucket_iter = match fs::read_dir(&objects_dir) {
            Ok(it) => it,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(e) => return Err(ShadowError::Io(e)),
        };
        for bucket in bucket_iter {
            let bucket = bucket?;
            let bucket_name = bucket.file_name();
            let bucket_str = bucket_name.to_string_lossy();
            if bucket_str.len() != 2 || !bucket_str.chars().all(|c| c.is_ascii_hexdigit()) {
                continue;
            }
            for obj in fs::read_dir(bucket.path())? {
                let obj = obj?;
                let obj_path = obj.path();
                let file_name = obj.file_name();
                let suffix = file_name.to_string_lossy();
                if suffix.len() != 38 {
                    continue;
                }
                let oid_str = format!("{bucket_str}{suffix}");
                let oid = match Oid::from_str(&oid_str) {
                    Ok(o) => o,
                    Err(_) => continue,
                };
                if reachable.contains(&oid) {
                    continue;
                }
                // Age check: only prune objects older than the horizon to
                // avoid racing with a writer that just landed a tree but
                // whose caller hasn't yet recorded the oid in metadata.
                let md = match fs::metadata(&obj_path) {
                    Ok(m) => m,
                    Err(_) => continue,
                };
                let mtime = md.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                if mtime > cutoff {
                    continue;
                }
                if fs::remove_file(&obj_path).is_ok() {
                    pruned += 1;
                }
            }
        }
        Ok(pruned)
    }
}

/// Outcome of restoring one path from a stored tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShadowRestoreAction {
    /// On-disk content was updated to match the tree.
    Wrote { path: String, bytes: u64 },
    /// On-disk file was deleted (tree didn't contain the path).
    Deleted { path: String },
    /// Disk already matched the tree (or both were absent) — no write.
    NoOp { path: String },
}

/// Classification of one path inside a `diff_full` result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShadowChangeStatus {
    /// Path is in the `to` tree but not in `from`.
    Added,
    /// Path is in both trees but content differs.
    Modified,
    /// Path is in `from` but not in `to`.
    Deleted,
}

/// One per-path row in a `diff_full` result.
#[derive(Debug, Clone)]
pub struct ShadowFileChange {
    pub path: String,
    pub status: ShadowChangeStatus,
    pub binary: bool,
    pub additions: u32,
    pub deletions: u32,
}

/// Walk `tree_oid` and add every transitively reachable tree+blob oid to
/// `out`. Used by `cleanup` to build the keep-set before pruning loose
/// objects.
fn collect_reachable(
    repo: &Repository,
    tree_oid: Oid,
    out: &mut HashSet<Oid>,
) -> Result<()> {
    if !out.insert(tree_oid) {
        // Already visited this subtree on a previous keep tree.
        return Ok(());
    }
    let tree = repo.find_tree(tree_oid)?;
    tree.walk(git2::TreeWalkMode::PreOrder, |_, entry| {
        out.insert(entry.id());
        git2::TreeWalkResult::Ok
    })?;
    Ok(())
}

/// Build a stub `IndexEntry` for `add_frombuffer`. libgit2 writes the
/// blob and fills in `id` from the buffer; the rest of the metadata
/// (`uid`/`gid`/`dev`/`ino`/`ctime`/`mtime`) is intentionally zeroed —
/// we don't care, and zeros keep tree oids deterministic across runs.
fn make_index_entry(rel_path: &str, size: u32) -> IndexEntry {
    IndexEntry {
        ctime: IndexTime::new(0, 0),
        mtime: IndexTime::new(0, 0),
        dev: 0,
        ino: 0,
        mode: 0o100644, // regular non-executable file
        uid: 0,
        gid: 0,
        file_size: size,
        id: Oid::zero(),
        flags: 0,
        flags_extended: 0,
        path: rel_path.as_bytes().to_vec(),
    }
}

/// Stable 32-char project id derived from the canonical worktree path.
/// SHA-256 over the path string, truncated to 16 bytes / 32 hex chars —
/// far more than enough collision space for "shadow repos this user has
/// ever opened" while staying short enough for a readable directory name.
fn compute_project_hash(canon_root: &Path) -> String {
    let s = canon_root.to_string_lossy();
    // Windows paths are case-insensitive; fold so `C:\Foo` and `c:\foo`
    // map to the same shadow repo. Unix preserves case.
    #[cfg(windows)]
    let s = s.to_lowercase();
    let mut hasher = Sha256::new();
    hasher.update(s.as_bytes());
    let hash = hasher.finalize();
    hex::encode(&hash[..16])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    fn write(p: &Path, content: &[u8]) {
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let mut f = fs::File::create(p).unwrap();
        f.write_all(content).unwrap();
    }

    /// Minimal setup: tempdir for the worktree (with a fake `.git/` so
    /// `walk`'s gitignore branches activate the same as in prod), plus a
    /// separate tempdir for the shadow root.
    struct Fixture {
        _wt_dir: tempfile::TempDir,
        _shadow_dir: tempfile::TempDir,
        worktree: PathBuf,
        snapshot: Arc<ShadowSnapshot>,
    }

    fn fixture() -> Fixture {
        let wt_dir = tempfile::tempdir().unwrap();
        let shadow_dir = tempfile::tempdir().unwrap();
        let worktree = wt_dir.path().canonicalize().unwrap();
        fs::create_dir_all(worktree.join(".git")).unwrap();
        let snapshot =
            ShadowSnapshot::for_worktree(&worktree, shadow_dir.path()).unwrap();
        Fixture {
            _wt_dir: wt_dir,
            _shadow_dir: shadow_dir,
            worktree,
            snapshot,
        }
    }

    #[test]
    fn for_worktree_creates_bare_repo_on_first_call_and_reopens_on_second() {
        let wt_dir = tempfile::tempdir().unwrap();
        let shadow_dir = tempfile::tempdir().unwrap();
        let wt = wt_dir.path();

        let s1 = ShadowSnapshot::for_worktree(wt, shadow_dir.path()).unwrap();
        let path1 = s1.repo_path().to_path_buf();
        drop(s1);

        let s2 = ShadowSnapshot::for_worktree(wt, shadow_dir.path()).unwrap();
        assert_eq!(s2.repo_path(), path1, "same worktree must yield same shadow path");

        // Bare repos have a HEAD file but no working tree.
        assert!(path1.join("HEAD").exists());
        assert!(!path1.join(".git").exists());
    }

    #[test]
    fn track_returns_distinct_tree_oids_when_content_changes() {
        let f = fixture();
        let foo = f.worktree.join("foo.txt");
        write(&foo, b"v1");
        let r1 = f.snapshot.track().unwrap();

        write(&foo, b"v2");
        let r2 = f.snapshot.track().unwrap();

        assert_ne!(r1.tree_oid, r2.tree_oid);
        assert_eq!(r1.files_tracked, 1);
        assert_eq!(r2.files_tracked, 1);
    }

    #[test]
    fn track_is_deterministic_for_same_content() {
        let f = fixture();
        write(&f.worktree.join("a.txt"), b"alpha");
        write(&f.worktree.join("b.txt"), b"beta");
        let r1 = f.snapshot.track().unwrap();
        let r2 = f.snapshot.track().unwrap();
        assert_eq!(
            r1.tree_oid, r2.tree_oid,
            "identical worktree state must produce identical tree oid"
        );
    }

    #[test]
    fn track_empty_worktree_yields_empty_tree() {
        let f = fixture();
        let r = f.snapshot.track().unwrap();
        assert_eq!(r.files_tracked, 0);
        // The canonical empty tree oid in libgit2 — sanity check on the
        // deterministic-build claim above.
        let s = r.tree_oid.to_string();
        assert_eq!(s, "4b825dc642cb6eb9a060e54bf8d69288fbee4904");
    }

    #[test]
    fn patch_reports_only_changed_paths() {
        let f = fixture();
        write(&f.worktree.join("stable.txt"), b"unchanged");
        write(&f.worktree.join("churn.txt"), b"original");
        let r1 = f.snapshot.track().unwrap();

        write(&f.worktree.join("churn.txt"), b"edited");
        let changed = f.snapshot.patch(r1.tree_oid).unwrap();
        assert_eq!(changed, vec!["churn.txt".to_string()]);
    }

    #[test]
    fn diff_paths_lists_added_and_removed() {
        let f = fixture();
        write(&f.worktree.join("kept.txt"), b"same");
        write(&f.worktree.join("removed.txt"), b"gone soon");
        let r1 = f.snapshot.track().unwrap();

        fs::remove_file(f.worktree.join("removed.txt")).unwrap();
        write(&f.worktree.join("added.txt"), b"new!");
        let r2 = f.snapshot.track().unwrap();

        let mut paths = f.snapshot.diff_paths(r1.tree_oid, r2.tree_oid).unwrap();
        paths.sort();
        assert_eq!(paths, vec!["added.txt".to_string(), "removed.txt".to_string()]);
    }

    #[test]
    fn diff_emits_unified_patch_text() {
        let f = fixture();
        write(&f.worktree.join("x.txt"), b"line1\nline2\n");
        let r1 = f.snapshot.track().unwrap();

        write(&f.worktree.join("x.txt"), b"line1\nline2-modified\n");
        let r2 = f.snapshot.track().unwrap();

        let diff = f.snapshot.diff(r1.tree_oid, r2.tree_oid).unwrap();
        assert!(diff.contains("-line2"));
        assert!(diff.contains("+line2-modified"));
    }

    #[test]
    fn track_skips_files_over_hard_cap() {
        let f = fixture();
        // 51 MiB — over the 50 MiB hard cap.
        let big = vec![b'A'; (MAX_TRACKED_FILE_SIZE + 1) as usize];
        write(&f.worktree.join("huge.bin"), &big);
        write(&f.worktree.join("small.txt"), b"fine");

        let r = f.snapshot.track().unwrap();
        assert_eq!(r.files_tracked, 1);
        assert_eq!(r.files_too_large.len(), 1);
        assert_eq!(r.files_too_large[0].0, "huge.bin");
    }

    #[test]
    fn track_captures_medium_files_above_sync_soft_limit() {
        let f = fixture();
        // 6 MiB — above SYNC soft limit but well below the hard cap. The
        // shadow itself doesn't gate on the soft limit; that's a caller
        // (tracker) concern about where to run the call. We verify only
        // that the file ends up in the tree.
        let med = vec![b'B'; (SYNC_CAPTURE_SOFT_LIMIT + 1024) as usize];
        write(&f.worktree.join("med.bin"), &med);

        let r = f.snapshot.track().unwrap();
        assert_eq!(r.files_tracked, 1);
        assert!(r.files_too_large.is_empty());
        let bytes = f.snapshot.read_path(r.tree_oid, "med.bin").unwrap().unwrap();
        assert_eq!(bytes.len(), med.len());
    }

    #[test]
    fn read_path_returns_blob_bytes() {
        let f = fixture();
        write(&f.worktree.join("sub/file.rs"), b"fn main() {}");
        let r = f.snapshot.track().unwrap();
        let bytes = f.snapshot.read_path(r.tree_oid, "sub/file.rs").unwrap();
        assert_eq!(bytes.as_deref(), Some(&b"fn main() {}"[..]));
    }

    #[test]
    fn read_path_returns_none_for_absent_path() {
        let f = fixture();
        write(&f.worktree.join("present.txt"), b"hi");
        let r = f.snapshot.track().unwrap();
        let bytes = f.snapshot.read_path(r.tree_oid, "absent.txt").unwrap();
        assert!(bytes.is_none());
    }

    #[test]
    fn list_paths_walks_tree_recursively() {
        let f = fixture();
        write(&f.worktree.join("a.txt"), b"a");
        write(&f.worktree.join("sub/b.txt"), b"b");
        write(&f.worktree.join("sub/deep/c.txt"), b"c");
        let r = f.snapshot.track().unwrap();
        let mut paths = f.snapshot.list_paths(r.tree_oid).unwrap();
        paths.sort();
        assert_eq!(
            paths,
            vec![
                "a.txt".to_string(),
                "sub/b.txt".to_string(),
                "sub/deep/c.txt".to_string(),
            ]
        );
    }

    #[test]
    fn shadow_respects_gitignore() {
        let f = fixture();
        write(&f.worktree.join(".gitignore"), b"secret.txt\n");
        write(&f.worktree.join("public.txt"), b"public");
        write(&f.worktree.join("secret.txt"), b"hidden");
        let r = f.snapshot.track().unwrap();
        let paths = f.snapshot.list_paths(r.tree_oid).unwrap();
        assert!(paths.contains(&"public.txt".to_string()));
        assert!(paths.contains(&".gitignore".to_string()));
        assert!(!paths.contains(&"secret.txt".to_string()));
    }

    #[test]
    fn project_hash_is_stable_for_same_worktree() {
        let wt = tempfile::tempdir().unwrap();
        let canon = wt.path().canonicalize().unwrap();
        let h1 = compute_project_hash(&canon);
        let h2 = compute_project_hash(&canon);
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 32, "32-char hex (16 bytes)");
    }

    #[cfg(windows)]
    #[test]
    fn project_hash_folds_windows_case() {
        let upper = Path::new("C:\\Projects\\Foo");
        let lower = Path::new("c:\\projects\\foo");
        assert_eq!(compute_project_hash(upper), compute_project_hash(lower));
    }

    #[test]
    fn restore_path_writes_blob_when_disk_differs() {
        let f = fixture();
        let p = f.worktree.join("file.txt");
        write(&p, b"original");
        let r = f.snapshot.track().unwrap();

        // Mutate the file, then restore.
        write(&p, b"agent edit");
        let action = f.snapshot.restore_path(r.tree_oid, "file.txt").unwrap();
        match action {
            ShadowRestoreAction::Wrote { path, bytes } => {
                assert_eq!(path, "file.txt");
                assert_eq!(bytes, b"original".len() as u64);
            }
            other => panic!("expected Wrote, got {other:?}"),
        }
        assert_eq!(fs::read(&p).unwrap(), b"original");
    }

    #[test]
    fn restore_path_is_noop_when_disk_matches() {
        let f = fixture();
        let p = f.worktree.join("same.txt");
        write(&p, b"identical");
        let r = f.snapshot.track().unwrap();

        // Same content on disk: restore should be a no-op.
        let action = f.snapshot.restore_path(r.tree_oid, "same.txt").unwrap();
        assert!(
            matches!(action, ShadowRestoreAction::NoOp { .. }),
            "expected NoOp, got {action:?}"
        );
    }

    #[test]
    fn restore_path_deletes_when_absent_from_tree() {
        let f = fixture();
        // Tree captured while file didn't exist.
        let empty = f.snapshot.track().unwrap();
        // Agent creates a new file post-snapshot.
        let p = f.worktree.join("born_after.txt");
        write(&p, b"newborn");
        assert!(p.exists());

        let action = f.snapshot.restore_path(empty.tree_oid, "born_after.txt").unwrap();
        match action {
            ShadowRestoreAction::Deleted { path } => assert_eq!(path, "born_after.txt"),
            other => panic!("expected Deleted, got {other:?}"),
        }
        assert!(!p.exists());
    }

    #[test]
    fn restore_path_noop_when_absent_both_in_tree_and_disk() {
        let f = fixture();
        let empty = f.snapshot.track().unwrap();
        let action = f.snapshot.restore_path(empty.tree_oid, "never_existed.txt").unwrap();
        assert!(
            matches!(action, ShadowRestoreAction::NoOp { .. }),
            "absent-from-both is a no-op, got {action:?}"
        );
    }

    #[test]
    fn restore_path_creates_parent_dirs() {
        let f = fixture();
        let nested = f.worktree.join("sub/deep/leaf.txt");
        write(&nested, b"nested-content");
        let r = f.snapshot.track().unwrap();

        // Wipe the whole subtree on disk.
        fs::remove_dir_all(f.worktree.join("sub")).unwrap();
        assert!(!nested.exists());

        let action = f.snapshot.restore_path(r.tree_oid, "sub/deep/leaf.txt").unwrap();
        assert!(matches!(action, ShadowRestoreAction::Wrote { .. }));
        assert_eq!(fs::read(&nested).unwrap(), b"nested-content");
    }

    #[test]
    fn restore_paths_from_batches_multiple_paths() {
        let f = fixture();
        write(&f.worktree.join("a.txt"), b"A1");
        write(&f.worktree.join("b.txt"), b"B1");
        write(&f.worktree.join("c.txt"), b"C1");
        let r = f.snapshot.track().unwrap();

        // Edit two, leave one alone, plus add an unrelated file we don't ask to restore.
        write(&f.worktree.join("a.txt"), b"agent edit A");
        write(&f.worktree.join("b.txt"), b"agent edit B");
        write(&f.worktree.join("d.txt"), b"unrelated");

        let actions = f
            .snapshot
            .restore_paths_from(
                r.tree_oid,
                &["a.txt".to_string(), "b.txt".to_string(), "c.txt".to_string()],
            )
            .unwrap();
        assert_eq!(actions.len(), 3);
        // a and b were modified → Wrote; c was untouched → NoOp.
        assert!(matches!(actions[0], ShadowRestoreAction::Wrote { .. }));
        assert!(matches!(actions[1], ShadowRestoreAction::Wrote { .. }));
        assert!(matches!(actions[2], ShadowRestoreAction::NoOp { .. }));

        assert_eq!(fs::read(f.worktree.join("a.txt")).unwrap(), b"A1");
        assert_eq!(fs::read(f.worktree.join("b.txt")).unwrap(), b"B1");
        // We didn't request d.txt; unrelated file must stay untouched.
        assert_eq!(fs::read(f.worktree.join("d.txt")).unwrap(), b"unrelated");
    }

    #[test]
    fn cleanup_prunes_unreachable_with_zero_horizon() {
        let f = fixture();
        // Tree #1: just a.txt. We don't keep its oid — the point of this
        // setup is that the b.txt blob and intermediate trees exist in the
        // odb so `cleanup` has unreachable objects to find.
        write(&f.worktree.join("a.txt"), b"alpha");
        let _ = f.snapshot.track().unwrap().tree_oid;

        // Tree #2: a.txt unchanged + b.txt added (also intentionally
        // dropped from the keep set).
        write(&f.worktree.join("b.txt"), b"beta");
        let _ = f.snapshot.track().unwrap().tree_oid;

        // Tree #3: b.txt removed, a.txt edited
        fs::remove_file(f.worktree.join("b.txt")).unwrap();
        write(&f.worktree.join("a.txt"), b"alpha-v2");
        let t3 = f.snapshot.track().unwrap().tree_oid;

        // Keep only t3. t1 and t2's distinct objects (b.txt's blob, the
        // intermediate trees, the original a.txt blob) should prune.
        // Use Duration::ZERO to prune regardless of mtime.
        let pruned = f.snapshot.cleanup(&[t3], Duration::ZERO).unwrap();
        assert!(pruned > 0, "expected at least one object pruned, got {pruned}");

        // t3 is still reachable and usable.
        let paths = f.snapshot.list_paths(t3).unwrap();
        assert_eq!(paths, vec!["a.txt".to_string()]);
        let bytes = f.snapshot.read_path(t3, "a.txt").unwrap().unwrap();
        assert_eq!(bytes, b"alpha-v2");
    }

    #[test]
    fn cleanup_respects_prune_horizon() {
        let f = fixture();
        write(&f.worktree.join("a.txt"), b"alpha");
        let _t1 = f.snapshot.track().unwrap().tree_oid;
        write(&f.worktree.join("a.txt"), b"alpha-v2");
        let t2 = f.snapshot.track().unwrap().tree_oid;

        // Keep only t2 but use a 1-day horizon: just-written loose objects
        // shouldn't be pruned (they're younger than the cutoff).
        let pruned = f
            .snapshot
            .cleanup(&[t2], Duration::from_secs(24 * 3600))
            .unwrap();
        assert_eq!(pruned, 0, "young objects must be preserved");
    }

    #[test]
    fn cleanup_keeps_everything_when_all_trees_in_keep_set() {
        let f = fixture();
        write(&f.worktree.join("a.txt"), b"alpha");
        let t1 = f.snapshot.track().unwrap().tree_oid;
        write(&f.worktree.join("b.txt"), b"beta");
        let t2 = f.snapshot.track().unwrap().tree_oid;

        let pruned = f.snapshot.cleanup(&[t1, t2], Duration::ZERO).unwrap();
        assert_eq!(pruned, 0);
        // Both trees still readable.
        assert!(f.snapshot.read_path(t1, "a.txt").unwrap().is_some());
        assert!(f.snapshot.read_path(t2, "b.txt").unwrap().is_some());
    }

    /// Day 2 concurrency: 3 threads each calling `track()` concurrently
    /// against the same shadow on a quiescent worktree must all succeed
    /// and produce the same tree oid (the mutex serializes them; the
    /// content they see is identical).
    #[test]
    fn track_is_safe_under_concurrent_callers() {
        let f = fixture();
        write(&f.worktree.join("a.txt"), b"alpha");
        write(&f.worktree.join("sub/b.txt"), b"beta");

        let snap = Arc::clone(&f.snapshot);
        let handles: Vec<_> = (0..3)
            .map(|_| {
                let s = Arc::clone(&snap);
                std::thread::spawn(move || s.track().unwrap().tree_oid)
            })
            .collect();
        let oids: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert_eq!(oids.len(), 3);
        assert!(
            oids.windows(2).all(|w| w[0] == w[1]),
            "expected all three threads to produce the same tree oid, got {oids:?}"
        );
    }

    #[test]
    fn diff_full_classifies_added_modified_deleted() {
        let f = fixture();
        write(&f.worktree.join("kept.txt"), b"same");
        write(&f.worktree.join("changed.txt"), b"v1");
        write(&f.worktree.join("doomed.txt"), b"goodbye");
        let t1 = f.snapshot.track().unwrap().tree_oid;

        write(&f.worktree.join("changed.txt"), b"v2\nv3\n");
        fs::remove_file(f.worktree.join("doomed.txt")).unwrap();
        write(&f.worktree.join("born.txt"), b"hello world\n");
        let t2 = f.snapshot.track().unwrap().tree_oid;

        let mut diffs = f.snapshot.diff_full(t1, t2).unwrap();
        diffs.sort_by(|a, b| a.path.cmp(&b.path));
        let by_path: std::collections::HashMap<_, _> =
            diffs.iter().map(|d| (d.path.as_str(), d)).collect();

        // born.txt → Added with all the lines as additions
        let born = by_path["born.txt"];
        assert_eq!(born.status, ShadowChangeStatus::Added);
        assert!(!born.binary);
        assert!(born.additions >= 1 && born.deletions == 0);

        // changed.txt → Modified with both adds and dels recorded
        let changed = by_path["changed.txt"];
        assert_eq!(changed.status, ShadowChangeStatus::Modified);
        assert!(!changed.binary);
        assert!(changed.additions >= 1 && changed.deletions >= 1);

        // doomed.txt → Deleted
        let doomed = by_path["doomed.txt"];
        assert_eq!(doomed.status, ShadowChangeStatus::Deleted);
        assert!(!doomed.binary);

        // kept.txt didn't change → not in the diff at all
        assert!(!by_path.contains_key("kept.txt"));
    }

    #[test]
    fn diff_full_marks_binary_files() {
        let f = fixture();
        // libgit2's binary detector flags content with NUL bytes in the
        // first ~8000 bytes. A short blob with an embedded NUL is
        // sufficient and avoids slow file IO.
        let bin_v1 = vec![0u8, 1, 2, 3, b'A', b'B'];
        let bin_v2 = vec![0u8, 1, 2, 3, b'X', b'Y'];
        write(&f.worktree.join("blob.bin"), &bin_v1);
        let t1 = f.snapshot.track().unwrap().tree_oid;
        write(&f.worktree.join("blob.bin"), &bin_v2);
        let t2 = f.snapshot.track().unwrap().tree_oid;

        let diffs = f.snapshot.diff_full(t1, t2).unwrap();
        let row = diffs.iter().find(|d| d.path == "blob.bin").unwrap();
        assert!(row.binary, "expected blob.bin to be flagged binary");
        // Binary diffs report zero line counts (no meaningful text delta).
        assert_eq!(row.additions, 0);
        assert_eq!(row.deletions, 0);
    }

    #[test]
    fn unified_diff_for_path_emits_git_canonical_header() {
        let f = fixture();
        write(&f.worktree.join("note.md"), b"# title\nbody\n");
        let t1 = f.snapshot.track().unwrap().tree_oid;
        write(&f.worktree.join("note.md"), b"# title\nbody-edited\n");
        let t2 = f.snapshot.track().unwrap().tree_oid;

        let text = f
            .snapshot
            .unified_diff_for_path(t1, t2, "note.md")
            .unwrap()
            .unwrap();
        // Canonical libgit2 patch output begins with the git header,
        // then ---/+++ lines, then hunks. We don't assert exact bytes
        // but the structural markers must be present.
        assert!(
            text.contains("diff --git a/note.md b/note.md"),
            "missing diff --git header: {text}"
        );
        assert!(text.contains("--- a/note.md"));
        assert!(text.contains("+++ b/note.md"));
        assert!(text.contains("-body"));
        assert!(text.contains("+body-edited"));
    }

    #[test]
    fn unified_diff_for_path_returns_none_for_unchanged() {
        let f = fixture();
        write(&f.worktree.join("static.txt"), b"unchanged");
        let t = f.snapshot.track().unwrap().tree_oid;
        let out = f.snapshot.unified_diff_for_path(t, t, "static.txt").unwrap();
        assert!(out.is_none());
    }

    #[test]
    fn concurrent_track_and_restore_do_not_deadlock() {
        let f = fixture();
        write(&f.worktree.join("base.txt"), b"v1");
        let baseline = f.snapshot.track().unwrap().tree_oid;
        write(&f.worktree.join("base.txt"), b"v2-agent");

        let snap = Arc::clone(&f.snapshot);
        let t1 = {
            let s = Arc::clone(&snap);
            std::thread::spawn(move || s.track().unwrap().tree_oid)
        };
        let t2 = {
            let s = Arc::clone(&snap);
            std::thread::spawn(move || {
                s.restore_path(baseline, "base.txt").unwrap()
            })
        };

        let _ = t1.join().unwrap();
        let action = t2.join().unwrap();
        // One of two valid outcomes depending on interleave: either the
        // restore raced ahead of the track and Wrote; or it landed after
        // the track observed v2 and Wrote anyway. Both are correct — the
        // only failure mode we're guarding against is deadlock or panic.
        assert!(matches!(
            action,
            ShadowRestoreAction::Wrote { .. } | ShadowRestoreAction::NoOp { .. }
        ));
    }
}

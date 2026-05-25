//! Gitoxide-backed shadow snapshot store.
//!
//! Bare git repo at `<shadow_root>/<project_hash>/` storing tree+blob objects
//! only (no commits, no refs). Callers record per-message tree oids in SQLite
//! and use this module to diff/restore against them.
//!
//! Ported from `git2` (libgit2 binding) to `gix` (pure-Rust gitoxide) per the
//! gix migration decision in `docs/educated-guesses/001-scope-full-migration.md`.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use gix::objs::tree::EntryKind;
use gix::ObjectId;
use sha2::{Digest, Sha256};
use thiserror::Error;

use super::walk::{join_rel, walk_for_sweep};

/// Re-exposed for callers that previously used `git2::Oid` directly.
/// Gitoxide names this `ObjectId`; we alias to preserve the call-site name.
pub type Oid = ObjectId;

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
    /// Wraps any error returned by a gitoxide operation. Box<dyn> keeps the
    /// variant a single shape across gix's many per-operation error types.
    #[error("gix: {0}")]
    Git(Box<dyn std::error::Error + Send + Sync>),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("shadow repo mutex poisoned")]
    Poisoned,
}

/// Helper to lift any gix error into `ShadowError::Git` at a call site.
fn git_err<E>(e: E) -> ShadowError
where
    E: std::error::Error + Send + Sync + 'static,
{
    ShadowError::Git(Box::new(e))
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
/// serialize on a per-shadow mutex; the mutex covers the gix repository
/// handle (gix::Repository is `Send` but not `Sync`, so the Mutex pattern
/// from the libgit2-era code carries over directly).
pub struct ShadowSnapshot {
    repo: Mutex<gix::Repository>,
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

        // gix has no direct `open_bare` — open() succeeds on any repository
        // layout and we don't need to distinguish: bare-vs-non-bare matters
        // only when there's a worktree, which our shadow repos never have.
        // First try to open; if that fails (e.g. first-time create), init bare.
        let repo = match gix::open(&repo_path) {
            Ok(r) => r,
            Err(_) => gix::init_bare(&repo_path).map_err(git_err)?,
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
    /// a tree object, and persist it. Returns the tree oid plus bookkeeping.
    /// Files above `MAX_TRACKED_FILE_SIZE` are skipped and reported in
    /// `files_too_large`.
    ///
    /// This is synchronous and holds the shadow's mutex for the duration
    /// of the walk + blob writes. Concurrent callers serialize behind it.
    ///
    /// Idiomatic-gix-vs-libgit2 difference: we no longer synthesise an
    /// in-memory index just to flush it as a tree. Instead we use gix's
    /// tree `Editor` which lets us append `(path, blob_oid, mode)` entries
    /// directly and writes the bottom-up tree set for us.
    pub fn track(&self) -> Result<TrackResult> {
        let walked = walk_for_sweep(&self.worktree_root);

        let repo_guard = self.repo.lock().map_err(|_| ShadowError::Poisoned)?;
        let repo = &*repo_guard;

        // Start the tree editor from the well-known empty tree. write_blob
        // writes blobs straight into the ODB; the editor accumulates entries
        // and persists subtrees + the root tree on `write()`.
        let empty_tree = empty_tree_oid();
        let mut editor = repo.edit_tree(empty_tree).map_err(git_err)?;

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
            let blob_oid = repo.write_blob(&content).map_err(git_err)?;
            let path_bstr = path_to_bstr(&file.rel_path);
            editor
                .upsert(&path_bstr, EntryKind::Blob, blob_oid.detach())
                .map_err(git_err)?;
            tracked += 1;
        }

        let tree_oid = editor.write().map_err(git_err)?.detach();
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
        let from_tree = repo.find_tree(from).map_err(git_err)?;
        let to_tree = repo.find_tree(to).map_err(git_err)?;
        let changes = repo
            .diff_tree_to_tree(Some(&from_tree), Some(&to_tree), None)
            .map_err(git_err)?;

        let mut paths = Vec::with_capacity(changes.len());
        for change in changes {
            paths.push(change_path(&change));
        }
        Ok(paths)
    }

    /// Unified-diff text between two stored trees. Suitable for handing
    /// to the diff renderer in the chat-view's file pane.
    pub fn diff(&self, from: Oid, to: Oid) -> Result<String> {
        let repo = self.repo.lock().map_err(|_| ShadowError::Poisoned)?;
        let from_tree = repo.find_tree(from).map_err(git_err)?;
        let to_tree = repo.find_tree(to).map_err(git_err)?;
        let changes = repo
            .diff_tree_to_tree(Some(&from_tree), Some(&to_tree), None)
            .map_err(git_err)?;

        let mut out = String::new();
        for change in &changes {
            let path = change_path(change);
            if let Some(text) = render_unified_diff(&repo, change, &path)? {
                out.push_str(&text);
            }
        }
        Ok(out)
    }

    /// Per-path stats for the diff between two stored trees: status
    /// (Added/Modified/Deleted), binary flag, added/deleted line counts.
    pub fn diff_full(&self, from: Oid, to: Oid) -> Result<Vec<ShadowFileChange>> {
        let repo = self.repo.lock().map_err(|_| ShadowError::Poisoned)?;
        let from_tree = repo.find_tree(from).map_err(git_err)?;
        let to_tree = repo.find_tree(to).map_err(git_err)?;
        let changes = repo
            .diff_tree_to_tree(Some(&from_tree), Some(&to_tree), None)
            .map_err(git_err)?;

        let mut out = Vec::with_capacity(changes.len());
        for change in &changes {
            let path = change_path(change);
            let status = change_status(change);
            let (binary, additions, deletions) =
                compute_line_stats(&repo, change).unwrap_or((false, 0, 0));
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
        let from_tree = repo.find_tree(from).map_err(git_err)?;
        let to_tree = repo.find_tree(to).map_err(git_err)?;
        let changes = repo
            .diff_tree_to_tree(Some(&from_tree), Some(&to_tree), None)
            .map_err(git_err)?;

        for change in &changes {
            let p = change_path(change);
            if p == path {
                return render_unified_diff(&repo, change, &p);
            }
        }
        Ok(None)
    }

    /// Materialize the content of `path` as it was recorded in `tree`.
    /// Returns the blob bytes or `None` if the path is absent from that
    /// tree. Used by the higher-level `file_diff` to render before/after.
    pub fn read_path(&self, tree: Oid, path: &str) -> Result<Option<Vec<u8>>> {
        let repo = self.repo.lock().map_err(|_| ShadowError::Poisoned)?;
        let tree_obj = repo.find_tree(tree).map_err(git_err)?;
        match tree_obj.lookup_entry_by_path(Path::new(path)).map_err(git_err)? {
            Some(entry) => {
                let blob = repo.find_blob(entry.oid()).map_err(git_err)?;
                Ok(Some(blob.data.clone()))
            }
            None => Ok(None),
        }
    }

    /// List every path stored in `tree`. Used by `TaskNetChange` and
    /// revert plans to enumerate the per-snapshot file set.
    pub fn list_paths(&self, tree: Oid) -> Result<Vec<String>> {
        let repo = self.repo.lock().map_err(|_| ShadowError::Poisoned)?;
        let mut paths = Vec::new();
        collect_blob_paths(&repo, tree, String::new(), &mut paths)?;
        Ok(paths)
    }

    /// Restore `path` to its state in `tree`.
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
            let tree_obj = repo.find_tree(tree).map_err(git_err)?;
            let mut out = Vec::with_capacity(paths.len());
            for path in paths {
                let bytes = match tree_obj
                    .lookup_entry_by_path(Path::new(path))
                    .map_err(git_err)?
                {
                    Some(entry) => {
                        let blob = repo.find_blob(entry.oid()).map_err(git_err)?;
                        Some(blob.data.clone())
                    }
                    None => None,
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
    pub fn cleanup(&self, keep_trees: &[Oid], prune_horizon: Duration) -> Result<usize> {
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
                let oid = match ObjectId::from_hex(oid_str.as_bytes()) {
                    Ok(o) => o,
                    Err(_) => continue,
                };
                if reachable.contains(&oid) {
                    continue;
                }
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

// ---------- helpers ----------

/// Well-known empty-tree oid for git's SHA-1 mode. Used as the starting
/// point for `Repository::edit_tree`. Identical hex string for libgit2 and
/// gitoxide because git's tree-hashing is content-defined.
fn empty_tree_oid() -> ObjectId {
    // 4b825dc642cb6eb9a060e54bf8d69288fbee4904
    ObjectId::from_hex(b"4b825dc642cb6eb9a060e54bf8d69288fbee4904").expect("hardcoded valid hex")
}

/// Convert a forward-slash relative path to a `BString` for gix APIs.
fn path_to_bstr(path: &str) -> gix::bstr::BString {
    gix::bstr::BString::from(path.as_bytes())
}

/// Extract the path from a gix tree-diff change as a String (forward-slash
/// canonical form, matching what we store in tree entries).
fn change_path(change: &gix::object::tree::diff::ChangeDetached) -> String {
    change.location().to_string()
}

fn change_status(change: &gix::object::tree::diff::ChangeDetached) -> ShadowChangeStatus {
    use gix::object::tree::diff::ChangeDetached::*;
    match change {
        Addition { .. } => ShadowChangeStatus::Added,
        Deletion { .. } => ShadowChangeStatus::Deleted,
        Modification { .. } => ShadowChangeStatus::Modified,
        Rewrite { .. } => ShadowChangeStatus::Modified,
    }
}

/// Returns (binary, additions, deletions) for one diff change. Reads blobs
/// from the odb on either side and runs the imara-diff line counter under
/// the hood; binary detection mirrors libgit2's heuristic (NUL in first 8KiB).
fn compute_line_stats(
    repo: &gix::Repository,
    change: &gix::object::tree::diff::ChangeDetached,
) -> Result<(bool, u32, u32)> {
    let (old_oid, new_oid) = change_oids(change);

    let old_bytes = match old_oid {
        Some(o) => Some(repo.find_blob(o).map_err(git_err)?.data.clone()),
        None => None,
    };
    let new_bytes = match new_oid {
        Some(o) => Some(repo.find_blob(o).map_err(git_err)?.data.clone()),
        None => None,
    };

    let is_binary = looks_binary(old_bytes.as_deref()) || looks_binary(new_bytes.as_deref());
    if is_binary {
        return Ok((true, 0, 0));
    }

    let old_text = old_bytes.as_deref().unwrap_or(&[]);
    let new_text = new_bytes.as_deref().unwrap_or(&[]);
    let (adds, dels) = count_line_changes(old_text, new_text);
    Ok((false, adds, dels))
}

/// Render a single change as canonical `diff --git`-style unified-diff text.
/// Returns Ok(None) when the change has no representable text diff (e.g.
/// identical content after rename detection).
fn render_unified_diff(
    repo: &gix::Repository,
    change: &gix::object::tree::diff::ChangeDetached,
    path: &str,
) -> Result<Option<String>> {
    let (old_oid, new_oid) = change_oids(change);

    let old_bytes = match old_oid {
        Some(o) => Some(repo.find_blob(o).map_err(git_err)?.data.clone()),
        None => None,
    };
    let new_bytes = match new_oid {
        Some(o) => Some(repo.find_blob(o).map_err(git_err)?.data.clone()),
        None => None,
    };

    if looks_binary(old_bytes.as_deref()) || looks_binary(new_bytes.as_deref()) {
        // Match libgit2's `Binary files ... differ` output shape so the
        // frontend diff renderer doesn't need a special branch for gix.
        let mut header = String::new();
        header.push_str(&format!("diff --git a/{path} b/{path}\n"));
        header.push_str(&format!("Binary files a/{path} and b/{path} differ\n"));
        return Ok(Some(header));
    }

    let old_text = old_bytes.as_deref().unwrap_or(&[]);
    let new_text = new_bytes.as_deref().unwrap_or(&[]);
    if old_text == new_text {
        return Ok(None);
    }

    Ok(Some(format_unified_diff(path, old_text, new_text)))
}

/// Extract before/after blob oids from a tree-diff change.
fn change_oids(
    change: &gix::object::tree::diff::ChangeDetached,
) -> (Option<ObjectId>, Option<ObjectId>) {
    use gix::object::tree::diff::ChangeDetached::*;
    match change {
        Addition { id, .. } => (None, Some(*id)),
        Deletion { id, .. } => (Some(*id), None),
        Modification {
            previous_id, id, ..
        } => (Some(*previous_id), Some(*id)),
        Rewrite {
            source_id, id, ..
        } => (Some(*source_id), Some(*id)),
    }
}

/// Recursive tree walk that records every blob path (forward-slash
/// canonical) into `out`. Replaces libgit2's `tree.walk(PreOrder, cb)`.
fn collect_blob_paths(
    repo: &gix::Repository,
    tree_oid: ObjectId,
    prefix: String,
    out: &mut Vec<String>,
) -> Result<()> {
    let tree = repo.find_tree(tree_oid).map_err(git_err)?;
    for entry in tree.iter() {
        let entry = entry.map_err(git_err)?;
        let name = entry.filename().to_string();
        let path = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}{name}")
        };
        match entry.mode().kind() {
            EntryKind::Blob | EntryKind::BlobExecutable => out.push(path),
            EntryKind::Tree => {
                let sub_oid = entry.oid().to_owned();
                let sub_prefix = format!("{path}/");
                collect_blob_paths(repo, sub_oid, sub_prefix, out)?;
            }
            // Symlinks and commit (submodule) entries — not tracked by our
            // shadow store. The track() path only ever upserts EntryKind::Blob,
            // so encountering these would indicate corruption; skip silently.
            EntryKind::Link | EntryKind::Commit => {}
        }
    }
    Ok(())
}

/// Walk `tree_oid` and add every transitively reachable tree+blob oid to
/// `out`. Used by `cleanup` to build the keep-set before pruning loose
/// objects.
fn collect_reachable(
    repo: &gix::Repository,
    tree_oid: Oid,
    out: &mut HashSet<Oid>,
) -> Result<()> {
    if !out.insert(tree_oid) {
        return Ok(());
    }
    let tree = repo.find_tree(tree_oid).map_err(git_err)?;
    for entry in tree.iter() {
        let entry = entry.map_err(git_err)?;
        let oid = entry.oid().to_owned();
        match entry.mode().kind() {
            EntryKind::Blob | EntryKind::BlobExecutable | EntryKind::Link => {
                out.insert(oid);
            }
            EntryKind::Tree => {
                collect_reachable(repo, oid, out)?;
            }
            EntryKind::Commit => {} // submodule — not in our shadow store
        }
    }
    Ok(())
}

/// Stable 32-char project id derived from the canonical worktree path.
fn compute_project_hash(canon_root: &Path) -> String {
    let s = canon_root.to_string_lossy();
    #[cfg(windows)]
    let s = s.to_lowercase();
    let mut hasher = Sha256::new();
    hasher.update(s.as_bytes());
    let hash = hasher.finalize();
    hex::encode(&hash[..16])
}

/// libgit2-style binary detection: NUL byte in the first 8 KiB.
fn looks_binary(content: Option<&[u8]>) -> bool {
    let bytes = match content {
        Some(b) => b,
        None => return false,
    };
    let scan_len = bytes.len().min(8000);
    bytes[..scan_len].contains(&0)
}

/// Compute (additions, deletions) for a text diff using a minimal
/// line-by-line comparison. Matches the granularity libgit2 reports in
/// `Patch::line_stats` — counts changed lines, not inner-line diffs.
fn count_line_changes(old: &[u8], new: &[u8]) -> (u32, u32) {
    let old_text = std::str::from_utf8(old).unwrap_or("");
    let new_text = std::str::from_utf8(new).unwrap_or("");
    let old_lines: Vec<&str> = old_text.split_inclusive('\n').collect();
    let new_lines: Vec<&str> = new_text.split_inclusive('\n').collect();

    // similar's TextDiff gives us a Myers diff; sum the change ops.
    let diff = similar::TextDiff::from_slices(&old_lines, &new_lines);
    let mut adds = 0u32;
    let mut dels = 0u32;
    for op in diff.ops() {
        match op.tag() {
            similar::DiffTag::Insert => adds += op.new_range().len() as u32,
            similar::DiffTag::Delete => dels += op.old_range().len() as u32,
            similar::DiffTag::Replace => {
                adds += op.new_range().len() as u32;
                dels += op.old_range().len() as u32;
            }
            similar::DiffTag::Equal => {}
        }
    }
    (adds, dels)
}

/// Format a single-file unified diff in `diff --git` canonical shape so
/// the frontend renderer behaves identically to its libgit2-era output.
fn format_unified_diff(path: &str, old: &[u8], new: &[u8]) -> String {
    let old_text = std::str::from_utf8(old).unwrap_or("");
    let new_text = std::str::from_utf8(new).unwrap_or("");

    let mut out = String::new();
    out.push_str(&format!("diff --git a/{path} b/{path}\n"));
    out.push_str(&format!("--- a/{path}\n"));
    out.push_str(&format!("+++ b/{path}\n"));

    let diff = similar::TextDiff::from_lines(old_text, new_text);
    for hunk in diff.unified_diff().context_radius(3).iter_hunks() {
        out.push_str(&hunk.to_string());
    }
    out
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

        let action = f.snapshot.restore_path(r.tree_oid, "same.txt").unwrap();
        assert!(
            matches!(action, ShadowRestoreAction::NoOp { .. }),
            "expected NoOp, got {action:?}"
        );
    }

    #[test]
    fn restore_path_deletes_when_absent_from_tree() {
        let f = fixture();
        let empty = f.snapshot.track().unwrap();
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
        assert!(matches!(actions[0], ShadowRestoreAction::Wrote { .. }));
        assert!(matches!(actions[1], ShadowRestoreAction::Wrote { .. }));
        assert!(matches!(actions[2], ShadowRestoreAction::NoOp { .. }));

        assert_eq!(fs::read(f.worktree.join("a.txt")).unwrap(), b"A1");
        assert_eq!(fs::read(f.worktree.join("b.txt")).unwrap(), b"B1");
        assert_eq!(fs::read(f.worktree.join("d.txt")).unwrap(), b"unrelated");
    }

    #[test]
    fn cleanup_prunes_unreachable_with_zero_horizon() {
        let f = fixture();
        write(&f.worktree.join("a.txt"), b"alpha");
        let _ = f.snapshot.track().unwrap().tree_oid;

        write(&f.worktree.join("b.txt"), b"beta");
        let _ = f.snapshot.track().unwrap().tree_oid;

        fs::remove_file(f.worktree.join("b.txt")).unwrap();
        write(&f.worktree.join("a.txt"), b"alpha-v2");
        let t3 = f.snapshot.track().unwrap().tree_oid;

        let pruned = f.snapshot.cleanup(&[t3], Duration::ZERO).unwrap();
        assert!(pruned > 0, "expected at least one object pruned, got {pruned}");

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
        assert!(f.snapshot.read_path(t1, "a.txt").unwrap().is_some());
        assert!(f.snapshot.read_path(t2, "b.txt").unwrap().is_some());
    }

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

        let born = by_path["born.txt"];
        assert_eq!(born.status, ShadowChangeStatus::Added);
        assert!(!born.binary);
        assert!(born.additions >= 1 && born.deletions == 0);

        let changed = by_path["changed.txt"];
        assert_eq!(changed.status, ShadowChangeStatus::Modified);
        assert!(!changed.binary);
        assert!(changed.additions >= 1 && changed.deletions >= 1);

        let doomed = by_path["doomed.txt"];
        assert_eq!(doomed.status, ShadowChangeStatus::Deleted);
        assert!(!doomed.binary);

        assert!(!by_path.contains_key("kept.txt"));
    }

    #[test]
    fn diff_full_marks_binary_files() {
        let f = fixture();
        let bin_v1 = vec![0u8, 1, 2, 3, b'A', b'B'];
        let bin_v2 = vec![0u8, 1, 2, 3, b'X', b'Y'];
        write(&f.worktree.join("blob.bin"), &bin_v1);
        let t1 = f.snapshot.track().unwrap().tree_oid;
        write(&f.worktree.join("blob.bin"), &bin_v2);
        let t2 = f.snapshot.track().unwrap().tree_oid;

        let diffs = f.snapshot.diff_full(t1, t2).unwrap();
        let row = diffs.iter().find(|d| d.path == "blob.bin").unwrap();
        assert!(row.binary, "expected blob.bin to be flagged binary");
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
        assert!(matches!(
            action,
            ShadowRestoreAction::Wrote { .. } | ShadowRestoreAction::NoOp { .. }
        ));
    }
}

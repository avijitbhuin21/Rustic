use anyhow::{Context, Result};
use ignore::WalkBuilder;
use rustic_db::{CheckpointRow, Database, FileSnapshotRow};
use similar::{ChangeTag, TextDiff};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::{CheckpointInfo, DiffStatus, FileDiff, FileChange, TaskDiff};

/// Directories we never include in a project snapshot. These are either
/// regenerable build artifacts or tooling caches that would blow up snapshot
/// size for no benefit. `.gitignore` is intentionally NOT honored — user
/// files that happen to be gitignored (e.g. local configs) still get
/// snapshotted so revert restores the real working state.
pub const SNAPSHOT_SKIP_DIRS: &[&str] = &[
    ".git",
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
];

fn is_skip_dir_name(name: &str) -> bool {
    SNAPSHOT_SKIP_DIRS.iter().any(|s| *s == name)
}

/// Returns true if `s` looks like a UUID (32 hex chars + 4 hyphens, length 36).
/// We don't enforce v4-specific bits — any UUID-shaped string is fine, since
/// task / checkpoint IDs are always generated via `Uuid::new_v4()` internally.
fn is_uuid_shaped(s: &str) -> bool {
    s.len() == 36
        && s.bytes()
            .all(|b| b.is_ascii_hexdigit() || b == b'-')
        // Hyphen positions in canonical UUID form: 8-4-4-4-12.
        && &s[8..9] == "-"
        && &s[13..14] == "-"
        && &s[18..19] == "-"
        && &s[23..24] == "-"
}

/// Path on disk where a given checkpoint's project snapshot lives.
///
/// SECURITY: `task_id` and `checkpoint_id` are joined into a path that is
/// later passed to destructive ops (`remove_dir_all`, recursive copy). Even
/// though both ids are generated server-side from `Uuid::new_v4()` in normal
/// operation, defence-in-depth: validate the shape so a malicious caller
/// can't sneak a `..` segment through and escape the snapshot root.
pub fn snapshot_dir_for(snapshot_root: &Path, task_id: &str, checkpoint_id: &str) -> PathBuf {
    if !is_uuid_shaped(task_id) || !is_uuid_shaped(checkpoint_id) {
        // Return a deliberately-broken path so any subsequent `remove_dir_all`
        // / `read` attempt fails with NotFound rather than escaping the root.
        return snapshot_root
            .join("__INVALID_SNAPSHOT_ID__")
            .join("__INVALID_SNAPSHOT_ID__");
    }
    snapshot_root.join(task_id).join(checkpoint_id)
}

/// Copy the project's non-ignored content into `snap_dir`, mirroring the
/// directory structure. Called when a checkpoint is created so that a later
/// revert can restore the exact pre-turn state of the project — including
/// files and directories the user (not the agent) created.
pub fn create_project_snapshot(project_root: &Path, snap_dir: &Path) -> Result<()> {
    // Fresh snapshot — wipe any stale directory that happened to be there.
    if snap_dir.exists() {
        std::fs::remove_dir_all(snap_dir)
            .with_context(|| format!("Failed to clear stale snapshot dir: {}", snap_dir.display()))?;
    }
    std::fs::create_dir_all(snap_dir)
        .with_context(|| format!("Failed to create snapshot dir: {}", snap_dir.display()))?;

    let walker = WalkBuilder::new(project_root)
        .hidden(false)
        .git_ignore(false)
        .git_exclude(false)
        .ignore(false)
        .filter_entry(|entry| {
            // Skip the hardcoded heavy dirs. Files are always kept.
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                let name = entry.file_name().to_string_lossy();
                return !is_skip_dir_name(&name);
            }
            true
        })
        .build();

    for entry in walker.flatten() {
        let rel = match entry.path().strip_prefix(project_root) {
            Ok(p) => p,
            Err(_) => continue,
        };
        if rel.as_os_str().is_empty() {
            continue; // the root itself
        }
        let target = snap_dir.join(rel);
        let file_type = match entry.file_type() {
            Some(t) => t,
            None => continue,
        };
        if file_type.is_dir() {
            std::fs::create_dir_all(&target)
                .with_context(|| format!("Failed to create snapshot subdir: {}", target.display()))?;
        } else if file_type.is_file() {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            std::fs::copy(entry.path(), &target)
                .with_context(|| format!("Failed to copy file into snapshot: {}", entry.path().display()))?;
        }
        // Symlinks are intentionally skipped — restoring them portably is
        // fragile across platforms. If this becomes a problem we can extend.
    }

    Ok(())
}

/// Restore the project's non-ignored content from `snap_dir`. Deletes every
/// file and directory currently in the project that isn't in our skip list,
/// then copies the snapshot's contents back. Returns a best-effort list of
/// changes for UI display (files that were deleted or restored).
pub fn restore_project_snapshot(project_root: &Path, snap_dir: &Path) -> Result<Vec<FileChange>> {
    if !snap_dir.exists() {
        anyhow::bail!("Snapshot directory missing: {}", snap_dir.display());
    }

    // 1) Gather the set of files present in the snapshot so we can tell which
    //    currently-on-disk files are "extra" (created after the checkpoint).
    let mut snap_files: HashSet<PathBuf> = HashSet::new();
    for entry in WalkBuilder::new(snap_dir)
        .hidden(false).git_ignore(false).git_exclude(false).ignore(false)
        .build()
        .flatten()
    {
        if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            if let Ok(rel) = entry.path().strip_prefix(snap_dir) {
                snap_files.insert(rel.to_path_buf());
            }
        }
    }

    // 2) Walk the live project, collecting files + dirs for deletion. We
    //    collect first then delete so we don't mutate the iterator.
    let mut live_files: Vec<PathBuf> = Vec::new();
    let mut live_dirs: Vec<PathBuf> = Vec::new();
    for entry in WalkBuilder::new(project_root)
        .hidden(false).git_ignore(false).git_exclude(false).ignore(false)
        .filter_entry(|entry| {
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                let name = entry.file_name().to_string_lossy();
                return !is_skip_dir_name(&name);
            }
            true
        })
        .build()
        .flatten()
    {
        let p = entry.path();
        if p == project_root { continue; }
        match entry.file_type() {
            Some(t) if t.is_file() => live_files.push(p.to_path_buf()),
            Some(t) if t.is_dir()  => live_dirs.push(p.to_path_buf()),
            _ => {}
        }
    }

    let mut changes: Vec<FileChange> = Vec::new();

    // 3) Delete files that don't exist in the snapshot. Files that DO exist
    //    will be overwritten by the restore pass (no need to delete first).
    for p in &live_files {
        let rel = match p.strip_prefix(project_root) {
            Ok(r) => r,
            Err(_) => continue,
        };
        if !snap_files.contains(rel) {
            if std::fs::remove_file(p).is_ok() {
                changes.push(FileChange {
                    file_path: p.to_string_lossy().to_string(),
                    change_type: "delete".to_string(),
                });
            }
        }
    }

    // 4) Copy snapshot files back into the project. Deepest-first ordering
    //    isn't required here; create_dir_all handles missing parents.
    for entry in WalkBuilder::new(snap_dir)
        .hidden(false).git_ignore(false).git_exclude(false).ignore(false)
        .build()
        .flatten()
    {
        let ft = match entry.file_type() { Some(t) => t, None => continue };
        if !ft.is_file() { continue; }
        let rel = match entry.path().strip_prefix(snap_dir) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let target = project_root.join(rel);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        std::fs::copy(entry.path(), &target)
            .with_context(|| format!("Failed to restore file: {}", target.display()))?;
        changes.push(FileChange {
            file_path: target.to_string_lossy().to_string(),
            change_type: "restore".to_string(),
        });
    }

    // 5) Remove empty directories that are no longer populated. Deepest-first
    //    so a parent only gets tried after its children have been handled.
    live_dirs.sort_by_key(|p| std::cmp::Reverse(p.components().count()));
    for dir in &live_dirs {
        // `remove_dir` only succeeds if empty — so this is self-limiting.
        let _ = std::fs::remove_dir(dir);
    }

    Ok(changes)
}

/// Recursively delete a snapshot directory. Safe to call when the directory
/// doesn't exist — returns Ok in that case.
pub fn delete_snapshot_dir(snap_dir: &Path) -> Result<()> {
    if snap_dir.exists() {
        std::fs::remove_dir_all(snap_dir)
            .with_context(|| format!("Failed to delete snapshot dir: {}", snap_dir.display()))?;
    }
    Ok(())
}

/// Create a new checkpoint for the given task at the specified message index.
///
/// A checkpoint carries two things:
///   1. A DB row linking the task + message_index to a checkpoint id.
///   2. A mirror of the project directory saved to
///      `<snapshot_root>/<task_id>/<id>/` (excluding heavy build dirs).
///
/// Reverting to this checkpoint later restores the mirror onto the project.
pub fn create_checkpoint(
    db: &Database,
    task_id: &str,
    message_index: i64,
    project_root: &Path,
    snapshot_root: &Path,
) -> Result<String> {
    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();

    db.insert_checkpoint(&CheckpointRow {
        id: id.clone(),
        task_id: task_id.to_string(),
        message_index,
        created_at: now,
    })?;

    let snap_dir = snapshot_dir_for(snapshot_root, task_id, &id);
    create_project_snapshot(project_root, &snap_dir)
        .with_context(|| format!("Failed to snapshot project for checkpoint {}", id))?;

    Ok(id)
}

/// Snapshot a file's current content into a checkpoint.
/// If the file doesn't exist yet (is_new = true), the revert will delete it.
pub fn snapshot_file(db: &Database, checkpoint_id: &str, file_path: &Path) -> Result<()> {
    let path_str = file_path.to_string_lossy().to_string();

    let (content, was_new) = if file_path.exists() {
        (std::fs::read(file_path)
            .with_context(|| format!("Failed to read file for snapshot: {}", path_str))?,
         false)
    } else {
        // File doesn't exist yet — mark as new so revert will delete it
        (Vec::new(), true)
    };

    let snap_id = uuid::Uuid::new_v4().to_string();
    db.insert_file_snapshot(&FileSnapshotRow {
        id: snap_id,
        checkpoint_id: checkpoint_id.to_string(),
        file_path: path_str,
        content,
        was_new,
    })?;

    Ok(())
}


/// Revert the project to the state it was in when `checkpoint_id` was
/// created. Restores from the on-disk project snapshot at
/// `<snapshot_root>/<task_id>/<checkpoint_id>/`, which is an exact mirror of
/// the project at that moment (excluding heavy build dirs).
///
/// Any files currently on disk that weren't in the snapshot are deleted, and
/// any files in the snapshot are copied back. This is "replace the fork with
/// the saved branch" semantics — nothing the agent or user did since the
/// checkpoint survives, and empty user-created dirs that existed at the time
/// are preserved because they were captured in the snapshot.
pub fn revert_to(
    db: &Database,
    checkpoint_id: &str,
    project_root: &Path,
    snapshot_root: &Path,
) -> Result<Vec<FileChange>> {
    let checkpoint = db
        .get_checkpoint(checkpoint_id)?
        .ok_or_else(|| anyhow::anyhow!("Checkpoint not found: {}", checkpoint_id))?;

    let snap_dir = snapshot_dir_for(snapshot_root, &checkpoint.task_id, checkpoint_id);
    restore_project_snapshot(project_root, &snap_dir)
}

/// List all checkpoints for a task with file counts.
pub fn list_checkpoints(db: &Database, task_id: &str) -> Result<Vec<CheckpointInfo>> {
    let rows = db.list_checkpoints(task_id)?;
    let mut infos = Vec::new();

    for row in rows {
        let snapshots = db.get_file_snapshots(&row.id)?;
        infos.push(CheckpointInfo {
            id: row.id,
            task_id: row.task_id,
            message_index: row.message_index,
            created_at: row.created_at,
            file_count: snapshots.len(),
        });
    }

    Ok(infos)
}

/// Preview what would happen if we revert to a checkpoint. Diffs the saved
/// project snapshot against the current state of the project and reports:
///   - Files present on disk but not in the snapshot → "delete" (created
///     after the checkpoint and will be removed).
///   - Files in the snapshot but missing on disk → "restore" (will be
///     re-created).
/// Content-only differences are intentionally skipped to keep the list
/// concise; they'll silently be rewritten on revert.
pub fn preview_checkpoint(
    db: &Database,
    checkpoint_id: &str,
    project_root: &Path,
    snapshot_root: &Path,
) -> Result<Vec<FileChange>> {
    let checkpoint = db
        .get_checkpoint(checkpoint_id)?
        .ok_or_else(|| anyhow::anyhow!("Checkpoint not found: {}", checkpoint_id))?;
    let snap_dir = snapshot_dir_for(snapshot_root, &checkpoint.task_id, checkpoint_id);

    if !snap_dir.exists() {
        return Ok(Vec::new());
    }

    // Collect the snapshot's file set (relative paths).
    let mut snap_files: HashSet<PathBuf> = HashSet::new();
    for entry in WalkBuilder::new(&snap_dir)
        .hidden(false).git_ignore(false).git_exclude(false).ignore(false)
        .build()
        .flatten()
    {
        if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            if let Ok(rel) = entry.path().strip_prefix(&snap_dir) {
                snap_files.insert(rel.to_path_buf());
            }
        }
    }

    // Collect the live project's file set (relative paths, same skip rules).
    let mut live_files: HashSet<PathBuf> = HashSet::new();
    for entry in WalkBuilder::new(project_root)
        .hidden(false).git_ignore(false).git_exclude(false).ignore(false)
        .filter_entry(|entry| {
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                let name = entry.file_name().to_string_lossy();
                return !is_skip_dir_name(&name);
            }
            true
        })
        .build()
        .flatten()
    {
        if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            if let Ok(rel) = entry.path().strip_prefix(project_root) {
                live_files.insert(rel.to_path_buf());
            }
        }
    }

    let mut changes: Vec<FileChange> = Vec::new();
    for p in &live_files {
        if !snap_files.contains(p) {
            changes.push(FileChange {
                file_path: p.to_string_lossy().replace('\\', "/"),
                change_type: "delete".to_string(),
            });
        }
    }
    for p in &snap_files {
        if !live_files.contains(p) {
            changes.push(FileChange {
                file_path: p.to_string_lossy().replace('\\', "/"),
                change_type: "restore".to_string(),
            });
        }
    }
    changes.sort_by(|a, b| a.file_path.cmp(&b.file_path));
    Ok(changes)
}

/// Delete all checkpoints for a task.
pub fn delete_task_checkpoints(db: &Database, task_id: &str) -> Result<()> {
    db.delete_task_checkpoints(task_id)?;
    Ok(())
}

/// Build a TaskDiff by comparing two file trees: a "before" tree (the
/// snapshot dir taken at message-send time) and an "after" tree (either the
/// next checkpoint's snapshot or the live project root). Replaces the old
/// per-tool DB-row diff path now that snapshots only fire on user message.
fn diff_two_trees(before_dir: &Path, after_dir: &Path) -> Result<TaskDiff> {
    let mut before_files: HashMap<PathBuf, PathBuf> = HashMap::new();
    if before_dir.exists() {
        for entry in WalkBuilder::new(before_dir)
            .hidden(false)
            .git_ignore(false)
            .git_exclude(false)
            .ignore(false)
            .build()
            .flatten()
        {
            if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                if let Ok(rel) = entry.path().strip_prefix(before_dir) {
                    before_files.insert(rel.to_path_buf(), entry.path().to_path_buf());
                }
            }
        }
    }

    let mut after_files: HashMap<PathBuf, PathBuf> = HashMap::new();
    if after_dir.exists() {
        for entry in WalkBuilder::new(after_dir)
            .hidden(false)
            .git_ignore(false)
            .git_exclude(false)
            .ignore(false)
            .filter_entry(|entry| {
                if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    let name = entry.file_name().to_string_lossy();
                    return !is_skip_dir_name(&name);
                }
                true
            })
            .build()
            .flatten()
        {
            if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                if let Ok(rel) = entry.path().strip_prefix(after_dir) {
                    after_files.insert(rel.to_path_buf(), entry.path().to_path_buf());
                }
            }
        }
    }

    let mut all_paths: HashSet<PathBuf> = HashSet::new();
    all_paths.extend(before_files.keys().cloned());
    all_paths.extend(after_files.keys().cloned());

    let mut files: Vec<FileDiff> = Vec::new();
    let mut total_insertions = 0usize;
    let mut total_deletions = 0usize;

    for rel in all_paths {
        let before_path = before_files.get(&rel);
        let after_path = after_files.get(&rel);

        let before_text = before_path
            .and_then(|p| std::fs::read_to_string(p).ok())
            .unwrap_or_default();
        let after_text = after_path
            .and_then(|p| std::fs::read_to_string(p).ok())
            .unwrap_or_default();

        if before_text == after_text {
            continue;
        }

        let status = match (before_path.is_some(), after_path.is_some()) {
            (false, true) => DiffStatus::Created,
            (true, false) => DiffStatus::Deleted,
            _ => DiffStatus::Modified,
        };

        let diff = TextDiff::from_lines(&before_text, &after_text);
        let mut insertions = 0usize;
        let mut deletions = 0usize;
        for change in diff.iter_all_changes() {
            match change.tag() {
                ChangeTag::Insert => insertions += 1,
                ChangeTag::Delete => deletions += 1,
                ChangeTag::Equal => {}
            }
        }
        if insertions == 0 && deletions == 0 && status == DiffStatus::Modified {
            continue;
        }

        let path_str = rel.to_string_lossy().replace('\\', "/");
        let unified_diff = diff
            .unified_diff()
            .header(&format!("a/{}", path_str), &format!("b/{}", path_str))
            .to_string();

        total_insertions += insertions;
        total_deletions += deletions;

        files.push(FileDiff {
            path: path_str,
            status,
            insertions,
            deletions,
            unified_diff,
        });
    }

    files.sort_by(|a, b| a.path.cmp(&b.path));

    Ok(TaskDiff {
        files,
        total_insertions,
        total_deletions,
    })
}

/// Compute a diff of files changed during a specific checkpoint turn.
///
/// Compares this checkpoint's snapshot (= state right before the user's
/// message was processed) to either the next checkpoint's snapshot (= state
/// at the start of the next turn) or the live project root if this is the
/// most recent checkpoint.
pub fn compute_checkpoint_diff(
    db: &Database,
    task_id: &str,
    checkpoint_id: &str,
    project_root: &Path,
    snapshot_root: &Path,
) -> Result<TaskDiff> {
    let this_snap_dir = snapshot_dir_for(snapshot_root, task_id, checkpoint_id);
    if !this_snap_dir.exists() {
        return Ok(TaskDiff { files: Vec::new(), total_insertions: 0, total_deletions: 0 });
    }

    // Find the immediately-following checkpoint by message_index.
    let checkpoints = db.list_checkpoints(task_id)?;
    let this_message_index = checkpoints
        .iter()
        .find(|c| c.id == checkpoint_id)
        .map(|c| c.message_index);
    let next_cp = match this_message_index {
        Some(idx) => checkpoints
            .iter()
            .filter(|c| c.message_index > idx)
            .min_by_key(|c| c.message_index),
        None => None,
    };

    let after_dir = match next_cp {
        Some(c) => snapshot_dir_for(snapshot_root, task_id, &c.id),
        None => project_root.to_path_buf(),
    };

    diff_two_trees(&this_snap_dir, &after_dir)
}

/// Compute a diff of every file changed during a task.
///
/// Compares the EARLIEST checkpoint's snapshot (= state at the start of the
/// task, before the first user message) to the current project root. With
/// per-tool snapshotting removed, this is the only meaningful "task diff"
/// view available — there is no per-tool granularity in the snapshot trail.
pub fn compute_task_diff(
    db: &Database,
    task_id: &str,
    project_root: &Path,
    snapshot_root: &Path,
) -> Result<TaskDiff> {
    let checkpoints = db.list_checkpoints(task_id)?;
    let earliest = checkpoints.iter().min_by_key(|c| c.message_index);
    let Some(cp) = earliest else {
        return Ok(TaskDiff { files: Vec::new(), total_insertions: 0, total_deletions: 0 });
    };

    let snap_dir = snapshot_dir_for(snapshot_root, task_id, &cp.id);
    diff_two_trees(&snap_dir, project_root)
}

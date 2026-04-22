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

/// Path on disk where a given checkpoint's project snapshot lives.
pub fn snapshot_dir_for(snapshot_root: &Path, task_id: &str, checkpoint_id: &str) -> PathBuf {
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

/// Compute a diff of files changed during a specific checkpoint turn.
///
/// Shows exactly what changed between the start of the turn (before any writes)
/// and the end of the turn (after all writes completed).
///
/// - Before state: earliest snapshot per file in this checkpoint (taken before first write)
/// - After state: earliest snapshot of that file in any *later* checkpoint (= state at start
///   of the next turn that touches it), or current disk state if none exists yet.
pub fn compute_checkpoint_diff(db: &Database, task_id: &str, checkpoint_id: &str) -> Result<TaskDiff> {
    // Get all snapshots for this checkpoint (before-states)
    let snapshots = db.get_file_snapshots(checkpoint_id)?;

    // Deduplicate: keep first snapshot per file (the true before-state for this turn)
    let mut before_states: HashMap<String, FileSnapshotRow> = HashMap::new();
    for snap in snapshots {
        before_states.entry(snap.file_path.clone()).or_insert(snap);
    }

    if before_states.is_empty() {
        return Ok(TaskDiff { files: Vec::new(), total_insertions: 0, total_deletions: 0 });
    }

    // Get the message_index for this checkpoint so we can find later snapshots
    let checkpoints = db.list_checkpoints(task_id)?;
    let this_message_index = checkpoints
        .iter()
        .find(|c| c.id == checkpoint_id)
        .map(|c| c.message_index)
        .unwrap_or(i64::MAX);

    // Get all snapshots from later checkpoints (to find after-states)
    let later_snapshots = db.get_file_snapshots_after_message(task_id, this_message_index)?;

    // For each file modified in this turn, find the earliest later snapshot = after-state
    let mut after_states: HashMap<String, FileSnapshotRow> = HashMap::new();
    for snap in later_snapshots {
        after_states.entry(snap.file_path.clone()).or_insert(snap);
    }

    let mut files: Vec<FileDiff> = Vec::new();
    let mut total_insertions = 0usize;
    let mut total_deletions = 0usize;

    for (file_path, before_snap) in &before_states {
        let before_text = String::from_utf8_lossy(&before_snap.content).into_owned();

        // Determine after state
        let (status, after_text) = if let Some(later) = after_states.get(file_path) {
            // There's a later snapshot — that's the after state for this turn
            let after = String::from_utf8_lossy(&later.content).into_owned();
            if before_snap.was_new {
                (DiffStatus::Created, after)
            } else {
                (DiffStatus::Modified, after)
            }
        } else {
            // No later snapshot — use current disk state
            let path = Path::new(file_path);
            if path.exists() {
                let after = std::fs::read_to_string(path).unwrap_or_default();
                if before_snap.was_new {
                    (DiffStatus::Created, after)
                } else {
                    (DiffStatus::Modified, after)
                }
            } else if !before_snap.was_new {
                // Existed before, deleted now
                (DiffStatus::Deleted, String::new())
            } else {
                // Was new and no longer exists — created then deleted, skip
                continue;
            }
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

        let unified_diff = diff
            .unified_diff()
            .header(&format!("a/{}", file_path), &format!("b/{}", file_path))
            .to_string();

        total_insertions += insertions;
        total_deletions += deletions;

        files.push(FileDiff {
            path: file_path.clone(),
            status,
            insertions,
            deletions,
            unified_diff,
        });
    }

    files.sort_by(|a, b| a.path.cmp(&b.path));

    Ok(TaskDiff { files, total_insertions, total_deletions })
}

/// Compute a diff of all files changed during a task.
///
/// Collects all FileSnapshotRow records for this task across all checkpoints,
/// deduplicates by file path keeping the earliest snapshot per file (true pre-task state),
/// then compares to the current file on disk to produce a unified diff.
pub fn compute_task_diff(db: &Database, task_id: &str) -> Result<TaskDiff> {
    let all_snapshots = db.get_all_file_snapshots_for_task(task_id)?;

    // Deduplicate: keep the first (earliest) snapshot per file path.
    let mut earliest: HashMap<String, FileSnapshotRow> = HashMap::new();
    for snap in all_snapshots {
        earliest.entry(snap.file_path.clone()).or_insert(snap);
    }

    let mut files: Vec<FileDiff> = Vec::new();
    let mut total_insertions = 0usize;
    let mut total_deletions = 0usize;

    for (file_path, snap) in earliest {
        let current_exists = Path::new(&file_path).exists();

        let (status, before_text, after_text) = if snap.was_new && current_exists {
            // File was created by the agent
            let after = std::fs::read_to_string(&file_path).unwrap_or_default();
            (DiffStatus::Created, String::new(), after)
        } else if !snap.was_new && current_exists {
            // File existed before and still exists — modified
            let before = String::from_utf8_lossy(&snap.content).into_owned();
            let after = std::fs::read_to_string(&file_path).unwrap_or_default();
            (DiffStatus::Modified, before, after)
        } else if !snap.was_new && !current_exists {
            // File existed before but was deleted
            let before = String::from_utf8_lossy(&snap.content).into_owned();
            (DiffStatus::Deleted, before, String::new())
        } else {
            // was_new=true and file no longer exists — created then deleted, skip
            continue;
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

        // Only include files that actually changed
        if insertions == 0 && deletions == 0 && status == DiffStatus::Modified {
            continue;
        }

        let unified_diff = diff
            .unified_diff()
            .header(&format!("a/{}", file_path), &format!("b/{}", file_path))
            .to_string();

        total_insertions += insertions;
        total_deletions += deletions;

        files.push(FileDiff {
            path: file_path,
            status,
            insertions,
            deletions,
            unified_diff,
        });
    }

    // Sort by path for consistent ordering
    files.sort_by(|a, b| a.path.cmp(&b.path));

    Ok(TaskDiff {
        files,
        total_insertions,
        total_deletions,
    })
}

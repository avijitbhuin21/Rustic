use anyhow::{Context, Result};
use rustic_db::{CheckpointRow, Database, FileSnapshotRow};
use similar::{ChangeTag, TextDiff};
use std::collections::HashMap;
use std::path::Path;

use super::{CheckpointInfo, DiffStatus, FileDiff, FileChange, TaskDiff};

/// Create a new checkpoint for the given task at the specified message index.
pub fn create_checkpoint(db: &Database, task_id: &str, message_index: i64) -> Result<String> {
    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();

    db.insert_checkpoint(&CheckpointRow {
        id: id.clone(),
        task_id: task_id.to_string(),
        message_index,
        created_at: now,
    })?;

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

/// Revert all files to the state captured in the given checkpoint.
/// For files that were new (didn't exist before), deletes them.
/// For files that existed, restores their original content.
pub fn revert_to(db: &Database, checkpoint_id: &str) -> Result<Vec<FileChange>> {
    let snapshots = db.get_file_snapshots(checkpoint_id)?;
    let mut changes = Vec::new();

    for snap in &snapshots {
        let path = Path::new(&snap.file_path);
        if snap.was_new {
            // File was created by the agent — delete it
            if path.exists() {
                std::fs::remove_file(path)
                    .with_context(|| format!("Failed to delete file: {}", snap.file_path))?;
            }
            changes.push(FileChange {
                file_path: snap.file_path.clone(),
                change_type: "delete".to_string(),
            });
        } else {
            // Restore original content
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            std::fs::write(path, &snap.content)
                .with_context(|| format!("Failed to restore file: {}", snap.file_path))?;
            changes.push(FileChange {
                file_path: snap.file_path.clone(),
                change_type: "restore".to_string(),
            });
        }
    }

    Ok(changes)
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

/// Preview what would happen if we revert to a checkpoint.
pub fn preview_checkpoint(db: &Database, checkpoint_id: &str) -> Result<Vec<FileChange>> {
    let snapshots = db.get_file_snapshots(checkpoint_id)?;
    let mut changes = Vec::new();

    for snap in &snapshots {
        changes.push(FileChange {
            file_path: snap.file_path.clone(),
            change_type: if snap.was_new { "delete".to_string() } else { "restore".to_string() },
        });
    }

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

use anyhow::{Context, Result};
use rustic_db::{CheckpointRow, Database, FileSnapshotRow};
use std::path::Path;

use super::{CheckpointInfo, FileChange};

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

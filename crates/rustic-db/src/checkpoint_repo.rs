use anyhow::Result;
use rusqlite::params;

use crate::connection::Database;
use crate::models::{CheckpointRow, FileSnapshotRow};

impl Database {
    pub fn insert_checkpoint(&self, checkpoint: &CheckpointRow) -> Result<()> {
        self.conn().execute(
            "INSERT INTO checkpoints (id, task_id, message_index, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![checkpoint.id, checkpoint.task_id, checkpoint.message_index, checkpoint.created_at],
        )?;
        Ok(())
    }

    pub fn list_checkpoints(&self, task_id: &str) -> Result<Vec<CheckpointRow>> {
        let mut stmt = self.conn().prepare(
            "SELECT id, task_id, message_index, created_at
             FROM checkpoints WHERE task_id = ?1 ORDER BY message_index"
        )?;
        let rows = stmt.query_map(params![task_id], |row| {
            Ok(CheckpointRow {
                id: row.get(0)?,
                task_id: row.get(1)?,
                message_index: row.get(2)?,
                created_at: row.get(3)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn get_checkpoint(&self, id: &str) -> Result<Option<CheckpointRow>> {
        let mut stmt = self.conn().prepare(
            "SELECT id, task_id, message_index, created_at FROM checkpoints WHERE id = ?1"
        )?;
        let mut rows = stmt.query_map(params![id], |row| {
            Ok(CheckpointRow {
                id: row.get(0)?,
                task_id: row.get(1)?,
                message_index: row.get(2)?,
                created_at: row.get(3)?,
            })
        })?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    pub fn delete_task_checkpoints(&self, task_id: &str) -> Result<()> {
        self.conn().execute(
            "DELETE FROM checkpoints WHERE task_id = ?1",
            params![task_id],
        )?;
        Ok(())
    }

    pub fn insert_file_snapshot(&self, snapshot: &FileSnapshotRow) -> Result<()> {
        self.conn().execute(
            "INSERT INTO file_snapshots (id, checkpoint_id, file_path, content, was_new)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                snapshot.id, snapshot.checkpoint_id, snapshot.file_path,
                snapshot.content, snapshot.was_new
            ],
        )?;
        Ok(())
    }

    pub fn get_file_snapshots(&self, checkpoint_id: &str) -> Result<Vec<FileSnapshotRow>> {
        let mut stmt = self.conn().prepare(
            "SELECT id, checkpoint_id, file_path, content, was_new
             FROM file_snapshots WHERE checkpoint_id = ?1"
        )?;
        let rows = stmt.query_map(params![checkpoint_id], |row| {
            Ok(FileSnapshotRow {
                id: row.get(0)?,
                checkpoint_id: row.get(1)?,
                file_path: row.get(2)?,
                content: row.get(3)?,
                was_new: row.get(4)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Get all file snapshots from checkpoints that come AFTER the given message_index
    /// for the same task. Used to find the "after state" of files changed in a specific turn.
    pub fn get_file_snapshots_after_message(
        &self,
        task_id: &str,
        after_message_index: i64,
    ) -> Result<Vec<FileSnapshotRow>> {
        let mut stmt = self.conn().prepare(
            "SELECT fs.id, fs.checkpoint_id, fs.file_path, fs.content, fs.was_new
             FROM file_snapshots fs
             JOIN checkpoints c ON fs.checkpoint_id = c.id
             WHERE c.task_id = ?1 AND c.message_index > ?2
             ORDER BY c.message_index ASC"
        )?;
        let rows = stmt.query_map(params![task_id, after_message_index], |row| {
            Ok(FileSnapshotRow {
                id: row.get(0)?,
                checkpoint_id: row.get(1)?,
                file_path: row.get(2)?,
                content: row.get(3)?,
                was_new: row.get(4)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn get_all_file_snapshots_for_task(&self, task_id: &str) -> Result<Vec<FileSnapshotRow>> {
        let mut stmt = self.conn().prepare(
            "SELECT fs.id, fs.checkpoint_id, fs.file_path, fs.content, fs.was_new
             FROM file_snapshots fs
             JOIN checkpoints c ON fs.checkpoint_id = c.id
             WHERE c.task_id = ?1
             ORDER BY c.message_index ASC"
        )?;
        let rows = stmt.query_map(params![task_id], |row| {
            Ok(FileSnapshotRow {
                id: row.get(0)?,
                checkpoint_id: row.get(1)?,
                file_path: row.get(2)?,
                content: row.get(3)?,
                was_new: row.get(4)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn get_snapshots_for_task_up_to(&self, task_id: &str, checkpoint_id: &str) -> Result<Vec<FileSnapshotRow>> {
        let mut stmt = self.conn().prepare(
            "SELECT fs.id, fs.checkpoint_id, fs.file_path, fs.content, fs.was_new
             FROM file_snapshots fs
             JOIN checkpoints c ON fs.checkpoint_id = c.id
             WHERE c.task_id = ?1 AND c.message_index <= (
                SELECT message_index FROM checkpoints WHERE id = ?2
             )
             ORDER BY c.message_index"
        )?;
        let rows = stmt.query_map(params![task_id, checkpoint_id], |row| {
            Ok(FileSnapshotRow {
                id: row.get(0)?,
                checkpoint_id: row.get(1)?,
                file_path: row.get(2)?,
                content: row.get(3)?,
                was_new: row.get(4)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }
}

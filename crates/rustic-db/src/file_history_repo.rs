//! Repository for the changed-files tracker tables introduced in migration 010.
//!
//! Three tables:
//! - `file_history_snapshots` — one row per user-message anchor.
//! - `file_history_files`     — one row per (snapshot, path) with optional blob hash.
//! - `file_history_blobs`     — index of blob hashes on disk + ref_count.
//!
//! Blob *content* lives on disk; only the index is here. `ref_count` is
//! maintained automatically by triggers (see migrations/010_file_history.sql).

use rusqlite::{params, OptionalExtension};

use crate::connection::Database;
use crate::error::Result;
use crate::models::{FileHistoryBlobRow, FileHistoryFileRow, FileHistorySnapshotRow};

impl Database {
    // -------- snapshots --------

    /// Insert (or no-op) a snapshot row for the given user message.
    /// `sequence` should be a per-task monotonically increasing integer.
    pub fn fh_insert_snapshot(&self, message_id: &str, task_id: &str, sequence: i64) -> Result<()> {
        self.conn().execute(
            "INSERT OR IGNORE INTO file_history_snapshots (message_id, task_id, sequence)
             VALUES (?1, ?2, ?3)",
            params![message_id, task_id, sequence],
        )?;
        Ok(())
    }

    /// Highest `sequence` already used for this task, or 0 if none.
    pub fn fh_max_sequence_for_task(&self, task_id: &str) -> Result<i64> {
        let mut stmt = self.conn().prepare_cached(
            "SELECT COALESCE(MAX(sequence), 0) FROM file_history_snapshots WHERE task_id = ?1",
        )?;
        let n: i64 = stmt.query_row(params![task_id], |row| row.get(0))?;
        Ok(n)
    }

    pub fn fh_get_snapshot(&self, message_id: &str) -> Result<Option<FileHistorySnapshotRow>> {
        let mut stmt = self.conn().prepare_cached(
            "SELECT message_id, task_id, sequence, created_at
             FROM file_history_snapshots WHERE message_id = ?1",
        )?;
        Ok(stmt
            .query_row(params![message_id], |row| {
                Ok(FileHistorySnapshotRow {
                    message_id: row.get(0)?,
                    task_id: row.get(1)?,
                    sequence: row.get(2)?,
                    created_at: row.get(3)?,
                })
            })
            .optional()?)
    }

    /// Snapshots for a task in chronological (sequence ASC) order.
    pub fn fh_list_snapshots_for_task(&self, task_id: &str) -> Result<Vec<FileHistorySnapshotRow>> {
        let mut stmt = self.conn().prepare_cached(
            "SELECT message_id, task_id, sequence, created_at
             FROM file_history_snapshots
             WHERE task_id = ?1
             ORDER BY sequence ASC",
        )?;
        let rows = stmt.query_map(params![task_id], |row| {
            Ok(FileHistorySnapshotRow {
                message_id: row.get(0)?,
                task_id: row.get(1)?,
                sequence: row.get(2)?,
                created_at: row.get(3)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Delete the oldest snapshots beyond `keep_last_n` for this task. Returns
    /// the number of snapshots deleted. ON DELETE CASCADE removes their files;
    /// triggers decrement blob ref_counts.
    pub fn fh_evict_old_snapshots(&self, task_id: &str, keep_last_n: i64) -> Result<usize> {
        let n = self.conn().execute(
            "DELETE FROM file_history_snapshots
             WHERE message_id IN (
                 SELECT message_id FROM file_history_snapshots
                 WHERE task_id = ?1
                 ORDER BY sequence DESC
                 LIMIT -1 OFFSET ?2
             )",
            params![task_id, keep_last_n],
        )?;
        Ok(n)
    }

    // -------- files within a snapshot --------

    /// Upsert a (snapshot, path) -> blob_hash entry. `blob_hash = None` records
    /// "did not exist at this version".
    ///
    /// Implemented as explicit DELETE + INSERT in one transaction so both
    /// `AFTER DELETE` and `AFTER INSERT` triggers fire on the row replacement.
    /// `INSERT OR REPLACE` in SQLite does NOT reliably fire the delete trigger
    /// for the conflict-resolved row across versions — that asymmetry would
    /// leave blob ref_counts wrong on every upsert path.
    pub fn fh_upsert_file(
        &self,
        message_id: &str,
        path: &str,
        blob_hash: Option<&str>,
        mtime_ns: Option<i64>,
        size: Option<i64>,
    ) -> Result<()> {
        let tx = self.conn().unchecked_transaction()?;
        tx.execute(
            "DELETE FROM file_history_files WHERE message_id = ?1 AND path = ?2",
            params![message_id, path],
        )?;
        tx.execute(
            "INSERT INTO file_history_files (message_id, path, blob_hash, mtime_ns, size)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![message_id, path, blob_hash, mtime_ns, size],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn fh_get_file(
        &self,
        message_id: &str,
        path: &str,
    ) -> Result<Option<FileHistoryFileRow>> {
        let mut stmt = self.conn().prepare_cached(
            "SELECT message_id, path, blob_hash, mtime_ns, size
             FROM file_history_files WHERE message_id = ?1 AND path = ?2",
        )?;
        Ok(stmt
            .query_row(params![message_id, path], |row| {
                Ok(FileHistoryFileRow {
                    message_id: row.get(0)?,
                    path: row.get(1)?,
                    blob_hash: row.get(2)?,
                    mtime_ns: row.get(3)?,
                    size: row.get(4)?,
                })
            })
            .optional()?)
    }

    pub fn fh_list_files_for_snapshot(
        &self,
        message_id: &str,
    ) -> Result<Vec<FileHistoryFileRow>> {
        let mut stmt = self.conn().prepare_cached(
            "SELECT message_id, path, blob_hash, mtime_ns, size
             FROM file_history_files WHERE message_id = ?1
             ORDER BY path ASC",
        )?;
        let rows = stmt.query_map(params![message_id], |row| {
            Ok(FileHistoryFileRow {
                message_id: row.get(0)?,
                path: row.get(1)?,
                blob_hash: row.get(2)?,
                mtime_ns: row.get(3)?,
                size: row.get(4)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// For each distinct path that appears in any snapshot of the given task,
    /// return the row from the LATEST snapshot (highest sequence) that has
    /// recorded that path. Used by the open_snapshot pre-capture path to
    /// decide which paths to re-stat at turn-start and whether the cached
    /// (mtime_ns, size) lets us reuse a prior blob_hash without re-hashing.
    pub fn fh_latest_files_for_task(
        &self,
        task_id: &str,
    ) -> Result<Vec<FileHistoryFileRow>> {
        let mut stmt = self.conn().prepare_cached(
            "SELECT f.message_id, f.path, f.blob_hash, f.mtime_ns, f.size
             FROM file_history_files f
             JOIN file_history_snapshots s ON f.message_id = s.message_id
             WHERE s.task_id = ?1
               AND s.sequence = (
                 SELECT MAX(s2.sequence)
                 FROM file_history_files f2
                 JOIN file_history_snapshots s2 ON f2.message_id = s2.message_id
                 WHERE s2.task_id = ?1 AND f2.path = f.path
               )
             ORDER BY f.path ASC",
        )?;
        let rows = stmt.query_map(params![task_id], |row| {
            Ok(FileHistoryFileRow {
                message_id: row.get(0)?,
                path: row.get(1)?,
                blob_hash: row.get(2)?,
                mtime_ns: row.get(3)?,
                size: row.get(4)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    // -------- blobs --------

    /// Register a blob in the index. Idempotent; returns false if the row
    /// already existed (caller can skip the disk write in that case).
    /// ref_count starts at 0 and is bumped by file-row triggers.
    pub fn fh_register_blob(&self, hash: &str, size: i64) -> Result<bool> {
        let n = self.conn().execute(
            "INSERT OR IGNORE INTO file_history_blobs (hash, size) VALUES (?1, ?2)",
            params![hash, size],
        )?;
        Ok(n > 0)
    }

    pub fn fh_blob_exists(&self, hash: &str) -> Result<bool> {
        let mut stmt = self
            .conn()
            .prepare_cached("SELECT 1 FROM file_history_blobs WHERE hash = ?1")?;
        Ok(stmt
            .query_row(params![hash], |_| Ok(()))
            .optional()?
            .is_some())
    }

    /// Hashes whose ref_count has dropped to zero. Caller is responsible for
    /// unlinking the blob file on disk and then calling `fh_delete_blobs`.
    pub fn fh_unreferenced_blobs(&self) -> Result<Vec<FileHistoryBlobRow>> {
        let mut stmt = self.conn().prepare_cached(
            "SELECT hash, size, ref_count, created_at
             FROM file_history_blobs WHERE ref_count <= 0",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(FileHistoryBlobRow {
                hash: row.get(0)?,
                size: row.get(1)?,
                ref_count: row.get(2)?,
                created_at: row.get(3)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Drop blob rows by hash (after the on-disk file has been unlinked).
    pub fn fh_delete_blobs(&self, hashes: &[String]) -> Result<usize> {
        if hashes.is_empty() {
            return Ok(0);
        }
        let tx = self.conn().unchecked_transaction()?;
        let mut deleted = 0usize;
        {
            let mut stmt =
                tx.prepare("DELETE FROM file_history_blobs WHERE hash = ?1 AND ref_count <= 0")?;
            for hash in hashes {
                deleted += stmt.execute(params![hash])?;
            }
        }
        tx.commit()?;
        Ok(deleted)
    }

    /// All hashes currently in the index — used by the startup reconciliation
    /// pass to find orphan blob *files* on disk.
    pub fn fh_all_blob_hashes(&self) -> Result<Vec<String>> {
        let mut stmt = self
            .conn()
            .prepare_cached("SELECT hash FROM file_history_blobs")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> Database {
        Database::in_memory().expect("in-memory db")
    }

    fn seed_task(db: &Database, id: &str) {
        // Manual insert to avoid threading the full TaskRow shape through the
        // tracker tests — we only need the FK target.
        db.conn()
            .execute(
                "INSERT INTO projects (id, name, root_path) VALUES ('p', 'p', 'p')",
                [],
            )
            .ok(); // ok if already exists from a prior test in the same DB
        db.conn()
            .execute(
                "INSERT INTO tasks (id, project_id, title, status, provider_type, model)
                 VALUES (?1, 'p', 't', 'created', 'native', 'm')",
                params![id],
            )
            .expect("insert task");
    }

    #[test]
    fn snapshot_lifecycle_maintains_refcount() {
        let db = fresh();
        seed_task(&db, "task-1");

        // Snapshot 1: file foo.txt with blob "h1"
        db.fh_insert_snapshot("msg-1", "task-1", 1).unwrap();
        db.fh_register_blob("h1", 100).unwrap();
        db.fh_upsert_file("msg-1", "foo.txt", Some("h1"), None, None).unwrap();

        let blob: i64 = db
            .conn()
            .query_row(
                "SELECT ref_count FROM file_history_blobs WHERE hash = 'h1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(blob, 1, "insert trigger should bump ref_count to 1");

        // Snapshot 2: same file now points at "h2"; "h1" still referenced by msg-1
        db.fh_insert_snapshot("msg-2", "task-1", 2).unwrap();
        db.fh_register_blob("h2", 120).unwrap();
        db.fh_upsert_file("msg-2", "foo.txt", Some("h2"), None, None).unwrap();

        let h1_rc: i64 = db
            .conn()
            .query_row(
                "SELECT ref_count FROM file_history_blobs WHERE hash = 'h1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let h2_rc: i64 = db
            .conn()
            .query_row(
                "SELECT ref_count FROM file_history_blobs WHERE hash = 'h2'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(h1_rc, 1);
        assert_eq!(h2_rc, 1);

        // Evict snapshot 1 -> h1 ref_count drops to 0
        let evicted = db.fh_evict_old_snapshots("task-1", 1).unwrap();
        assert_eq!(evicted, 1);

        let unref = db.fh_unreferenced_blobs().unwrap();
        let unref_hashes: Vec<_> = unref.into_iter().map(|b| b.hash).collect();
        assert_eq!(unref_hashes, vec!["h1".to_string()]);

        // Caller would unlink h1 on disk, then:
        let dropped = db.fh_delete_blobs(&["h1".to_string()]).unwrap();
        assert_eq!(dropped, 1);

        // h2 still referenced
        assert!(db.fh_blob_exists("h2").unwrap());
        assert!(!db.fh_blob_exists("h1").unwrap());
    }

    #[test]
    fn null_blob_hash_records_did_not_exist() {
        let db = fresh();
        seed_task(&db, "task-2");

        db.fh_insert_snapshot("msg-x", "task-2", 1).unwrap();
        db.fh_upsert_file("msg-x", "new.txt", None, None, None).unwrap();

        let row = db.fh_get_file("msg-x", "new.txt").unwrap().unwrap();
        assert!(row.blob_hash.is_none());

        // No blob row should have been touched.
        let count: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM file_history_blobs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn replace_decrements_old_blob_increments_new() {
        let db = fresh();
        seed_task(&db, "task-3");

        db.fh_insert_snapshot("msg-r", "task-3", 1).unwrap();
        db.fh_register_blob("a", 1).unwrap();
        db.fh_register_blob("b", 1).unwrap();
        db.fh_upsert_file("msg-r", "f", Some("a"), None, None).unwrap();
        db.fh_upsert_file("msg-r", "f", Some("b"), None, None).unwrap();

        let a: i64 = db
            .conn()
            .query_row(
                "SELECT ref_count FROM file_history_blobs WHERE hash = 'a'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let b: i64 = db
            .conn()
            .query_row(
                "SELECT ref_count FROM file_history_blobs WHERE hash = 'b'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(a, 0, "old blob should be decremented to 0 by REPLACE");
        assert_eq!(b, 1, "new blob should be incremented to 1");
    }
}

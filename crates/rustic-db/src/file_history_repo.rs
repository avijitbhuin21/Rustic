//! Repository for the shadow-backed file-history snapshot index.
//!
//! Post-R.1 (migration 014), this layer is thin: one row per user-message
//! anchor with the libgit2 tree oid that the shadow snapshot captured at
//! `open_snapshot` time. All per-file data lives in the shadow repo — see
//! `crates/rustic-agent/src/file_history/shadow.rs`.

use rusqlite::{params, OptionalExtension};

use crate::connection::Database;
use crate::error::Result;
use crate::models::FileHistorySnapshotRow;

impl Database {
    /// Insert a new snapshot row. `sequence` is the per-task monotonically
    /// increasing turn counter. `tree_oid` is the shadow tree hash captured
    /// at the time of insert (passed through from `ShadowSnapshot::track()`).
    /// Idempotent on `message_id` collisions — the existing row wins so
    /// callers can safely retry `open_snapshot`.
    pub fn fh_insert_snapshot(
        &self,
        message_id: &str,
        task_id: &str,
        sequence: i64,
        tree_oid: &str,
    ) -> Result<()> {
        self.conn().execute(
            "INSERT OR IGNORE INTO file_history_snapshots
                 (message_id, task_id, sequence, tree_oid)
             VALUES (?1, ?2, ?3, ?4)",
            params![message_id, task_id, sequence, tree_oid],
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
            "SELECT message_id, task_id, sequence, tree_oid, created_at
             FROM file_history_snapshots WHERE message_id = ?1",
        )?;
        Ok(stmt
            .query_row(params![message_id], |row| {
                Ok(FileHistorySnapshotRow {
                    message_id: row.get(0)?,
                    task_id: row.get(1)?,
                    sequence: row.get(2)?,
                    tree_oid: row.get(3)?,
                    created_at: row.get(4)?,
                })
            })
            .optional()?)
    }

    /// Snapshots for a task in chronological (sequence ASC) order.
    pub fn fh_list_snapshots_for_task(
        &self,
        task_id: &str,
    ) -> Result<Vec<FileHistorySnapshotRow>> {
        let mut stmt = self.conn().prepare_cached(
            "SELECT message_id, task_id, sequence, tree_oid, created_at
             FROM file_history_snapshots
             WHERE task_id = ?1
             ORDER BY sequence ASC",
        )?;
        let rows = stmt.query_map(params![task_id], |row| {
            Ok(FileHistorySnapshotRow {
                message_id: row.get(0)?,
                task_id: row.get(1)?,
                sequence: row.get(2)?,
                tree_oid: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Update the recorded tree oid for an existing snapshot. Used by the
    /// sweep worker (Day 4) when re-capturing the worktree state after a
    /// bash run. No-op if the snapshot row is missing.
    pub fn fh_update_tree_oid(&self, message_id: &str, tree_oid: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE file_history_snapshots SET tree_oid = ?1 WHERE message_id = ?2",
            params![tree_oid, message_id],
        )?;
        Ok(())
    }

    /// Delete the oldest snapshots beyond `keep_last_n` for this task.
    /// Returns the count deleted. Caller is responsible for following up
    /// with `ShadowSnapshot::cleanup` so any unreferenced tree objects in
    /// the shadow's odb get pruned (we keep the metadata-vs-objects
    /// distinction explicit; reachability lives entirely in the shadow).
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

    /// Every non-null tree oid currently referenced by any snapshot row.
    /// Used by `ShadowSnapshot::cleanup` to build the keep-set before
    /// pruning loose objects.
    pub fn fh_all_tree_oids(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn().prepare_cached(
            "SELECT tree_oid FROM file_history_snapshots WHERE tree_oid IS NOT NULL",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Persist the post-turn worktree tree oid for a task. Called once per
    /// completed turn so `list_task_net_changes` can diff against the state
    /// the task actually left behind rather than live disk.
    pub fn update_task_final_tree_oid(&self, task_id: &str, tree_oid: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE tasks SET final_tree_oid = ?1 WHERE id = ?2",
            params![tree_oid, task_id],
        )?;
        Ok(())
    }

    /// Read back the stored post-turn tree oid for a task. Returns `None`
    /// for tasks that predate this feature or have never completed a turn.
    pub fn get_task_final_tree_oid(&self, task_id: &str) -> Result<Option<String>> {
        let mut stmt = self.conn().prepare_cached(
            "SELECT final_tree_oid FROM tasks WHERE id = ?1",
        )?;
        Ok(stmt
            .query_row(params![task_id], |row| row.get::<_, Option<String>>(0))
            .optional()?
            .flatten())
    }

    /// Every non-null `final_tree_oid` stored across all tasks for this DB.
    /// Included in the GC keep-set so the shadow repo doesn't prune a tree
    /// that's only referenced by a task's final-state record.
    pub fn fh_all_final_tree_oids(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn().prepare_cached(
            "SELECT final_tree_oid FROM tasks WHERE final_tree_oid IS NOT NULL",
        )?;
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
        db.conn()
            .execute(
                "INSERT INTO projects (id, name, root_path) VALUES ('p', 'p', 'p')",
                [],
            )
            .ok();
        db.conn()
            .execute(
                "INSERT INTO tasks (id, project_id, title, status, provider_type, model)
                 VALUES (?1, 'p', 't', 'created', 'native', 'm')",
                params![id],
            )
            .expect("insert task");
    }

    #[test]
    fn insert_and_get_round_trip_preserves_tree_oid() {
        let db = fresh();
        seed_task(&db, "task-1");
        db.fh_insert_snapshot("msg-1", "task-1", 1, "abcd1234").unwrap();

        let row = db.fh_get_snapshot("msg-1").unwrap().unwrap();
        assert_eq!(row.message_id, "msg-1");
        assert_eq!(row.task_id, "task-1");
        assert_eq!(row.sequence, 1);
        assert_eq!(row.tree_oid.as_deref(), Some("abcd1234"));
    }

    #[test]
    fn list_snapshots_orders_by_sequence_ascending() {
        let db = fresh();
        seed_task(&db, "task-2");
        db.fh_insert_snapshot("msg-2", "task-2", 2, "t2").unwrap();
        db.fh_insert_snapshot("msg-1", "task-2", 1, "t1").unwrap();
        db.fh_insert_snapshot("msg-3", "task-2", 3, "t3").unwrap();

        let rows = db.fh_list_snapshots_for_task("task-2").unwrap();
        let ids: Vec<_> = rows.iter().map(|r| r.message_id.as_str()).collect();
        assert_eq!(ids, vec!["msg-1", "msg-2", "msg-3"]);
    }

    #[test]
    fn update_tree_oid_overwrites_existing_value() {
        let db = fresh();
        seed_task(&db, "task-3");
        db.fh_insert_snapshot("msg-x", "task-3", 1, "initial").unwrap();
        db.fh_update_tree_oid("msg-x", "after-sweep").unwrap();
        let row = db.fh_get_snapshot("msg-x").unwrap().unwrap();
        assert_eq!(row.tree_oid.as_deref(), Some("after-sweep"));
    }

    #[test]
    fn evict_old_snapshots_keeps_last_n() {
        let db = fresh();
        seed_task(&db, "task-4");
        for i in 1..=5 {
            db.fh_insert_snapshot(&format!("m{i}"), "task-4", i as i64, "t").unwrap();
        }
        let evicted = db.fh_evict_old_snapshots("task-4", 2).unwrap();
        assert_eq!(evicted, 3);

        let remaining = db.fh_list_snapshots_for_task("task-4").unwrap();
        let ids: Vec<_> = remaining.iter().map(|r| r.message_id.as_str()).collect();
        assert_eq!(ids, vec!["m4", "m5"]);
    }

    #[test]
    fn all_tree_oids_returns_non_null_set() {
        let db = fresh();
        seed_task(&db, "task-5");
        db.fh_insert_snapshot("a", "task-5", 1, "tree-a").unwrap();
        db.fh_insert_snapshot("b", "task-5", 2, "tree-b").unwrap();
        db.conn()
            .execute(
                "UPDATE file_history_snapshots SET tree_oid = NULL WHERE message_id = 'b'",
                [],
            )
            .unwrap();

        let mut oids = db.fh_all_tree_oids().unwrap();
        oids.sort();
        assert_eq!(oids, vec!["tree-a".to_string()]);
    }

    #[test]
    fn max_sequence_for_task_returns_zero_when_empty() {
        let db = fresh();
        seed_task(&db, "task-6");
        assert_eq!(db.fh_max_sequence_for_task("task-6").unwrap(), 0);
        db.fh_insert_snapshot("m", "task-6", 7, "t").unwrap();
        assert_eq!(db.fh_max_sequence_for_task("task-6").unwrap(), 7);
    }
}

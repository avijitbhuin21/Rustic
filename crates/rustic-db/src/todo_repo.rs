//! Repository for persistent agent todos (migration 012).
//!
//! Each task has at most one current todo list (the `todo_write` tool replaces
//! it on every call), so we store the entire list as a JSON array keyed by
//! task_id and upsert in place.

use rusqlite::{params, OptionalExtension};

use crate::connection::Database;
use crate::error::Result;

impl Database {
    /// Replace the todo list for a task. `todos_json` is a JSON-encoded array
    /// of `{ content, status }` objects — the same shape the frontend reads.
    pub fn set_task_todos(&self, task_id: &str, todos_json: &str) -> Result<()> {
        self.conn().execute(
            "INSERT INTO task_todos (task_id, todos_json, updated_at)
             VALUES (?1, ?2, datetime('now'))
             ON CONFLICT(task_id) DO UPDATE SET
                 todos_json = excluded.todos_json,
                 updated_at = excluded.updated_at",
            params![task_id, todos_json],
        )?;
        Ok(())
    }

    /// Read the persisted todo list for a task. Returns `None` when the task
    /// has never written todos.
    pub fn get_task_todos(&self, task_id: &str) -> Result<Option<String>> {
        let mut stmt = self
            .conn()
            .prepare_cached("SELECT todos_json FROM task_todos WHERE task_id = ?1")?;
        Ok(stmt
            .query_row(params![task_id], |row| row.get::<_, String>(0))
            .optional()?)
    }

    /// Snapshot the todo list as it stood AT THE START of the turn anchored at
    /// `message_id`. Pairs with `file_history.open_snapshot` — both are called
    /// once per user turn so a later revert restores the worktree and the
    /// todo list to the same point in time.
    ///
    /// `INSERT OR IGNORE` so a re-entered turn (resume after stop, etc.)
    /// doesn't overwrite the original pre-turn state with the now-mutated list.
    pub fn snapshot_todos_at_message(
        &self,
        task_id: &str,
        message_id: &str,
        todos_json: &str,
    ) -> Result<()> {
        self.conn().execute(
            "INSERT OR IGNORE INTO task_todo_snapshots (task_id, message_id, todos_json)
             VALUES (?1, ?2, ?3)",
            params![task_id, message_id, todos_json],
        )?;
        Ok(())
    }

    /// Read the todo snapshot recorded at the start of the turn anchored at
    /// `message_id`. Returns `(task_id, todos_json)` so the caller can restore
    /// without a second lookup. `None` when the turn predates this feature or
    /// when revert is targeting a message that never opened a snapshot.
    pub fn get_todo_snapshot(&self, message_id: &str) -> Result<Option<(String, String)>> {
        let mut stmt = self.conn().prepare_cached(
            "SELECT task_id, todos_json FROM task_todo_snapshots WHERE message_id = ?1",
        )?;
        Ok(stmt
            .query_row(params![message_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .optional()?)
    }

    /// Read the earliest todo snapshot for a task — i.e., the pre-task state
    /// captured at the very first turn. Used by `revert_task` to restore the
    /// list to the way it looked before the agent started any work in this
    /// task. `None` when the task has no snapshots at all.
    pub fn get_first_todo_snapshot_for_task(
        &self,
        task_id: &str,
    ) -> Result<Option<String>> {
        let mut stmt = self.conn().prepare_cached(
            "SELECT todos_json FROM task_todo_snapshots
             WHERE task_id = ?1
             ORDER BY created_at ASC
             LIMIT 1",
        )?;
        Ok(stmt
            .query_row(params![task_id], |row| row.get::<_, String>(0))
            .optional()?)
    }
}

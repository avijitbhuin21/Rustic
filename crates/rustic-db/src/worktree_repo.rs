//! Repository for per-task isolated worktrees + the derived merge queue.
//!
//! States: `active` -> `review` -> `queued` -> `merging` -> `merged`,
//! with `needs-reconciliation` (parked on conflict/validation failure,
//! re-enqueueable) and `discarded` as terminal-ish side exits. The FIFO
//! merge queue is `state='queued' ORDER BY queued_at` -- no separate table.

use rusqlite::{params, OptionalExtension};

use crate::connection::Database;
use crate::error::Result;
use crate::models::TaskWorktreeRow;

fn row_to_worktree(row: &rusqlite::Row) -> rusqlite::Result<TaskWorktreeRow> {
    Ok(TaskWorktreeRow {
        task_id: row.get(0)?,
        project_id: row.get(1)?,
        project_root: row.get(2)?,
        worktree_path: row.get(3)?,
        branch: row.get(4)?,
        base_branch: row.get(5)?,
        base_oid: row.get(6)?,
        state: row.get(7)?,
        queued_at: row.get(8)?,
        merged_oid: row.get(9)?,
        last_error: row.get(10)?,
        created_at: row.get(11)?,
    })
}

const COLS: &str = "task_id, project_id, project_root, worktree_path, branch, base_branch, base_oid, state, queued_at, merged_oid, last_error, created_at";

impl Database {
    /// Insert the row for a freshly created task worktree (state `active`).
    pub fn wt_insert(
        &self,
        task_id: &str,
        project_id: &str,
        project_root: &str,
        worktree_path: &str,
        branch: &str,
        base_branch: &str,
        base_oid: &str,
    ) -> Result<()> {
        self.conn().execute(
            "INSERT OR REPLACE INTO task_worktrees\n                 (task_id, project_id, project_root, worktree_path, branch, base_branch, base_oid, state)\n             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'active')",
            params![task_id, project_id, project_root, worktree_path, branch, base_branch, base_oid],
        )?;
        Ok(())
    }

    /// Fetch the worktree row for a task, if any.
    pub fn wt_get(&self, task_id: &str) -> Result<Option<TaskWorktreeRow>> {
        let mut stmt = self.conn().prepare_cached(&format!(
            "SELECT {COLS} FROM task_worktrees WHERE task_id = ?1"
        ))?;
        Ok(stmt
            .query_row(params![task_id], |r| row_to_worktree(r))
            .optional()?)
    }

    /// Set the state for a task worktree; clears `last_error` on non-error states.
    pub fn wt_set_state(&self, task_id: &str, state: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE task_worktrees SET state = ?2,\n                    last_error = CASE WHEN ?2 = 'needs-reconciliation' THEN last_error ELSE NULL END\n             WHERE task_id = ?1",
            params![task_id, state],
        )?;
        Ok(())
    }

    /// Overwrite the stored branch name for a task worktree (used by the
    /// branch-to-detached migration; empty string = detached, no branch).
    pub fn wt_set_branch(&self, task_id: &str, branch: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE task_worktrees SET branch = ?2 WHERE task_id = ?1",
            params![task_id, branch],
        )?;
        Ok(())
    }

    /// Park a worktree as needs-reconciliation with the failure reason.
    pub fn wt_park(&self, task_id: &str, error: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE task_worktrees SET state = 'needs-reconciliation', last_error = ?2 WHERE task_id = ?1",
            params![task_id, error],
        )?;
        Ok(())
    }

    /// Enqueue a worktree for merging (FIFO by queued_at, tie-broken by task_id).
    /// Re-enqueues out of a park or an interrupted merge KEEP their original
    /// queued_at so the head of the queue can't be overtaken while its
    /// conflict is being resolved.
    pub fn wt_enqueue(&self, task_id: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE task_worktrees\n                SET queued_at = CASE\n                        WHEN state IN ('needs-reconciliation','merging') AND queued_at IS NOT NULL\n                        THEN queued_at\n                        ELSE strftime('%Y-%m-%dT%H:%M:%fZ','now') END,\n                    state = 'queued', last_error = NULL\n             WHERE task_id = ?1",
            params![task_id],
        )?;
        Ok(())
    }

    /// Mark a worktree merged, recording the commit that landed on main.
    /// Auto-merge keep-alive: the task keeps working in the same worktree, so
    /// the row goes BACK to `active` with the landed commit as the new fork
    /// point (the squash left the branch exactly at that commit).
    pub fn wt_record_merge(&self, task_id: &str, merged_oid: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE task_worktrees\n                SET state = 'active', merged_oid = ?2, base_oid = ?2,\n                    queued_at = NULL, last_error = NULL\n             WHERE task_id = ?1",
            params![task_id, merged_oid],
        )?;
        Ok(())
    }

    /// Nothing-to-merge fast path: re-sync the fork point to the current base
    /// tip and go back to `active` without touching `merged_oid`.
    pub fn wt_reset_active(&self, task_id: &str, base_oid: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE task_worktrees\n                SET state = 'active', base_oid = ?2, queued_at = NULL, last_error = NULL\n             WHERE task_id = ?1",
            params![task_id, base_oid],
        )?;
        Ok(())
    }

    /// Mark a worktree merged, recording the commit landed on main.
    pub fn wt_set_merged(&self, task_id: &str, merged_oid: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE task_worktrees SET state = 'merged', merged_oid = ?2, last_error = NULL WHERE task_id = ?1",
            params![task_id, merged_oid],
        )?;
        Ok(())
    }

    /// Next queued item for a repo root (FIFO), if any.
    pub fn wt_next_queued(&self, project_root: &str) -> Result<Option<TaskWorktreeRow>> {
        let mut stmt = self.conn().prepare_cached(&format!(
            "SELECT {COLS} FROM task_worktrees\n             WHERE project_root = ?1 AND state = 'queued'\n             ORDER BY queued_at ASC, task_id ASC LIMIT 1"
        ))?;
        Ok(stmt
            .query_row(params![project_root], |r| row_to_worktree(r))
            .optional()?)
    }

    /// Whether any worktree for this repo root is parked on a conflict.
    /// The merge worker halts while one exists (strict FIFO: nothing lands
    /// past an unresolved head).
    pub fn wt_has_parked(&self, project_root: &str) -> Result<bool> {
        let mut stmt = self.conn().prepare_cached(
            "SELECT 1 FROM task_worktrees\n             WHERE project_root = ?1 AND state = 'needs-reconciliation' LIMIT 1",
        )?;
        Ok(stmt
            .query_row(params![project_root], |_| Ok(()))
            .optional()?
            .is_some())
    }

    /// All worktree rows for a project (any state), newest first.
    pub fn wt_list_for_project(&self, project_id: &str) -> Result<Vec<TaskWorktreeRow>> {
        let mut stmt = self.conn().prepare_cached(&format!(
            "SELECT {COLS} FROM task_worktrees WHERE project_id = ?1 ORDER BY created_at DESC"
        ))?;
        let rows = stmt.query_map(params![project_id], |r| row_to_worktree(r))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Every row in the table -- used by the startup orphan sweep.
    pub fn wt_list_all(&self) -> Result<Vec<TaskWorktreeRow>> {
        let mut stmt = self
            .conn()
            .prepare_cached(&format!("SELECT {COLS} FROM task_worktrees"))?;
        let rows = stmt.query_map([], |r| row_to_worktree(r))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Startup recovery: any `merging` row was interrupted mid-merge -- reset
    /// to `queued` (the worker is idempotent up to the ff-push).
    pub fn wt_reset_interrupted(&self) -> Result<usize> {
        let n = self.conn().execute(
            "UPDATE task_worktrees SET state = 'queued' WHERE state = 'merging'",
            [],
        )?;
        Ok(n)
    }

    /// Atomically claim a queued item for merging. Returns false when the row
    /// is no longer queued (e.g. the user reactivated the task) -- skip it.
    pub fn wt_try_start_merging(&self, task_id: &str) -> Result<bool> {
        let n = self.conn().execute(
            "UPDATE task_worktrees SET state = 'merging' WHERE task_id = ?1 AND state = 'queued'",
            params![task_id],
        )?;
        Ok(n > 0)
    }

    /// New user turn on an isolated task: pull it back out of review/queue.
    pub fn wt_reactivate(&self, task_id: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE task_worktrees SET state = 'active', queued_at = NULL
             WHERE task_id = ?1 AND state IN ('queued', 'review')",
            params![task_id],
        )?;
        Ok(())
    }

    /// Turn finished: an `active` worktree becomes reviewable.
    pub fn wt_mark_review(&self, task_id: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE task_worktrees SET state = 'review' WHERE task_id = ?1 AND state = 'active'",
            params![task_id],
        )?;
        Ok(())
    }

    /// Remove the row entirely (after discard cleanup or orphan pruning).
    pub fn wt_delete(&self, task_id: &str) -> Result<()> {
        self.conn().execute(
            "DELETE FROM task_worktrees WHERE task_id = ?1",
            params![task_id],
        )?;
        Ok(())
    }
}

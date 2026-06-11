//! GitHub auto-issue-resolve storage (see migration 017). Server-only feature;
//! these tables are simply unused on desktop.

use crate::error::Result;
use rusqlite::params;

use crate::connection::Database;
use crate::models::{GithubEventRow, GithubIssueRow};

const ISSUE_COLUMNS: &str =
    "id, project_id, repo_full_name, issue_number, title, issue_url, task_id, status, \
     pending_tool_use_id, pending_questions_json, cost_cap_usd, error, created_at, updated_at";

fn row_to_issue(row: &rusqlite::Row) -> rusqlite::Result<GithubIssueRow> {
    Ok(GithubIssueRow {
        id: row.get(0)?,
        project_id: row.get(1)?,
        repo_full_name: row.get(2)?,
        issue_number: row.get(3)?,
        title: row.get(4)?,
        issue_url: row.get(5)?,
        task_id: row.get(6)?,
        status: row.get(7)?,
        pending_tool_use_id: row.get(8)?,
        pending_questions_json: row.get(9)?,
        cost_cap_usd: row.get(10)?,
        error: row.get(11)?,
        created_at: row.get(12)?,
        updated_at: row.get(13)?,
    })
}

impl Database {
    /// Insert the issue if unseen, otherwise refresh its title/url. Returns
    /// the row id either way. Does NOT touch `status` on an existing row —
    /// lifecycle transitions are explicit.
    pub fn upsert_github_issue(
        &self,
        project_id: &str,
        repo_full_name: &str,
        issue_number: i64,
        title: &str,
        issue_url: &str,
        cost_cap_usd: Option<f64>,
    ) -> Result<i64> {
        self.conn().execute(
            "INSERT INTO github_issues (project_id, repo_full_name, issue_number, title, issue_url, cost_cap_usd)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(repo_full_name, issue_number) DO UPDATE SET
               title = excluded.title,
               issue_url = excluded.issue_url,
               updated_at = datetime('now')",
            params![project_id, repo_full_name, issue_number, title, issue_url, cost_cap_usd],
        )?;
        let id: i64 = self.conn().query_row(
            "SELECT id FROM github_issues WHERE repo_full_name = ?1 AND issue_number = ?2",
            params![repo_full_name, issue_number],
            |row| row.get(0),
        )?;
        Ok(id)
    }

    pub fn get_github_issue(&self, id: i64) -> Result<Option<GithubIssueRow>> {
        let mut stmt = self
            .conn()
            .prepare_cached(&format!("SELECT {ISSUE_COLUMNS} FROM github_issues WHERE id = ?1"))?;
        let mut rows = stmt.query_map(params![id], row_to_issue)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    pub fn get_github_issue_by_number(
        &self,
        repo_full_name: &str,
        issue_number: i64,
    ) -> Result<Option<GithubIssueRow>> {
        let mut stmt = self.conn().prepare_cached(&format!(
            "SELECT {ISSUE_COLUMNS} FROM github_issues WHERE repo_full_name = ?1 AND issue_number = ?2"
        ))?;
        let mut rows = stmt.query_map(params![repo_full_name, issue_number], row_to_issue)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    pub fn get_github_issue_by_task(&self, task_id: &str) -> Result<Option<GithubIssueRow>> {
        let mut stmt = self.conn().prepare_cached(&format!(
            "SELECT {ISSUE_COLUMNS} FROM github_issues WHERE task_id = ?1"
        ))?;
        let mut rows = stmt.query_map(params![task_id], row_to_issue)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    /// Issues for the queue panel, newest activity first. `project_id = None`
    /// lists across all projects.
    pub fn list_github_issues(&self, project_id: Option<&str>) -> Result<Vec<GithubIssueRow>> {
        match project_id {
            Some(pid) => {
                let mut stmt = self.conn().prepare_cached(&format!(
                    "SELECT {ISSUE_COLUMNS} FROM github_issues WHERE project_id = ?1 ORDER BY updated_at DESC"
                ))?;
                let rows = stmt.query_map(params![pid], row_to_issue)?;
                Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
            }
            None => {
                let mut stmt = self.conn().prepare_cached(&format!(
                    "SELECT {ISSUE_COLUMNS} FROM github_issues ORDER BY updated_at DESC"
                ))?;
                let rows = stmt.query_map([], row_to_issue)?;
                Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
            }
        }
    }

    pub fn set_github_issue_status(&self, id: i64, status: &str, error: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE github_issues SET status = ?1, error = ?2, updated_at = datetime('now') WHERE id = ?3",
            params![status, error, id],
        )?;
        Ok(())
    }

    pub fn bind_github_issue_task(&self, id: i64, task_id: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE github_issues SET task_id = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![task_id, id],
        )?;
        Ok(())
    }

    /// Record an ask_user suspension: the dangling tool_use id plus the
    /// questions the agent asked, and flip the row to `waiting_reply`.
    pub fn set_github_issue_pending_ask(
        &self,
        id: i64,
        tool_use_id: &str,
        questions_json: &str,
    ) -> Result<()> {
        self.conn().execute(
            "UPDATE github_issues SET
               pending_tool_use_id = ?1,
               pending_questions_json = ?2,
               status = 'waiting_reply',
               updated_at = datetime('now')
             WHERE id = ?3",
            params![tool_use_id, questions_json, id],
        )?;
        Ok(())
    }

    pub fn clear_github_issue_pending_ask(&self, id: i64, new_status: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE github_issues SET
               pending_tool_use_id = NULL,
               pending_questions_json = NULL,
               status = ?1,
               updated_at = datetime('now')
             WHERE id = ?2",
            params![new_status, id],
        )?;
        Ok(())
    }

    /// Crash recovery: rows stuck in `working` from a previous process get
    /// re-queued at boot so the worker picks them up again.
    pub fn requeue_working_github_issues(&self) -> Result<()> {
        self.conn().execute(
            "UPDATE github_issues SET status = 'queued', updated_at = datetime('now') WHERE status = 'working'",
            [],
        )?;
        Ok(())
    }

    // ── event queue ──────────────────────────────────────────────────────

    pub fn enqueue_github_event(&self, issue_id: i64, kind: &str, payload_json: &str) -> Result<i64> {
        self.conn().execute(
            "INSERT INTO github_events (issue_id, kind, payload_json) VALUES (?1, ?2, ?3)",
            params![issue_id, kind, payload_json],
        )?;
        Ok(self.conn().last_insert_rowid())
    }

    /// Pop candidate: the oldest event that is currently runnable. An issue
    /// in `waiting_reply` only accepts `comment` events (the reply that
    /// resumes it); `new_issue` / `issue_update` events for it stay parked
    /// until the suspension resolves. The single-threaded worker deletes the
    /// event once processed, so no claim/lock column is needed.
    pub fn next_github_event(&self) -> Result<Option<GithubEventRow>> {
        let mut stmt = self.conn().prepare_cached(
            "SELECT e.id, e.issue_id, e.kind, e.payload_json, e.created_at
             FROM github_events e
             JOIN github_issues i ON i.id = e.issue_id
             WHERE i.status != 'waiting_reply' OR e.kind = 'comment'
             ORDER BY e.id ASC
             LIMIT 1",
        )?;
        let mut rows = stmt.query_map([], |row| {
            Ok(GithubEventRow {
                id: row.get(0)?,
                issue_id: row.get(1)?,
                kind: row.get(2)?,
                payload_json: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    pub fn delete_github_event(&self, id: i64) -> Result<()> {
        self.conn()
            .execute("DELETE FROM github_events WHERE id = ?1", params![id])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::connection::Database;

    #[test]
    fn issue_upsert_and_lifecycle() {
        let db = Database::in_memory().expect("init");
        let id = db
            .upsert_github_issue("proj-1", "me/repo", 42, "Crash on save", "https://x/42", Some(2.5))
            .expect("insert");
        // Upsert again refreshes title, keeps id + status.
        let id2 = db
            .upsert_github_issue("proj-1", "me/repo", 42, "Crash on save (edited)", "https://x/42", None)
            .expect("upsert");
        assert_eq!(id, id2);
        let row = db.get_github_issue(id).expect("get").expect("row");
        assert_eq!(row.title, "Crash on save (edited)");
        assert_eq!(row.status, "queued");
        assert_eq!(row.cost_cap_usd, Some(2.5));

        db.bind_github_issue_task(id, "task-1").expect("bind");
        db.set_github_issue_pending_ask(id, "toolu_1", "[]").expect("pending");
        let row = db.get_github_issue_by_task("task-1").expect("get").expect("row");
        assert_eq!(row.status, "waiting_reply");
        assert_eq!(row.pending_tool_use_id.as_deref(), Some("toolu_1"));

        db.clear_github_issue_pending_ask(id, "working").expect("clear");
        let row = db.get_github_issue(id).expect("get").expect("row");
        assert_eq!(row.status, "working");
        assert!(row.pending_tool_use_id.is_none());
    }

    #[test]
    fn event_queue_respects_waiting_reply() {
        let db = Database::in_memory().expect("init");
        let a = db
            .upsert_github_issue("p", "me/repo", 1, "a", "", None)
            .expect("a");
        let b = db
            .upsert_github_issue("p", "me/repo", 2, "b", "", None)
            .expect("b");

        let e1 = db.enqueue_github_event(a, "new_issue", "{}").expect("e1");
        let _e2 = db.enqueue_github_event(b, "new_issue", "{}").expect("e2");

        // Plain FIFO first.
        let next = db.next_github_event().expect("next").expect("some");
        assert_eq!(next.id, e1);

        // Issue A suspends — its non-comment events park; B's event surfaces.
        db.set_github_issue_pending_ask(a, "toolu_1", "[]").expect("pend");
        let next = db.next_github_event().expect("next").expect("some");
        assert_eq!(next.issue_id, b);

        // A comment for the waiting issue IS runnable; A's parked new_issue
        // event (older) still must not surface.
        db.delete_github_event(next.id).expect("del");
        let c = db.enqueue_github_event(a, "comment", "{\"body\":\"answer\"}").expect("c");
        let next = db.next_github_event().expect("next").expect("some");
        assert_eq!(next.id, c);
        assert_ne!(next.id, e1);
    }
}

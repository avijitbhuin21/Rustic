//! Repository for external CLI agent sessions (migration 028).
//!
//! Rows are keyed by our own `id`, but almost every write arrives from a hook
//! callback that only knows the `pty_key` we injected into the PTY environment,
//! so most methods look up by that instead.

use rusqlite::{params, OptionalExtension, Row};

use crate::connection::Database;
use crate::error::Result;
use crate::models::ExternalAgentSessionRow;

fn map_row(row: &Row) -> rusqlite::Result<ExternalAgentSessionRow> {
    Ok(ExternalAgentSessionRow {
        id: row.get(0)?,
        project_id: row.get(1)?,
        agent: row.get(2)?,
        pty_key: row.get(3)?,
        external_session_id: row.get(4)?,
        title: row.get(5)?,
        transcript_path: row.get(6)?,
        cwd: row.get(7)?,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
        last_active_at: row.get(10)?,
    })
}

const SELECT_COLS: &str = "id, project_id, agent, pty_key, external_session_id, title, \
                           transcript_path, cwd, created_at, updated_at, last_active_at";

impl Database {
    /// Register a freshly spawned session. `external_session_id` is unknown at
    /// this point and filled in later by `attach_external_agent_session_id`.
    pub fn create_external_agent_session(
        &self,
        id: &str,
        project_id: &str,
        agent: &str,
        pty_key: &str,
        cwd: &str,
    ) -> Result<()> {
        self.conn().execute(
            "INSERT INTO external_agent_sessions
                 (id, project_id, agent, pty_key, cwd, last_active_at)
             VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'))",
            params![id, project_id, agent, pty_key, cwd],
        )?;
        Ok(())
    }

    /// Point an existing conversation at a new PTY spawn (the resume path), so
    /// subsequent hook callbacks carrying the new token land on this row.
    pub fn rebind_external_agent_session(&self, id: &str, pty_key: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE external_agent_sessions
                SET pty_key = ?2, updated_at = datetime('now'), last_active_at = datetime('now')
              WHERE id = ?1",
            params![id, pty_key],
        )?;
        Ok(())
    }

    /// Record the CLI's own conversation id against the spawn identified by
    /// `pty_key`.
    ///
    /// The id may already belong to a different row — the user can resume an
    /// older conversation from inside the CLI's own picker, which lands a
    /// pre-existing id on what we thought was a brand-new session. In that case
    /// the two rows describe one conversation, so the older row wins and the
    /// placeholder row is dropped after donating its `pty_key`. Returns the id
    /// of the surviving row.
    pub fn attach_external_agent_session_id(
        &self,
        pty_key: &str,
        external_session_id: &str,
        transcript_path: Option<&str>,
    ) -> Result<Option<String>> {
        let tx = self.conn().unchecked_transaction()?;

        let pending: Option<(String, String)> = tx
            .query_row(
                "SELECT id, agent FROM external_agent_sessions WHERE pty_key = ?1",
                params![pty_key],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;

        let Some((pending_id, agent)) = pending else {
            return Ok(None);
        };

        let existing_id: Option<String> = tx
            .query_row(
                "SELECT id FROM external_agent_sessions
                  WHERE agent = ?1 AND external_session_id = ?2",
                params![agent, external_session_id],
                |row| row.get(0),
            )
            .optional()?;

        let survivor = match existing_id {
            Some(existing_id) if existing_id != pending_id => {
                tx.execute(
                    "DELETE FROM external_agent_sessions WHERE id = ?1",
                    params![pending_id],
                )?;
                tx.execute(
                    "UPDATE external_agent_sessions
                        SET pty_key = ?2,
                            transcript_path = COALESCE(?3, transcript_path),
                            updated_at = datetime('now'),
                            last_active_at = datetime('now')
                      WHERE id = ?1",
                    params![existing_id, pty_key, transcript_path],
                )?;
                existing_id
            }
            _ => {
                tx.execute(
                    "UPDATE external_agent_sessions
                        SET external_session_id = ?2,
                            transcript_path = COALESCE(?3, transcript_path),
                            updated_at = datetime('now'),
                            last_active_at = datetime('now')
                      WHERE id = ?1",
                    params![pending_id, external_session_id, transcript_path],
                )?;
                pending_id
            }
        };

        tx.commit()?;
        Ok(Some(survivor))
    }

    /// Set the display title from the first user prompt. Later prompts must not
    /// overwrite it, so this is a no-op once a title exists.
    pub fn set_external_agent_session_title(&self, pty_key: &str, title: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE external_agent_sessions
                SET title = ?2, updated_at = datetime('now'), last_active_at = datetime('now')
              WHERE pty_key = ?1 AND (title IS NULL OR title = '')",
            params![pty_key, title],
        )?;
        Ok(())
    }

    pub fn touch_external_agent_session(&self, pty_key: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE external_agent_sessions
                SET last_active_at = datetime('now') WHERE pty_key = ?1",
            params![pty_key],
        )?;
        Ok(())
    }

    /// Most recently active sessions for a project, newest first. `agent`
    /// filters to a single CLI when supplied.
    pub fn list_external_agent_sessions(
        &self,
        project_id: &str,
        agent: Option<&str>,
        limit: i64,
    ) -> Result<Vec<ExternalAgentSessionRow>> {
        let sql = format!(
            "SELECT {SELECT_COLS} FROM external_agent_sessions
              WHERE project_id = ?1 AND (?2 IS NULL OR agent = ?2)
              ORDER BY COALESCE(last_active_at, updated_at) DESC, created_at DESC
              LIMIT ?3"
        );
        let mut stmt = self.conn().prepare_cached(&sql)?;
        let rows = stmt
            .query_map(params![project_id, agent, limit], map_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn get_external_agent_session(&self, id: &str) -> Result<Option<ExternalAgentSessionRow>> {
        let sql = format!("SELECT {SELECT_COLS} FROM external_agent_sessions WHERE id = ?1");
        let mut stmt = self.conn().prepare_cached(&sql)?;
        Ok(stmt.query_row(params![id], map_row).optional()?)
    }

    pub fn get_external_agent_session_by_pty_key(
        &self,
        pty_key: &str,
    ) -> Result<Option<ExternalAgentSessionRow>> {
        let sql = format!("SELECT {SELECT_COLS} FROM external_agent_sessions WHERE pty_key = ?1");
        let mut stmt = self.conn().prepare_cached(&sql)?;
        Ok(stmt.query_row(params![pty_key], map_row).optional()?)
    }

    pub fn delete_external_agent_session(&self, id: &str) -> Result<()> {
        self.conn().execute(
            "DELETE FROM external_agent_sessions WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db_with_project() -> Database {
        let db = Database::in_memory().expect("db");
        db.conn()
            .execute(
                "INSERT INTO projects (id, name, root_path) VALUES ('p1', 'Proj', '/tmp/p1')",
                [],
            )
            .expect("project");
        db
    }

    #[test]
    fn attach_fills_in_session_id_and_title() {
        let db = db_with_project();
        db.create_external_agent_session("s1", "p1", "claude", "key-1", "/tmp/p1")
            .unwrap();

        let survivor = db
            .attach_external_agent_session_id("key-1", "ext-abc", Some("/t.jsonl"))
            .unwrap();
        assert_eq!(survivor.as_deref(), Some("s1"));

        db.set_external_agent_session_title("key-1", "first prompt")
            .unwrap();
        db.set_external_agent_session_title("key-1", "second prompt")
            .unwrap();

        let row = db.get_external_agent_session("s1").unwrap().unwrap();
        assert_eq!(row.external_session_id.as_deref(), Some("ext-abc"));
        assert_eq!(row.transcript_path.as_deref(), Some("/t.jsonl"));
        assert_eq!(row.title.as_deref(), Some("first prompt"));
    }

    #[test]
    fn attach_merges_when_id_already_belongs_to_another_row() {
        let db = db_with_project();
        db.create_external_agent_session("old", "p1", "claude", "key-old", "/tmp/p1")
            .unwrap();
        db.attach_external_agent_session_id("key-old", "ext-abc", None)
            .unwrap();
        db.set_external_agent_session_title("key-old", "original title")
            .unwrap();

        // A "new" session that turns out to be the same conversation.
        db.create_external_agent_session("new", "p1", "claude", "key-new", "/tmp/p1")
            .unwrap();
        let survivor = db
            .attach_external_agent_session_id("key-new", "ext-abc", None)
            .unwrap();

        assert_eq!(survivor.as_deref(), Some("old"));
        assert!(db.get_external_agent_session("new").unwrap().is_none());
        let row = db.get_external_agent_session("old").unwrap().unwrap();
        assert_eq!(row.pty_key, "key-new");
        assert_eq!(row.title.as_deref(), Some("original title"));
    }

    #[test]
    fn list_filters_by_agent_and_unknown_pty_key_is_none() {
        let db = db_with_project();
        db.create_external_agent_session("s1", "p1", "claude", "k1", "/tmp/p1")
            .unwrap();
        db.create_external_agent_session("s2", "p1", "codex", "k2", "/tmp/p1")
            .unwrap();

        assert_eq!(
            db.list_external_agent_sessions("p1", None, 50)
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            db.list_external_agent_sessions("p1", Some("codex"), 50)
                .unwrap()
                .len(),
            1
        );
        assert!(db
            .attach_external_agent_session_id("nope", "x", None)
            .unwrap()
            .is_none());
    }
}

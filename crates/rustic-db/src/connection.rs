use crate::error::{DbError, Result};
use rusqlite::Connection;
use std::path::{Path, PathBuf};

const MIGRATIONS: &[(&str, &str)] = &[
    ("001_initial", include_str!("migrations/001_initial.sql")),
    (
        "002_agent_tasks",
        include_str!("migrations/002_agent_tasks.sql"),
    ),
    (
        "004_task_cost",
        include_str!("migrations/004_task_cost.sql"),
    ),
    (
        "005_drop_mcp_servers",
        include_str!("migrations/005_drop_mcp_servers.sql"),
    ),
    (
        "006_subagents",
        include_str!("migrations/006_subagents.sql"),
    ),
    (
        "007_message_turn_usage",
        include_str!("migrations/007_message_turn_usage.sql"),
    ),
    (
        "008_harness_session_id",
        include_str!("migrations/008_harness_session_id.sql"),
    ),
    (
        "009_subagent_activity",
        include_str!("migrations/009_subagent_activity.sql"),
    ),
    (
        "010_file_history",
        include_str!("migrations/010_file_history.sql"),
    ),
    (
        "011_file_history_stat_cache",
        include_str!("migrations/011_file_history_stat_cache.sql"),
    ),
    (
        "012_task_todos",
        include_str!("migrations/012_task_todos.sql"),
    ),
    (
        "013_task_todo_snapshots",
        include_str!("migrations/013_task_todo_snapshots.sql"),
    ),
    (
        "014_file_history_shadow",
        include_str!("migrations/014_file_history_shadow.sql"),
    ),
    (
        "015_task_final_tree_oid",
        include_str!("migrations/015_task_final_tree_oid.sql"),
    ),
    (
        "016_project_archived",
        include_str!("migrations/016_project_archived.sql"),
    ),
    (
        "017_github_issues",
        include_str!("migrations/017_github_issues.sql"),
    ),
    (
        "018_task_cost_json",
        include_str!("migrations/018_task_cost_json.sql"),
    ),
    (
        "019_message_archive",
        include_str!("migrations/019_message_archive.sql"),
    ),
    (
        "020_file_history_task_writes",
        include_str!("migrations/020_file_history_task_writes.sql"),
    ),
    (
        "021_task_worktrees",
        include_str!("migrations/021_task_worktrees.sql"),
    ),
    (
        "022_task_thinking_tier",
        include_str!("migrations/022_task_thinking_tier.sql"),
    ),
    (
        "023_task_pinned",
        include_str!("migrations/023_task_pinned.sql"),
    ),
    (
        "024_task_goal",
        include_str!("migrations/024_task_goal.sql"),
    ),
    (
        "025_subagent_name",
        include_str!("migrations/025_subagent_name.sql"),
    ),
    (
        "026_drop_task_worktrees",
        include_str!("migrations/026_drop_task_worktrees.sql"),
    ),
];

pub struct Database {
    conn: Connection,
    path: PathBuf,
}

impl Database {
    pub fn new(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(path)?;

        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        conn.execute_batch("PRAGMA synchronous=NORMAL;")?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;
        // Wait up to 5s for a competing writer (e.g. a second Rustic process
        // pointed at the same data dir) instead of failing immediately with
        // SQLITE_BUSY.
        conn.execute_batch("PRAGMA busy_timeout=5000;")?;
        conn.set_prepared_statement_cache_capacity(64);

        let mut db = Self {
            conn,
            path: path.to_path_buf(),
        };

        db.run_migrations()?;

        Ok(db)
    }

    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;

        let mut db = Self {
            conn,
            path: PathBuf::from(":memory:"),
        };

        db.run_migrations()?;

        Ok(db)
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Truncate the WAL file. Call before app shutdown so the -wal sidecar doesn't grow unbounded.
    pub fn checkpoint_truncate(&self) -> Result<()> {
        self.conn
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(())
    }

    fn run_migrations(&mut self) -> Result<()> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS _migrations (
                name TEXT PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )?;

        let pending: Vec<(&str, &str)> = MIGRATIONS
            .iter()
            .copied()
            .filter(|(name, _)| {
                self.conn
                    .query_row(
                        "SELECT COUNT(*) > 0 FROM _migrations WHERE name = ?1",
                        [name],
                        |row| row.get::<_, bool>(0),
                    )
                    .unwrap_or(false)
                    == false
            })
            .collect();

        if pending.is_empty() {
            return Ok(());
        }

        if self.path != PathBuf::from(":memory:") && self.path.exists() {
            let ts = chrono::Utc::now().format("%Y%m%dT%H%M%S");
            let mut backup = self.path.clone();
            let file_name = backup
                .file_name()
                .map(|s| s.to_os_string())
                .unwrap_or_else(|| std::ffi::OsString::from("rustic.db"));
            let backup_name = format!("{}.bak.{}", file_name.to_string_lossy(), ts);
            backup.set_file_name(backup_name);
            if let Err(e) = std::fs::copy(&self.path, &backup) {
                tracing::warn!(
                    src = %self.path.display(),
                    dst = %backup.display(),
                    "migrations: pre-migration backup failed: {}",
                    e
                );
            }
        }

        for (name, sql) in pending {
            let tx = self.conn.transaction().map_err(|e| DbError::Migration {
                name: name.to_string(),
                source: e,
            })?;
            tx.execute_batch(sql).map_err(|e| DbError::Migration {
                name: name.to_string(),
                source: e,
            })?;
            tx.execute("INSERT INTO _migrations (name) VALUES (?1)", [name])
                .map_err(|e| DbError::Migration {
                    name: name.to_string(),
                    source: e,
                })?;
            tx.commit().map_err(|e| DbError::Migration {
                name: name.to_string(),
                source: e,
            })?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_replay_idempotent() {
        let mut db = Database::in_memory().expect("first init");
        db.run_migrations().expect("second run is idempotent");
        let count: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM _migrations", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count as usize, MIGRATIONS.len());
    }

    #[test]
    fn migrations_apply_all() {
        let db = Database::in_memory().expect("init");
        for (name, _) in MIGRATIONS {
            let exists: bool = db
                .conn
                .query_row(
                    "SELECT COUNT(*) > 0 FROM _migrations WHERE name = ?1",
                    [name],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(exists, "migration {} did not record", name);
        }
    }
}

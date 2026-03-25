use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::{Path, PathBuf};

const MIGRATIONS: &[(&str, &str)] = &[
    ("001_initial", include_str!("migrations/001_initial.sql")),
    ("002_agent_tasks", include_str!("migrations/002_agent_tasks.sql")),
    ("003_checkpoints", include_str!("migrations/003_checkpoints.sql")),
];

pub struct Database {
    conn: Connection,
    path: PathBuf,
}

impl Database {
    pub fn new(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create database directory: {}", parent.display()))?;
        }

        let conn = Connection::open(path)
            .with_context(|| format!("Failed to open database: {}", path.display()))?;

        // Enable WAL mode for better concurrent read performance
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        // Enable foreign key constraints
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;

        let mut db = Self {
            conn,
            path: path.to_path_buf(),
        };

        db.run_migrations()?;

        Ok(db)
    }

    /// Open an in-memory database (useful for testing)
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

    fn run_migrations(&mut self) -> Result<()> {
        // Create migrations tracking table
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS _migrations (
                name TEXT PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            );"
        )?;

        for (name, sql) in MIGRATIONS {
            let already_applied: bool = self.conn.query_row(
                "SELECT COUNT(*) > 0 FROM _migrations WHERE name = ?1",
                [name],
                |row| row.get(0),
            )?;

            if !already_applied {
                self.conn.execute_batch(sql)
                    .with_context(|| format!("Failed to run migration: {}", name))?;
                self.conn.execute(
                    "INSERT INTO _migrations (name) VALUES (?1)",
                    [name],
                )?;
            }
        }

        Ok(())
    }
}

//! Typed error for the rustic-db crate.
//!
//! Library crates expose typed errors so callers can pattern-match on
//! failure modes (e.g. distinguish "row not found" from "schema corrupt"
//! from "disk I/O error"). The previous shape used `anyhow::Error` which
//! was convenient but opaque at the boundary.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum DbError {
    #[error("sql: {0}")]
    Sql(#[from] rusqlite::Error),

    #[error("json: {0}")]
    Json(#[from] serde_json::Error),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// A migration failed to apply. Carries the migration name + the
    /// underlying SQL error message.
    #[error("migration `{name}` failed: {source}")]
    Migration {
        name: String,
        #[source]
        source: rusqlite::Error,
    },

    /// A row was expected but not found. Returned by lookups that have a
    /// non-Optional return type contract.
    #[error("not found: {0}")]
    NotFound(String),
}

pub type Result<T> = std::result::Result<T, DbError>;

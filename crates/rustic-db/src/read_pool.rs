use crate::connection::Database;
use crate::error::Result;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// Pool of read-only SQLite connections that read alongside the single writer under WAL.
pub struct ReadPool {
    path: PathBuf,
    idle: Mutex<Vec<Database>>,
    generation: AtomicU64,
    max_idle: usize,
}

/// A checked-out read connection; returned to the pool on drop unless the pool was cleared meanwhile.
pub struct PooledDb<'a> {
    pool: &'a ReadPool,
    db: Option<Database>,
    generation: u64,
}

impl ReadPool {
    /// Create a pool for the database file at `path` keeping up to `max_idle` idle readers.
    pub fn new(path: &Path, max_idle: usize) -> Self {
        Self {
            path: path.to_path_buf(),
            idle: Mutex::new(Vec::new()),
            generation: AtomicU64::new(0),
            max_idle: max_idle.max(1),
        }
    }

    /// Borrow a read-only connection, opening a new one when none is idle.
    pub fn checkout(&self) -> Result<PooledDb<'_>> {
        let generation = self.generation.load(Ordering::SeqCst);
        let reused = self.idle.lock().ok().and_then(|mut v| v.pop());
        let db = match reused {
            Some(db) => db,
            None => Database::open_reader(&self.path)?,
        };
        Ok(PooledDb {
            pool: self,
            db: Some(db),
            generation,
        })
    }

    /// Drop every idle reader and invalidate outstanding checkouts (call before replacing the DB file).
    pub fn clear(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut v) = self.idle.lock() {
            v.clear();
        }
    }

    /// Number of idle readers currently pooled.
    pub fn idle_count(&self) -> usize {
        self.idle.lock().map(|v| v.len()).unwrap_or(0)
    }
}

impl Deref for PooledDb<'_> {
    type Target = Database;

    #[allow(clippy::expect_used)] // only None inside Drop
    fn deref(&self) -> &Database {
        self.db.as_ref().expect("pooled db present until drop")
    }
}

impl Drop for PooledDb<'_> {
    fn drop(&mut self) {
        let Some(db) = self.db.take() else { return };
        if self.pool.generation.load(Ordering::SeqCst) != self.generation {
            return;
        }
        if let Ok(mut v) = self.pool.idle.lock() {
            if v.len() < self.pool.max_idle {
                v.push(db);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db_path(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("rustic-readpool-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("rustic.db")
    }

    #[test]
    fn readers_see_committed_writes_and_are_reused() {
        let path = temp_db_path("reuse");
        let writer = Database::new(&path).unwrap();
        writer
            .conn()
            .execute_batch("CREATE TABLE t (v INTEGER); INSERT INTO t VALUES (7);")
            .unwrap();
        let pool = ReadPool::new(&path, 2);
        {
            let r = pool.checkout().unwrap();
            let v: i64 = r
                .conn()
                .query_row("SELECT v FROM t", [], |row| row.get(0))
                .unwrap();
            assert_eq!(v, 7);
            assert!(r.conn().execute("INSERT INTO t VALUES (1)", []).is_err());
        }
        assert_eq!(pool.idle_count(), 1);
        writer.conn().execute("UPDATE t SET v = 9", []).unwrap();
        let r = pool.checkout().unwrap();
        let v: i64 = r
            .conn()
            .query_row("SELECT v FROM t", [], |row| row.get(0))
            .unwrap();
        assert_eq!(v, 9);
    }

    #[test]
    fn clear_discards_outstanding_checkouts() {
        let path = temp_db_path("clear");
        let _writer = Database::new(&path).unwrap();
        let pool = ReadPool::new(&path, 2);
        let r = pool.checkout().unwrap();
        pool.clear();
        drop(r);
        assert_eq!(pool.idle_count(), 0);
    }
}

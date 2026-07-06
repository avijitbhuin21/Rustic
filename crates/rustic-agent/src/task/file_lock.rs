use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// How long to wait for a per-file lock before giving up.
/// Once the lock IS acquired the operation runs to completion — this cap
/// only applies to the waiting-for-the-lock phase.
pub const LOCK_TIMEOUT_SECS: u64 = 30;

/// Per-file async mutex registry.
///
/// Use `acquire` / `acquire_owned` instead of `get_lock` directly — they
/// apply `LOCK_TIMEOUT_SECS` automatically and return a ready-to-use
/// `Result` so callers can early-return on timeout without boilerplate.
pub struct FileLockRegistry {
    locks: Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>,
}

impl FileLockRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            locks: Mutex::new(HashMap::new()),
        })
    }

    /// Get or create the per-file lock for `path`.
    /// Prefer `acquire` / `acquire_owned` over calling this directly.
    pub fn get_lock(&self, path: &Path) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self.locks.lock().unwrap();
        locks
            .entry(path.to_path_buf())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }

    /// Acquire the per-file lock with a `LOCK_TIMEOUT_SECS` deadline.
    ///
    /// Returns `Ok(guard)` once the lock is yours — after that the actual
    /// file operation runs to completion with no time limit.
    /// Returns `Err(message)` if the lock did not free within the deadline.
    ///
    /// Uses `lock_owned()` so the returned `OwnedMutexGuard` keeps its own
    /// `Arc` clone — no borrowed lifetime, safe to return from this method.
    pub async fn acquire(&self, path: &Path) -> Result<tokio::sync::OwnedMutexGuard<()>, String> {
        let lock = self.get_lock(path);
        match tokio::time::timeout(
            std::time::Duration::from_secs(LOCK_TIMEOUT_SECS),
            lock.lock_owned(),
        )
        .await
        {
            Ok(guard) => Ok(guard),
            Err(_) => Err(format!(
                "FILE_LOCK_TIMEOUT: '{}' is locked by another concurrent operation and \
                 did not release within {LOCK_TIMEOUT_SECS}s. A subagent or parallel \
                 tool call may still be writing this file. Wait a moment and retry.",
                path.display()
            )),
        }
    }
}

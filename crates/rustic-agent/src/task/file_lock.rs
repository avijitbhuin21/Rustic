use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Per-file async mutex registry.
///
/// Before reading or writing a file, call `get_lock()` to get the `Arc<tokio::sync::Mutex<()>>`
/// for that path, then acquire the lock with a timeout:
///
/// ```no_run
/// let file_lock = context.file_lock.get_lock(&full_path);
/// let _guard = match tokio::time::timeout(
///     std::time::Duration::from_secs(30),
///     file_lock.lock(),
/// ).await {
///     Ok(guard) => guard,
///     Err(_) => return LOCK_TIMEOUT error,
/// };
/// // file is exclusively locked for the rest of this scope
/// ```
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
    /// The returned `Arc` must be kept alive while the guard is held.
    pub fn get_lock(&self, path: &Path) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self.locks.lock().unwrap();
        locks
            .entry(path.to_path_buf())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }
}

pub const LOCK_TIMEOUT_SECS: u64 = 180;

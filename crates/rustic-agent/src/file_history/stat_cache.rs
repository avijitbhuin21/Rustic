//! Persistent per-project stat cache for the shadow snapshot store.
//!
//! `ShadowSnapshot::track()` historically re-read and re-hashed the entire
//! worktree on every call (every task's first user message), because there was
//! no record of which files were unchanged since the last snapshot. On large
//! repos this re-hash dominates chat-init latency.
//!
//! This cache is the equivalent of git's index stat-cache: it remembers, per
//! path, the `(mtime, size)` fingerprint that was last hashed and the resulting
//! blob oid. On the next walk, a file whose `(mtime, size)` still match can
//! reuse the stored blob oid and skip the `fs::read` + `write_blob` entirely —
//! provided the blob is still present in the shadow ODB (the caller verifies
//! that with `repo.has_object`, so a GC-pruned blob safely degrades to a
//! re-read).
//!
//! Persistence is a sidecar JSON file (`stat_cache.json`) next to the shadow
//! ODB it describes, so the cache survives app restarts (which is what makes
//! the *first* message after a relaunch fast) and is removed together with the
//! project's shadow repo. JSON via `serde_json` keeps this module free of any
//! `Database` coupling and off the global DB mutex.
//!
//! **Racy-git caveat:** a file changed externally while the app was closed,
//! with an identical size *and* mtime to the cached fingerprint, would be a
//! false cache hit (the stale blob is reused). Git's index makes the exact same
//! tradeoff; it is deemed acceptable here. The in-session FS-watcher path
//! (`track_paths`) is unaffected — it always re-reads the paths it is told
//! changed.

use std::collections::HashMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

const STAT_CACHE_FILE: &str = "stat_cache.json";
/// Bump when the on-disk shape changes incompatibly. A mismatch loads as empty
/// (a one-time cold rebuild), never an error.
const STAT_CACHE_VERSION: u32 = 1;

/// One cached file fingerprint. `blob_oid` is the 40-char hex form of the gix
/// `ObjectId` so the file is human-inspectable and we avoid a serde impl on a
/// foreign type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatCacheEntry {
    /// `mtime` as nanoseconds since the Unix epoch. Signed so pre-epoch mtimes
    /// map to a stable negative value rather than panicking.
    pub mtime_ns: i64,
    pub size: u64,
    pub blob_oid: String,
}

/// Per-project map of `rel_path` (forward-slash canonical) → fingerprint.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct StatCache {
    version: u32,
    entries: HashMap<String, StatCacheEntry>,
}

impl StatCache {
    /// An empty cache stamped with the current version (used on a cold start
    /// and as the fresh map a full `track()` rebuilds into).
    pub fn new() -> Self {
        StatCache {
            version: STAT_CACHE_VERSION,
            entries: HashMap::new(),
        }
    }

    /// Load the cache from `<repo_path>/stat_cache.json`. A missing, unreadable,
    /// corrupt, or version-mismatched file yields an empty cache (the next walk
    /// degrades to a full cold build). This never errors — a lost cache only
    /// costs one extra rebuild.
    pub fn load(repo_path: &Path) -> Self {
        let path = repo_path.join(STAT_CACHE_FILE);
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(_) => return Self::new(),
        };
        match serde_json::from_slice::<StatCache>(&bytes) {
            Ok(cache) if cache.version == STAT_CACHE_VERSION => cache,
            Ok(_) => {
                tracing::debug!(
                    path = %path.display(),
                    "stat cache version mismatch; rebuilding from scratch"
                );
                Self::new()
            }
            Err(e) => {
                tracing::debug!(path = %path.display(), ?e, "stat cache parse failed; rebuilding");
                Self::new()
            }
        }
    }

    /// Persist atomically: serialize to a `.tmp` sibling, then `fs::rename` over
    /// the real file so a crash mid-write never leaves a half-written cache (a
    /// torn file just loads as empty next time). Failures are logged and
    /// swallowed — the cache is a pure optimization.
    pub fn save(&self, repo_path: &Path) {
        let path = repo_path.join(STAT_CACHE_FILE);
        let tmp = repo_path.join(format!("{STAT_CACHE_FILE}.tmp"));
        let bytes = match serde_json::to_vec(self) {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(?e, "stat cache serialize failed; skipping save");
                return;
            }
        };
        if let Err(e) = std::fs::write(&tmp, &bytes) {
            tracing::warn!(path = %tmp.display(), ?e, "stat cache tmp write failed");
            return;
        }
        if let Err(e) = std::fs::rename(&tmp, &path) {
            tracing::warn!(path = %path.display(), ?e, "stat cache rename failed");
            // Best effort: clean up the orphaned tmp so it doesn't linger.
            let _ = std::fs::remove_file(&tmp);
        }
    }

    /// Return the cached blob oid (hex) for `rel_path` iff both `mtime_ns` and
    /// `size` match the stored fingerprint. The caller must still confirm the
    /// blob exists in the ODB before trusting the oid.
    pub fn get_if_match(&self, rel_path: &str, mtime_ns: i64, size: u64) -> Option<&str> {
        self.entries.get(rel_path).and_then(|e| {
            if e.mtime_ns == mtime_ns && e.size == size {
                Some(e.blob_oid.as_str())
            } else {
                None
            }
        })
    }

    /// Insert or overwrite the fingerprint for `rel_path`.
    pub fn insert(&mut self, rel_path: String, mtime_ns: i64, size: u64, blob_oid: String) {
        self.entries.insert(
            rel_path,
            StatCacheEntry {
                mtime_ns,
                size,
                blob_oid,
            },
        );
    }

    /// Drop the fingerprint for `rel_path` (deleted / now-ignored / too-large).
    pub fn remove(&mut self, rel_path: &str) {
        self.entries.remove(rel_path);
    }

    /// Number of cached entries. Exposed for tests/observability.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Convert a `SystemTime` mtime into a stable `i64` nanosecond key. Times before
/// the Unix epoch map to a negative value; the walk's `UNIX_EPOCH` sentinel
/// (used when a platform can't report mtime) maps to `0`. Never panics.
pub fn mtime_to_ns(mtime: SystemTime) -> i64 {
    match mtime.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_nanos() as i64,
        Err(e) => -(e.duration().as_nanos() as i64),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_missing_returns_empty_current_version() {
        let dir = tempfile::tempdir().unwrap();
        let cache = StatCache::load(dir.path());
        assert!(cache.is_empty());
        assert_eq!(cache.version, STAT_CACHE_VERSION);
    }

    #[test]
    fn save_then_load_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let mut cache = StatCache::new();
        cache.insert("src/main.rs".to_string(), 123, 45, "abc123".to_string());
        cache.save(dir.path());

        let loaded = StatCache::load(dir.path());
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded.get_if_match("src/main.rs", 123, 45), Some("abc123"));
        // Mismatched mtime or size → miss.
        assert_eq!(loaded.get_if_match("src/main.rs", 999, 45), None);
        assert_eq!(loaded.get_if_match("src/main.rs", 123, 99), None);
    }

    #[test]
    fn corrupt_file_loads_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(STAT_CACHE_FILE), b"{not valid json").unwrap();
        let cache = StatCache::load(dir.path());
        assert!(cache.is_empty());
    }

    #[test]
    fn version_mismatch_loads_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        // Hand-craft a payload with a future version.
        let json = format!(
            "{{\"version\":{},\"entries\":{{\"a\":{{\"mtime_ns\":1,\"size\":2,\"blob_oid\":\"x\"}}}}}}",
            STAT_CACHE_VERSION + 1
        );
        std::fs::write(dir.path().join(STAT_CACHE_FILE), json).unwrap();
        let cache = StatCache::load(dir.path());
        assert!(cache.is_empty());
    }

    #[test]
    fn remove_drops_entry() {
        let mut cache = StatCache::new();
        cache.insert("a".to_string(), 1, 1, "o".to_string());
        assert_eq!(cache.len(), 1);
        cache.remove("a");
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn mtime_to_ns_handles_epoch_and_pre_epoch() {
        assert_eq!(mtime_to_ns(UNIX_EPOCH), 0);
        let after = UNIX_EPOCH + std::time::Duration::from_nanos(1_500);
        assert_eq!(mtime_to_ns(after), 1_500);
        let before = UNIX_EPOCH - std::time::Duration::from_nanos(2_000);
        assert_eq!(mtime_to_ns(before), -2_000);
    }
}

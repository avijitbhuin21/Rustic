//! Bounded LRU cache of parsed trees, keyed by canonical file path.
//!
//! Each entry remembers the source-file `mtime` it was parsed at; a `get`
//! call only returns the cached tree when the current mtime still matches.
//! That's how we invalidate without subscribing to file-system events —
//! the symbol indexer hands us the current mtime when it asks for a parse,
//! and a mismatch transparently triggers a reparse.
//!
//! M1: storage is a `DashMap<PathBuf, Entry>`. Reads under different
//! paths take independent shard locks; writes only contend on the
//! shard the modified entry hashes to. The LRU eviction step iterates
//! the dashmap once to find the lowest `last_used` — short linear walk
//! at our capacities (~500 entries) and the alternative (LinkedHashMap)
//! reintroduces a single mutex, defeating the dashmap win.

use dashmap::DashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;
use tree_sitter::Tree;

struct Entry {
    mtime: SystemTime,
    tree: Tree,
    /// Monotonic counter for LRU ordering. Higher = more recently used.
    last_used: u64,
}

/// LRU cache of `(path, mtime) → Tree`. When `put` would push the cache past
/// `capacity`, the least-recently-used entry is evicted first.
pub struct TreeCache {
    capacity: usize,
    inner: DashMap<PathBuf, Entry>,
    tick: AtomicU64,
}

impl TreeCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            inner: DashMap::new(),
            tick: AtomicU64::new(0),
        }
    }

    /// Return the cached tree iff there's an entry whose stored mtime equals
    /// `mtime`. The access bumps the LRU position so this entry survives the
    /// next eviction. Returns `None` for missing entries OR stale mtime —
    /// the caller reparses in either case.
    pub fn get(&self, path: &Path, mtime: SystemTime) -> Option<Tree> {
        let now = self.next_tick();
        let mut entry = self.inner.get_mut(path)?;
        if entry.mtime != mtime {
            return None;
        }
        entry.last_used = now;
        Some(entry.tree.clone())
    }

    /// Insert or replace the tree for `path` at `mtime`. Evicts the LRU
    /// entry if doing so would exceed capacity. Replacing an existing key
    /// never triggers eviction.
    pub fn put(&self, path: PathBuf, mtime: SystemTime, tree: Tree) {
        let now = self.next_tick();
        let was_present = self.inner.contains_key(&path);
        self.inner.insert(
            path,
            Entry {
                mtime,
                tree,
                last_used: now,
            },
        );
        if !was_present {
            // Find-and-evict the LRU entry while we're over capacity.
            // Two-or-more puts racing each other can each see "size <=
            // cap" before any of them evicts, so a single eviction per
            // put isn't enough — keep evicting until we're back below.
            // Bounded by a soft cap so a pathological race can't spin
            // forever; in practice this loop runs once or twice.
            let mut safety = 16;
            while self.inner.len() > self.capacity && safety > 0 {
                safety -= 1;
                let victim = self
                    .inner
                    .iter()
                    .min_by_key(|kv| kv.value().last_used)
                    .map(|kv| kv.key().clone());
                match victim {
                    Some(v) => {
                        self.inner.remove(&v);
                    }
                    None => break,
                }
            }
        }
    }

    /// Drop the cached entry for `path`. Called by the host's file-watcher
    /// path on external changes (P1.2) and by edit tools after a write.
    pub fn invalidate(&self, path: &Path) {
        self.inner.remove(path);
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    fn next_tick(&self) -> u64 {
        self.tick.fetch_add(1, Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn fake_tree(source: &[u8]) -> Tree {
        let lang = rustic_core::syntax::LanguageRegistry::get_language("rust").unwrap();
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&lang).unwrap();
        parser.parse(source, None).unwrap()
    }

    #[test]
    fn miss_then_hit_on_same_mtime() {
        let cache = TreeCache::new(10);
        let p = PathBuf::from("/x/foo.rs");
        let mtime = SystemTime::UNIX_EPOCH;
        assert!(cache.get(&p, mtime).is_none());
        cache.put(p.clone(), mtime, fake_tree(b"fn main() {}"));
        assert!(cache.get(&p, mtime).is_some());
    }

    #[test]
    fn stale_mtime_returns_none() {
        let cache = TreeCache::new(10);
        let p = PathBuf::from("/x/foo.rs");
        let m1 = SystemTime::UNIX_EPOCH;
        let m2 = m1 + Duration::from_secs(60);
        cache.put(p.clone(), m1, fake_tree(b"fn main() {}"));
        assert!(cache.get(&p, m2).is_none());
    }

    #[test]
    fn lru_evicts_oldest_when_capacity_exceeded() {
        let cache = TreeCache::new(2);
        let t = fake_tree(b"fn x() {}");
        let mtime = SystemTime::UNIX_EPOCH;
        cache.put(PathBuf::from("/a.rs"), mtime, t.clone());
        cache.put(PathBuf::from("/b.rs"), mtime, t.clone());
        assert_eq!(cache.len(), 2);
        // Touch /a so /b is LRU.
        let _ = cache.get(Path::new("/a.rs"), mtime);
        cache.put(PathBuf::from("/c.rs"), mtime, t);
        assert_eq!(cache.len(), 2);
        assert!(cache.get(Path::new("/a.rs"), mtime).is_some());
        assert!(cache.get(Path::new("/b.rs"), mtime).is_none(), "/b should have been evicted");
        assert!(cache.get(Path::new("/c.rs"), mtime).is_some());
    }

    #[test]
    fn invalidate_drops_entry() {
        let cache = TreeCache::new(10);
        let p = PathBuf::from("/x/foo.rs");
        let mtime = SystemTime::UNIX_EPOCH;
        cache.put(p.clone(), mtime, fake_tree(b"fn main() {}"));
        cache.invalidate(&p);
        assert!(cache.get(&p, mtime).is_none());
    }

    #[test]
    fn replace_same_key_doesnt_evict() {
        let cache = TreeCache::new(2);
        let t = fake_tree(b"fn x() {}");
        let mtime = SystemTime::UNIX_EPOCH;
        cache.put(PathBuf::from("/a.rs"), mtime, t.clone());
        cache.put(PathBuf::from("/b.rs"), mtime, t.clone());
        // Re-put /a should not evict /b.
        cache.put(PathBuf::from("/a.rs"), mtime, t);
        assert_eq!(cache.len(), 2);
        assert!(cache.get(Path::new("/b.rs"), mtime).is_some());
    }

    // M1 — concurrent get/put across distinct paths shouldn't contend
    // catastrophically. Smoke test: 8 threads pumping their own paths
    // into the same cache, expect no deadlock and a final size at most
    // capacity.
    #[test]
    fn many_threads_distinct_paths_no_deadlock() {
        use std::sync::Arc;
        use std::thread;
        let cache = Arc::new(TreeCache::new(64));
        let t = fake_tree(b"fn x() {}");
        let mut handles = Vec::new();
        for tid in 0..8 {
            let c = Arc::clone(&cache);
            let tree = t.clone();
            handles.push(thread::spawn(move || {
                for i in 0..50 {
                    let p = PathBuf::from(format!("/t{}/f{}.rs", tid, i));
                    c.put(p.clone(), SystemTime::UNIX_EPOCH, tree.clone());
                    let _ = c.get(&p, SystemTime::UNIX_EPOCH);
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert!(cache.len() <= cache.capacity());
    }
}

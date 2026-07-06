//! P1.1 — Tree-sitter integration for the agent's code-intelligence layer.
//!
//! Owned by `WorkspaceServices` (one instance per opened project). Combines:
//!
//! - **[`ParserPool`]** — keeps `Parser` instances parked between calls so
//!   we don't pay construction + grammar-bind on every parse.
//! - **[`TreeCache`]** — bounded LRU of parsed trees keyed by canonical
//!   file path + the `mtime` at parse time. The mtime is what invalidates
//!   without needing a file-watcher subscription on the hot path.
//! - **[`detect`]** — file-extension → language-name lookup that matches
//!   `rustic_core::syntax::LanguageRegistry`.
//!
//! The typical agent flow is one call to `WorkspaceTreesitter::parse(path,
//! mtime, source)` — that returns the cached tree if it's still fresh, or
//! reparses through the pool and stashes the result.

pub mod cache;
pub mod detect;
pub mod pool;

pub use cache::TreeCache;
pub use detect::{language_for_extension, language_for_path};
pub use pool::ParserPool;
/// Re-export of rustic-core's grammar registry so downstream crates (e.g.
/// rustic-agent's symbol indexer) can compile `tree_sitter::Query` against a
/// language without taking a direct dependency on rustic-core.
pub use rustic_core::syntax::LanguageRegistry;

use std::path::Path;
use std::time::SystemTime;
use tree_sitter::Tree;

/// Façade combining a parser pool and a tree cache, sized for a single
/// project. Multiple concurrent tasks in that project share one instance
/// (via `Arc`), so the pool's parsers and the cache's trees are reused
/// across all of them.
pub struct WorkspaceTreesitter {
    pool: ParserPool,
    cache: TreeCache,
}

impl WorkspaceTreesitter {
    /// Default cache capacity: 500 trees. The plan's number, sized for the
    /// 6-project / 3–4-concurrent-task target.
    pub fn new() -> Self {
        Self::with_capacity(500)
    }

    pub fn with_capacity(cache_capacity: usize) -> Self {
        Self {
            pool: ParserPool::new(),
            cache: TreeCache::new(cache_capacity),
        }
    }

    /// Parse `source` for `path`. If the cache already has a tree for this
    /// path at `mtime`, returns the cached clone (cheap — tree-sitter trees
    /// are refcounted internally). Otherwise reparses through the pool and
    /// stashes the new tree under the same key.
    ///
    /// Returns `None` when the file extension isn't a known grammar; the
    /// caller (symbol indexer in P1.2) should skip non-source files.
    pub fn parse(&self, path: &Path, mtime: SystemTime, source: &[u8]) -> Option<Tree> {
        let lang_name = language_for_path(path)?;
        if let Some(cached) = self.cache.get(path, mtime) {
            return Some(cached);
        }
        let language = rustic_core::syntax::LanguageRegistry::get_language(lang_name)?;
        let tree = self
            .pool
            .with_parser(lang_name, language, |parser| parser.parse(source, None))?;
        self.cache.put(path.to_path_buf(), mtime, tree.clone());
        Some(tree)
    }

    /// Drop the cached tree for `path`. Called by edit tools after a
    /// successful write so the next read reparses against the new bytes,
    /// and by the file-watcher path for external changes (P1.2).
    pub fn invalidate(&self, path: &Path) {
        self.cache.invalidate(path);
    }

    pub fn cache_len(&self) -> usize {
        self.cache.len()
    }

    pub fn parsers_parked(&self) -> usize {
        self.pool.parked()
    }
}

impl Default for WorkspaceTreesitter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn end_to_end_parse_caches_tree() {
        let ws = WorkspaceTreesitter::new();
        let path = PathBuf::from("hello.rs");
        let mtime = SystemTime::UNIX_EPOCH;
        let source = b"fn main() { println!(\"hi\"); }";

        let tree1 = ws.parse(&path, mtime, source).expect("parse rust source");
        assert_eq!(ws.cache_len(), 1);
        // Second call at the same mtime hits the cache; cache size stays at 1.
        let tree2 = ws.parse(&path, mtime, source).expect("cached parse");
        assert_eq!(ws.cache_len(), 1);
        // Both trees have identical root-node ranges (same parse).
        assert_eq!(
            tree1.root_node().byte_range(),
            tree2.root_node().byte_range()
        );
    }

    #[test]
    fn unknown_extension_yields_none_and_doesnt_cache() {
        let ws = WorkspaceTreesitter::new();
        let path = PathBuf::from("README");
        assert!(ws.parse(&path, SystemTime::UNIX_EPOCH, b"# hi").is_none());
        assert_eq!(ws.cache_len(), 0);
    }

    #[test]
    fn mtime_change_triggers_reparse() {
        let ws = WorkspaceTreesitter::new();
        let path = PathBuf::from("hello.rs");
        let m1 = SystemTime::UNIX_EPOCH;
        let m2 = m1 + std::time::Duration::from_secs(60);

        ws.parse(&path, m1, b"fn a() {}").unwrap();
        let parked_after_first = ws.parsers_parked();
        // Same path, new mtime → cache miss → reparse → cache replaces.
        ws.parse(&path, m2, b"fn b() {}").unwrap();
        assert_eq!(ws.cache_len(), 1);
        // Parsers were reused (pool still has them).
        assert_eq!(ws.parsers_parked(), parked_after_first);
    }
}

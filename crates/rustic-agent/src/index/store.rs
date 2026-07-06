//! Storage container for the workspace symbol index.

use super::symbol::{SymbolEntry, SymbolKind};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::RwLock;

/// Current state of the index build. Reported to callers so a tool result
/// can flag "results are partial — still indexing" while the background
/// build is in flight.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexStatus {
    /// `ensure_built` has never been called on this project.
    NotStarted,
    /// Background build kicked off; queries return partial results.
    Building,
    /// Build finished at least once. Subsequent file refreshes do not
    /// move the status back to `Building`.
    Ready,
    /// Build failed (e.g. walker hit an IO error). Queries still work
    /// against whatever made it in.
    Failed,
}

/// Per-project workspace symbol index. Backed by an `RwLock`: queries are
/// the common case (every tool call), writes happen only during the
/// initial build and per-file refresh.
pub struct SymbolIndex {
    /// Inner table guarded for read-mostly access.
    inner: RwLock<SymbolIndexInner>,
    /// Latched once a build has started, so concurrent first-callers don't
    /// each spawn a duplicate build task. Reset to `false` only if the
    /// owner wants to force a rebuild.
    build_started: AtomicBool,
}

struct SymbolIndexInner {
    by_name: HashMap<String, Vec<SymbolEntry>>,
    by_file: HashMap<PathBuf, Vec<SymbolEntry>>,
    /// mtime recorded at the time the file was last indexed. Used by the
    /// `outline` tool (and any future caller) to decide whether to skip a
    /// refresh: if the on-disk mtime matches what's stored here, the
    /// indexed entries are already current and reparsing is wasted work.
    by_file_mtime: HashMap<PathBuf, std::time::SystemTime>,
    status: IndexStatus,
}

impl SymbolIndexInner {
    fn new() -> Self {
        Self {
            by_name: HashMap::new(),
            by_file: HashMap::new(),
            by_file_mtime: HashMap::new(),
            status: IndexStatus::NotStarted,
        }
    }
}

impl Default for SymbolIndex {
    fn default() -> Self {
        Self::new()
    }
}

impl SymbolIndex {
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(SymbolIndexInner::new()),
            build_started: AtomicBool::new(false),
        }
    }

    /// Returns true exactly once — the first caller wins. The host uses this
    /// to decide whether to spawn the background build task. Subsequent calls
    /// (from other tasks in the same project) return false, so the build
    /// fires once even when 4 tasks race to use the index.
    pub fn try_claim_build(&self) -> bool {
        !self.build_started.swap(true, Ordering::SeqCst)
    }

    pub fn status(&self) -> IndexStatus {
        self.inner
            .read()
            .map(|i| i.status)
            .unwrap_or(IndexStatus::Failed)
    }

    pub fn set_status(&self, status: IndexStatus) {
        if let Ok(mut inner) = self.inner.write() {
            inner.status = status;
        }
    }

    /// Number of unique names currently indexed. Diagnostic only.
    pub fn len(&self) -> usize {
        self.inner.read().map(|i| i.by_name.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Total number of files that have at least one entry. Diagnostic only.
    pub fn file_count(&self) -> usize {
        self.inner.read().map(|i| i.by_file.len()).unwrap_or(0)
    }

    /// Replace all entries that belong to `file` with `entries`. Used by both
    /// the initial build (one call per file) and the per-file refresh path
    /// (write tools call this after a successful edit). When `mtime` is
    /// supplied, it's recorded alongside the entries so callers can later
    /// ask `is_file_fresh(file, current_mtime)` and skip a redundant refresh.
    pub fn replace_file_entries(
        &self,
        file: PathBuf,
        entries: Vec<SymbolEntry>,
        mtime: Option<std::time::SystemTime>,
    ) {
        let Ok(mut inner) = self.inner.write() else {
            return;
        };
        // Step 1: drop any prior entries for this file from the by_name map.
        if let Some(old) = inner.by_file.remove(&file) {
            for entry in old {
                if let Some(slot) = inner.by_name.get_mut(&entry.name) {
                    slot.retain(|e| e.file != file);
                    if slot.is_empty() {
                        inner.by_name.remove(&entry.name);
                    }
                }
            }
        }
        // Always update mtime so a file that legitimately indexed to "no
        // symbols" doesn't get re-parsed on every outline call. This is
        // separate from `by_file` so the freshness check works even when
        // the file is symbol-empty.
        if let Some(m) = mtime {
            inner.by_file_mtime.insert(file.clone(), m);
        }
        // Step 2: insert the new entries. We keep one canonical copy in
        // by_file and clone into by_name so multi-name lookups don't have
        // to follow indirections.
        if entries.is_empty() {
            return;
        }
        inner.by_file.insert(file.clone(), entries.clone());
        for entry in entries {
            inner
                .by_name
                .entry(entry.name.clone())
                .or_default()
                .push(entry);
        }
    }

    /// Returns true when the symbol index already reflects `file` at the
    /// supplied `mtime`. Used by the `outline` tool to skip the per-call
    /// `refresh_file` reparse when nothing has changed on disk since the
    /// last index pass. Returns false if the file was never indexed
    /// (caller should call `refresh_file`) or its mtime differs.
    pub fn is_file_fresh(&self, file: &Path, current_mtime: std::time::SystemTime) -> bool {
        let Ok(inner) = self.inner.read() else {
            return false;
        };
        inner
            .by_file_mtime
            .get(file)
            .map(|recorded| *recorded == current_mtime)
            .unwrap_or(false)
    }

    /// Drop everything indexed under `file` (e.g. file was deleted).
    pub fn drop_file(&self, file: &Path) {
        let Ok(mut inner) = self.inner.write() else {
            return;
        };
        if let Some(old) = inner.by_file.remove(file) {
            for entry in old {
                if let Some(slot) = inner.by_name.get_mut(&entry.name) {
                    slot.retain(|e| e.file != file);
                    if slot.is_empty() {
                        inner.by_name.remove(&entry.name);
                    }
                }
            }
        }
        // Drop the mtime entry too so a later re-creation of the file with
        // the same on-disk mtime (rare but possible) triggers a fresh index
        // pass rather than incorrectly short-circuiting.
        inner.by_file_mtime.remove(file);
    }

    /// Look up symbols by exact name. Optionally filter by `kind`. Returns
    /// up to `limit` entries.
    pub fn find(&self, name: &str, kind: Option<SymbolKind>, limit: usize) -> Vec<SymbolEntry> {
        let Ok(inner) = self.inner.read() else {
            return Vec::new();
        };
        let Some(entries) = inner.by_name.get(name) else {
            return Vec::new();
        };
        entries
            .iter()
            .filter(|e| kind.map(|k| e.kind == k).unwrap_or(true))
            .take(limit)
            .cloned()
            .collect()
    }

    /// Substring / prefix search across all symbol names. Slow path (linear
    /// scan); used for fuzzy lookups when an exact match misses.
    pub fn find_substring(
        &self,
        needle: &str,
        kind: Option<SymbolKind>,
        limit: usize,
    ) -> Vec<SymbolEntry> {
        if needle.is_empty() {
            return Vec::new();
        }
        let needle_lower = needle.to_ascii_lowercase();
        let Ok(inner) = self.inner.read() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for (name, entries) in inner.by_name.iter() {
            if !name.to_ascii_lowercase().contains(&needle_lower) {
                continue;
            }
            for entry in entries {
                if kind.map(|k| entry.kind != k).unwrap_or(false) {
                    continue;
                }
                out.push(entry.clone());
                if out.len() >= limit {
                    return out;
                }
            }
        }
        out
    }

    /// All entries declared in `file`, in their source order (line ascending).
    pub fn entries_in_file(&self, file: &Path) -> Vec<SymbolEntry> {
        let Ok(inner) = self.inner.read() else {
            return Vec::new();
        };
        let mut out = inner.by_file.get(file).cloned().unwrap_or_default();
        out.sort_by_key(|e| (e.line, e.col));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, file: &str, line: u32, kind: SymbolKind) -> SymbolEntry {
        SymbolEntry {
            name: name.to_string(),
            file: PathBuf::from(file),
            line,
            col: 1,
            kind,
            scope: None,
        }
    }

    #[test]
    fn replace_file_entries_supplants_old() {
        let idx = SymbolIndex::new();
        idx.replace_file_entries(
            PathBuf::from("a.rs"),
            vec![entry("foo", "a.rs", 1, SymbolKind::Function)],
            None,
        );
        assert_eq!(idx.find("foo", None, 10).len(), 1);
        idx.replace_file_entries(
            PathBuf::from("a.rs"),
            vec![entry("bar", "a.rs", 2, SymbolKind::Function)],
            None,
        );
        assert_eq!(idx.find("foo", None, 10).len(), 0);
        assert_eq!(idx.find("bar", None, 10).len(), 1);
    }

    #[test]
    fn drop_file_removes_all_entries() {
        let idx = SymbolIndex::new();
        idx.replace_file_entries(
            PathBuf::from("a.rs"),
            vec![
                entry("foo", "a.rs", 1, SymbolKind::Function),
                entry("bar", "a.rs", 2, SymbolKind::Function),
            ],
            None,
        );
        idx.drop_file(Path::new("a.rs"));
        assert!(idx.is_empty());
        assert_eq!(idx.file_count(), 0);
    }

    #[test]
    fn kind_filter_works() {
        let idx = SymbolIndex::new();
        idx.replace_file_entries(
            PathBuf::from("a.rs"),
            vec![
                entry("foo", "a.rs", 1, SymbolKind::Function),
                entry("foo", "a.rs", 5, SymbolKind::Struct),
            ],
            None,
        );
        assert_eq!(idx.find("foo", Some(SymbolKind::Function), 10).len(), 1);
        assert_eq!(idx.find("foo", Some(SymbolKind::Struct), 10).len(), 1);
        assert_eq!(idx.find("foo", None, 10).len(), 2);
    }

    #[test]
    fn substring_search_is_case_insensitive() {
        let idx = SymbolIndex::new();
        idx.replace_file_entries(
            PathBuf::from("a.rs"),
            vec![
                entry("MyFunction", "a.rs", 1, SymbolKind::Function),
                entry("myOtherThing", "a.rs", 2, SymbolKind::Function),
            ],
            None,
        );
        let hits = idx.find_substring("myfunc", None, 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "MyFunction");
    }

    #[test]
    fn try_claim_build_fires_once() {
        let idx = SymbolIndex::new();
        assert!(idx.try_claim_build());
        assert!(!idx.try_claim_build());
        assert!(!idx.try_claim_build());
    }

    // L10 — `is_file_fresh` short-circuits the outline reparse.
    #[test]
    fn is_file_fresh_returns_false_for_unindexed_file() {
        let idx = SymbolIndex::new();
        let now = std::time::SystemTime::now();
        assert!(!idx.is_file_fresh(Path::new("never_seen.rs"), now));
    }

    #[test]
    fn is_file_fresh_matches_recorded_mtime_only() {
        let idx = SymbolIndex::new();
        let m1 = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000);
        let m2 = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(2_000);
        idx.replace_file_entries(
            PathBuf::from("a.rs"),
            vec![entry("foo", "a.rs", 1, SymbolKind::Function)],
            Some(m1),
        );
        assert!(idx.is_file_fresh(Path::new("a.rs"), m1));
        assert!(!idx.is_file_fresh(Path::new("a.rs"), m2));
    }

    #[test]
    fn is_file_fresh_tracks_even_when_no_symbols_found() {
        // A file that legitimately produces no symbols (empty .rs, unsupported
        // language, etc.) should still get its mtime recorded so the outline
        // tool doesn't re-parse it on every call.
        let idx = SymbolIndex::new();
        let mtime = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(42);
        idx.replace_file_entries(PathBuf::from("empty.rs"), vec![], Some(mtime));
        assert!(idx.is_file_fresh(Path::new("empty.rs"), mtime));
    }

    #[test]
    fn drop_file_clears_mtime_so_reindex_doesnt_short_circuit() {
        let idx = SymbolIndex::new();
        let mtime = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(7);
        idx.replace_file_entries(
            PathBuf::from("a.rs"),
            vec![entry("foo", "a.rs", 1, SymbolKind::Function)],
            Some(mtime),
        );
        idx.drop_file(Path::new("a.rs"));
        assert!(!idx.is_file_fresh(Path::new("a.rs"), mtime));
    }

    #[test]
    fn entries_in_file_returns_sorted_by_line() {
        let idx = SymbolIndex::new();
        idx.replace_file_entries(
            PathBuf::from("a.rs"),
            vec![
                entry("z", "a.rs", 30, SymbolKind::Function),
                entry("a", "a.rs", 5, SymbolKind::Function),
                entry("m", "a.rs", 15, SymbolKind::Function),
            ],
            None,
        );
        let listed = idx.entries_in_file(Path::new("a.rs"));
        assert_eq!(
            listed.iter().map(|e| e.line).collect::<Vec<_>>(),
            vec![5, 15, 30]
        );
    }
}

//! Storage container for the workspace symbol index.

use super::symbol::{SymbolEntry, SymbolKind};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

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

/// Compact stored form of a `SymbolEntry`. The file path lives once as the
/// `by_file` key; the name is shared with the `by_name` key; scopes are
/// interned per file. Public queries materialize `SymbolEntry` on demand.
struct StoredEntry {
    name: Arc<str>,
    line: u32,
    col: u32,
    kind: SymbolKind,
    scope: Option<Arc<str>>,
}

impl StoredEntry {
    fn materialize(&self, file: &Path) -> SymbolEntry {
        SymbolEntry {
            name: self.name.to_string(),
            file: file.to_path_buf(),
            line: self.line,
            col: self.col,
            kind: self.kind,
            scope: self.scope.as_deref().map(str::to_string),
        }
    }
}

struct SymbolIndexInner {
    /// Owns every entry exactly once, grouped by file.
    by_file: HashMap<Arc<Path>, Vec<StoredEntry>>,
    /// Name → (file, index into that file's `by_file` vec), in insertion order.
    by_name: HashMap<Arc<str>, Vec<(Arc<Path>, u32)>>,
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
        inner.remove_file_entries(&file);
        // Always update mtime so a file that legitimately indexed to "no
        // symbols" doesn't get re-parsed on every outline call. This is
        // separate from `by_file` so the freshness check works even when
        // the file is symbol-empty.
        if let Some(m) = mtime {
            inner.by_file_mtime.insert(file.clone(), m);
        }
        // Step 2: insert the new entries once (in by_file); by_name holds
        // (file, index) references. Names and scopes are interned so the
        // by_name key and the stored entry share one allocation.
        if entries.is_empty() {
            return;
        }
        let file: Arc<Path> = Arc::from(file);
        let mut scopes: HashMap<String, Arc<str>> = HashMap::new();
        let mut stored = Vec::with_capacity(entries.len());
        for (i, entry) in entries.into_iter().enumerate() {
            let name: Arc<str> = match inner.by_name.get_key_value(entry.name.as_str()) {
                Some((k, _)) => Arc::clone(k),
                None => Arc::from(entry.name.as_str()),
            };
            let scope = entry.scope.map(|s| {
                Arc::clone(scopes.entry(s).or_insert_with_key(|k| Arc::from(k.as_str())))
            });
            inner
                .by_name
                .entry(Arc::clone(&name))
                .or_default()
                .push((Arc::clone(&file), i as u32));
            stored.push(StoredEntry {
                name,
                line: entry.line,
                col: entry.col,
                kind: entry.kind,
                scope,
            });
        }
        inner.by_file.insert(file, stored);
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
        inner.remove_file_entries(file);
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
        let Some(refs) = inner.by_name.get(name) else {
            return Vec::new();
        };
        refs.iter()
            .filter_map(|r| inner.resolve(r))
            .filter(|(e, _)| kind.map(|k| e.kind == k).unwrap_or(true))
            .take(limit)
            .map(|(e, f)| e.materialize(f))
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
        for (name, refs) in inner.by_name.iter() {
            if !name.to_ascii_lowercase().contains(&needle_lower) {
                continue;
            }
            for (entry, file) in refs.iter().filter_map(|r| inner.resolve(r)) {
                if kind.map(|k| entry.kind != k).unwrap_or(false) {
                    continue;
                }
                out.push(entry.materialize(file));
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
        let mut out: Vec<SymbolEntry> = match inner.by_file.get_key_value(file) {
            Some((f, entries)) => entries.iter().map(|e| e.materialize(f)).collect(),
            None => Vec::new(),
        };
        out.sort_by_key(|e| (e.line, e.col));
        out
    }
}

impl SymbolIndexInner {
    fn resolve<'a>(&'a self, (file, idx): &'a (Arc<Path>, u32)) -> Option<(&'a StoredEntry, &'a Path)> {
        self.by_file
            .get(file)
            .and_then(|v| v.get(*idx as usize))
            .map(|e| (e, &**file))
    }

    fn remove_file_entries(&mut self, file: &Path) {
        let Some(old) = self.by_file.remove(file) else {
            return;
        };
        for entry in old {
            if let Some(slot) = self.by_name.get_mut(&*entry.name) {
                slot.retain(|(f, _)| &**f != file);
                if slot.is_empty() {
                    self.by_name.remove(&*entry.name);
                }
            }
        }
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

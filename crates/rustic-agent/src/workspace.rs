//! P1.3 — `WorkspaceServices`: per-project shared services for concurrent tasks.
//!
//! Multiple tasks running in the same project hold an `Arc` to the same
//! `WorkspaceServices`. Tree-sitter parsers (P1.1), the workspace symbol index
//! (P1.2), the file watcher, and any other cross-task per-project state live
//! here — once per project, not once per task. With 3–4 concurrent tasks in
//! one project, this is the slot that keeps RAM at 1× instead of 4×.
//!
//! Today this is the skeleton: a struct that carries the canonical project
//! root and a host-side registry that dedupes lookups. P1.1 and P1.2 plug
//! their state in via additional fields without disturbing the public API.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Per-project shared services. One instance per opened project; every task
/// (and every sub-agent it spawns) in that project holds an `Arc` clone.
///
/// Carries the project-scoped state that benefits from being single-instance
/// across concurrent tasks: a tree-sitter parser pool + tree cache today
/// (P1.1), and a symbol index + file watcher next (P1.2).
pub struct WorkspaceServices {
    /// Canonical project root. The registry uses this as its dedupe key so two
    /// tasks opened on `./foo` and `/abs/foo` end up holding the same handle.
    project_root: PathBuf,
    /// P1.1: tree-sitter parser pool + bounded tree cache. Owned by `Arc`
    /// so the symbol indexer (P1.2) and the agent's code-intel tools can
    /// hold their own handle without going through `WorkspaceServices`.
    tree_sitter: Arc<rustic_treesitter::WorkspaceTreesitter>,
    /// P1.2: workspace symbol index. Empty until first use; the code-intel
    /// tools call `ensure_built` which spawns a background build the first
    /// time it's needed.
    symbol_index: Arc<crate::index::SymbolIndex>,
}

impl WorkspaceServices {
    /// Build a fresh services bundle for `project_root`. Prefer
    /// `WorkspaceRegistry::get_or_create` so concurrent tasks in the same
    /// project share one instance.
    pub fn new(project_root: PathBuf) -> Self {
        Self {
            project_root,
            tree_sitter: Arc::new(rustic_treesitter::WorkspaceTreesitter::new()),
            symbol_index: Arc::new(crate::index::SymbolIndex::new()),
        }
    }

    /// The canonical project root this services bundle was built against.
    pub fn project_root(&self) -> &Path {
        &self.project_root
    }

    /// Shared tree-sitter parser pool + tree cache for this project.
    pub fn tree_sitter(&self) -> &Arc<rustic_treesitter::WorkspaceTreesitter> {
        &self.tree_sitter
    }

    /// Shared workspace symbol index for this project.
    pub fn symbol_index(&self) -> &Arc<crate::index::SymbolIndex> {
        &self.symbol_index
    }

    /// Idempotently kick off a background build of the symbol index. The
    /// first call wins (via `SymbolIndex::try_claim_build`); subsequent calls
    /// from other tasks in the same project return immediately while the
    /// build is running on a tokio blocking thread.
    ///
    /// Callers can query `symbol_index().status()` to find out whether the
    /// build is still in progress.
    pub fn ensure_index_build_started(self: &Arc<Self>) {
        if !self.symbol_index.try_claim_build() {
            return;
        }
        let services = Arc::clone(self);
        // Spawn on a blocking thread because file walk + tree-sitter parse
        // is sync + CPU-heavy and we don't want to starve the tokio runtime.
        std::thread::spawn(move || {
            let _stats = crate::index::build_full(
                &services.project_root,
                &services.tree_sitter,
                &services.symbol_index,
                || false,
            );
        });
    }

    /// External-change hook: the host's file watcher calls this when a file
    /// on disk changes (user edits in the IDE pane, git pull, etc.). Drops
    /// the cached tree and refreshes the symbol-index entries for that
    /// path. Cheap — no-op for files in unsupported languages.
    pub fn notify_file_changed(&self, path: &Path) {
        self.tree_sitter.invalidate(path);
        let _ = crate::index::refresh_file(path, &self.tree_sitter, &self.symbol_index);
    }

    /// External-deletion hook. Drops all symbol entries for the path.
    pub fn notify_file_deleted(&self, path: &Path) {
        self.tree_sitter.invalidate(path);
        self.symbol_index.drop_file(path);
    }
}

/// Host-side cache of `Arc<WorkspaceServices>` keyed by canonical project root.
///
/// The first task to touch a project root creates the entry; subsequent tasks
/// (and sub-agents) get the same `Arc` back. Entries are never evicted today —
/// the per-project memory footprint is small until P1.1/P1.2 add real state,
/// at which point a project-close hook can call `drop_project`.
#[derive(Default)]
pub struct WorkspaceRegistry {
    inner: Mutex<HashMap<PathBuf, Arc<WorkspaceServices>>>,
}

impl WorkspaceRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Return the shared services for `project_root`, creating them on first
    /// use. The path is canonicalized before lookup; if canonicalization fails
    /// (project not yet on disk, permission denied, etc.) the path is used
    /// verbatim so unit tests and embedded callers still get a valid handle.
    pub fn get_or_create(&self, project_root: &Path) -> Arc<WorkspaceServices> {
        let key = project_root
            .canonicalize()
            .unwrap_or_else(|_| project_root.to_path_buf());
        let mut map = self
            .inner
            .lock()
            .expect("workspace registry mutex poisoned");
        if let Some(existing) = map.get(&key) {
            return Arc::clone(existing);
        }
        let services = Arc::new(WorkspaceServices::new(key.clone()));
        map.insert(key, Arc::clone(&services));
        services
    }

    /// Drop the cached entry for `project_root` (canonicalized). Call this when
    /// a project is closed from the workspace so any per-project state owned
    /// by `WorkspaceServices` (parsers, symbol index, file watcher) is freed.
    pub fn drop_project(&self, project_root: &Path) -> bool {
        let key = project_root
            .canonicalize()
            .unwrap_or_else(|_| project_root.to_path_buf());
        let mut map = self
            .inner
            .lock()
            .expect("workspace registry mutex poisoned");
        map.remove(&key).is_some()
    }

    /// Number of cached project handles. Diagnostic only.
    pub fn len(&self) -> usize {
        self.inner.lock().map(|m| m.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_subdir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rustic-ws-test-{}", name));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn registry_returns_same_arc_for_same_path() {
        let reg = WorkspaceRegistry::new();
        let p = temp_subdir("dedupe");
        let a = reg.get_or_create(&p);
        let b = reg.get_or_create(&p);
        assert!(Arc::ptr_eq(&a, &b));
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn registry_separates_distinct_projects() {
        let reg = WorkspaceRegistry::new();
        let p1 = temp_subdir("distinct-a");
        let p2 = temp_subdir("distinct-b");
        let a = reg.get_or_create(&p1);
        let b = reg.get_or_create(&p2);
        assert!(!Arc::ptr_eq(&a, &b));
        assert_eq!(reg.len(), 2);
    }

    #[test]
    fn drop_project_removes_entry() {
        let reg = WorkspaceRegistry::new();
        let p = temp_subdir("drop");
        let _ = reg.get_or_create(&p);
        assert_eq!(reg.len(), 1);
        assert!(reg.drop_project(&p));
        assert!(reg.is_empty());
        assert!(!reg.drop_project(&p), "second drop is a no-op");
    }

    #[test]
    fn workspace_services_remembers_root() {
        let p = temp_subdir("root");
        let ws = WorkspaceServices::new(p.clone());
        assert_eq!(ws.project_root(), p.as_path());
    }

    #[test]
    fn registry_handles_missing_path_without_panicking() {
        // Canonicalize fails for paths that don't exist; the registry should
        // still produce a usable handle keyed by the as-given path.
        let reg = WorkspaceRegistry::new();
        let missing = PathBuf::from("/this/path/does/not/exist/rustic-ws");
        let a = reg.get_or_create(&missing);
        let b = reg.get_or_create(&missing);
        assert!(Arc::ptr_eq(&a, &b));
    }
}

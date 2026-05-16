//! Index build pipeline.
//!
//! `build_full` walks a project root with the `ignore` crate (so `.gitignore`,
//! `.ignore`, hidden files, and binary-detected files are skipped) and feeds
//! every source file through the tree-sitter parse + tags-query pipeline.
//! Each file's entries are then pushed into the per-project `SymbolIndex`.
//!
//! `refresh_file` is the single-file version, called by edit tools after a
//! successful write so the index never drifts from disk during a session.

use super::queries::{kind_from_capture, query_source};
use super::store::{IndexStatus, SymbolIndex};
use super::symbol::SymbolEntry;
use ignore::WalkBuilder;
use rustic_treesitter::WorkspaceTreesitter;
use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;
use streaming_iterator::StreamingIterator;
use tree_sitter::{Query, QueryCursor};

/// Stats from one build pass. Used for logging and for the indexing-status
/// surface the agent's tool output can mention.
#[derive(Debug, Default, Clone, Copy)]
pub struct IndexBuildStats {
    pub files_visited: usize,
    pub files_indexed: usize,
    pub files_skipped: usize,
    pub symbols_recorded: usize,
}

/// Build the full symbol index for `project_root`. Walks the file system,
/// parses every supported source file, and populates `index`.
///
/// Designed to run on a tokio blocking thread (it does sync IO + CPU-heavy
/// parsing). The caller is responsible for spawning it; this fn just runs
/// the work to completion and returns stats.
///
/// The build is interruptible at a coarse granularity via the `should_stop`
/// closure — when it returns true the walker stops on the next file. The
/// host uses this to bail out when a project is closed mid-build.
pub fn build_full(
    project_root: &Path,
    ts: &Arc<WorkspaceTreesitter>,
    index: &Arc<SymbolIndex>,
    should_stop: impl Fn() -> bool,
) -> IndexBuildStats {
    index.set_status(IndexStatus::Building);
    let mut stats = IndexBuildStats::default();

    let walker = WalkBuilder::new(project_root)
        .hidden(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        // Apply `.gitignore` even outside a git repo. Users open projects
        // they haven't run `git init` on; the file is still authoritative
        // about what to skip.
        .require_git(false)
        .build();

    for result in walker {
        if should_stop() {
            tracing::info!(target: "rustic_agent::index", "build_full: stop signal received");
            break;
        }
        let entry = match result {
            Ok(e) => e,
            Err(err) => {
                tracing::debug!(target: "rustic_agent::index", error = %err, "walker entry error");
                continue;
            }
        };
        stats.files_visited += 1;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        match index_one(path, ts, index) {
            Ok(count) => {
                if count == 0 {
                    stats.files_skipped += 1;
                } else {
                    stats.files_indexed += 1;
                    stats.symbols_recorded += count;
                }
            }
            Err(err) => {
                tracing::debug!(
                    target: "rustic_agent::index",
                    path = %path.display(),
                    error = %err,
                    "indexing skipped one file"
                );
                stats.files_skipped += 1;
            }
        }
    }

    index.set_status(IndexStatus::Ready);
    tracing::info!(
        target: "rustic_agent::index",
        root = %project_root.display(),
        files_visited = stats.files_visited,
        files_indexed = stats.files_indexed,
        files_skipped = stats.files_skipped,
        symbols_recorded = stats.symbols_recorded,
        "build_full completed"
    );
    stats
}

/// Re-index a single file. Called by write tools after a successful edit
/// and by file-watcher callbacks for external changes.
///
/// Returns the number of symbols recorded (0 means either the file is not a
/// supported source format or it produced no top-level declarations — both
/// are valid outcomes).
pub fn refresh_file(
    path: &Path,
    ts: &Arc<WorkspaceTreesitter>,
    index: &Arc<SymbolIndex>,
) -> std::io::Result<usize> {
    // If the file vanished, drop its old entries and bail.
    if !path.exists() {
        index.drop_file(path);
        return Ok(0);
    }
    index_one(path, ts, index).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
}

/// Parse + query + insert for one file. Returns the number of entries
/// recorded. Returns an Err only on IO failures or query compile failures —
/// "no symbols found" is a successful zero.
fn index_one(
    path: &Path,
    ts: &Arc<WorkspaceTreesitter>,
    index: &Arc<SymbolIndex>,
) -> Result<usize, String> {
    let lang_name = match rustic_treesitter::language_for_path(path) {
        Some(n) => n,
        None => return Ok(0),
    };
    let query_src = match query_source(lang_name) {
        Some(q) => q,
        None => return Ok(0),
    };

    let bytes = std::fs::read(path).map_err(|e| format!("read failed: {}", e))?;
    // Hard cap on file size to avoid OOM on accidentally-checked-in
    // bundle.min.js or generated payloads. 2 MiB covers any reasonable
    // hand-written source file across the supported languages.
    if bytes.len() > 2 * 1024 * 1024 {
        return Ok(0);
    }
    let mtime = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);

    let tree = match ts.parse(path, mtime, &bytes) {
        Some(t) => t,
        None => return Ok(0),
    };
    let language = rustic_treesitter::LanguageRegistry::get_language(lang_name)
        .ok_or_else(|| format!("no grammar registered for language `{}`", lang_name))?;
    // C1 / C2 hygiene: a query that fails to compile against the live
    // grammar (most commonly a grammar-ABI-version mismatch, e.g. the
    // tree-sitter-markdown crate shipping ABI 15 against our tree-sitter
    // 0.24's max ABI 14) is treated as "no symbol coverage for this
    // language" — same as no query source. Logged via tracing so the
    // mismatch is debuggable, but the file scan continues without
    // failing the build.
    let query = match Query::new(&language, query_src) {
        Ok(q) => q,
        Err(e) => {
            tracing::warn!(
                language = %lang_name,
                error = %e,
                "tags query failed to compile against the active grammar — \
                 skipping symbol indexing for this language until the query is updated"
            );
            return Ok(0);
        }
    };
    let capture_names: Vec<String> = query
        .capture_names()
        .iter()
        .map(|s| s.to_string())
        .collect();

    let mut cursor = QueryCursor::new();
    let root = tree.root_node();
    let mut matches = cursor.matches(&query, root, bytes.as_slice());
    // Per-position dedupe. A nested `function_item` inside an `impl_item`
    // matches both the generic `name.function` pattern and the `name.method`
    // pattern. Pick the more specific kind so the agent sees one canonical
    // entry per declaration site.
    let mut by_position: std::collections::HashMap<(usize, usize), SymbolEntry> =
        std::collections::HashMap::new();

    while let Some(m) = matches.next() {
        for cap in m.captures {
            let idx = cap.index as usize;
            let cname = match capture_names.get(idx) {
                Some(n) => n.as_str(),
                None => continue,
            };
            let Some(kind) = kind_from_capture(cname) else {
                continue;
            };
            let node = cap.node;
            let name = match node.utf8_text(&bytes) {
                Ok(s) => s.trim().to_string(),
                Err(_) => continue,
            };
            if name.is_empty() {
                continue;
            }
            let start = node.start_position();
            let scope = compute_scope(&node, &bytes);
            let entry = SymbolEntry {
                name,
                file: path.to_path_buf(),
                line: (start.row as u32).saturating_add(1),
                col: (start.column as u32).saturating_add(1),
                kind,
                scope,
            };
            let key = (start.row, start.column);
            match by_position.get(&key) {
                Some(existing) if kind_specificity(existing.kind) >= kind_specificity(kind) => {
                    // Keep the existing, more-specific entry.
                }
                _ => {
                    by_position.insert(key, entry);
                }
            }
        }
    }

    let entries: Vec<SymbolEntry> = by_position.into_values().collect();
    let count = entries.len();
    index.replace_file_entries(path.to_path_buf(), entries, Some(mtime));
    Ok(count)
}

/// Higher = more specific. When two patterns capture the same node, the more
/// specific kind wins (e.g. an `impl Foo { fn bar(&self) }` produces both
/// `name.function` and `name.method` matches; we keep the method).
fn kind_specificity(kind: super::symbol::SymbolKind) -> u8 {
    use super::symbol::SymbolKind::*;
    match kind {
        Method => 5,
        Macro => 4,
        Constant => 4,
        Function => 3,
        Class | Struct | Enum | Trait | Interface | TypeAlias | Module => 2,
        Variable => 1,
    }
}

/// Walk up from `node` looking for an enclosing class/impl/trait/module so
/// methods can be reported as `Foo::bar` (or "in `class Foo`"). Cheap
/// enough to run per capture; we stop at the first useful ancestor.
fn compute_scope(node: &tree_sitter::Node<'_>, bytes: &[u8]) -> Option<String> {
    let mut current = node.parent();
    while let Some(n) = current {
        let kind = n.kind();
        if matches!(
            kind,
            "impl_item"
                | "trait_item"
                | "class_declaration"
                | "class_definition"
                | "class_specifier"
                | "class"
                | "interface_declaration"
                | "struct_item"
                | "object_declaration"
                | "module"
                | "namespace_definition"
        ) {
            // Most of these grammars expose a `name:` or `type:` field on the
            // enclosing node. Try a couple common field names; if none match,
            // give up and return None rather than guess.
            for field in ["name", "type", "type_identifier"] {
                if let Some(name_node) = n.child_by_field_name(field) {
                    if let Ok(s) = name_node.utf8_text(bytes) {
                        return Some(s.trim().to_string());
                    }
                }
            }
            return None;
        }
        current = n.parent();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_temp_file(name: &str, body: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(name);
        std::fs::write(&path, body).expect("write");
        dir
    }

    #[test]
    fn indexes_a_rust_function_and_struct() {
        let dir = write_temp_file(
            "lib.rs",
            r#"
pub struct Widget;

pub fn launch_widget() -> Widget {
    Widget
}

impl Widget {
    pub fn name(&self) -> &str { "w" }
}
"#,
        );
        let ts = Arc::new(WorkspaceTreesitter::new());
        let idx = Arc::new(SymbolIndex::new());
        let stats = build_full(dir.path(), &ts, &idx, || false);
        assert!(stats.files_indexed >= 1);
        assert!(!idx.find("launch_widget", None, 10).is_empty());
        assert!(!idx.find("Widget", None, 10).is_empty());
        // The method should land with `Widget` as its scope.
        let methods = idx.find("name", None, 10);
        assert_eq!(methods.len(), 1);
        assert_eq!(methods[0].scope.as_deref(), Some("Widget"));
        assert_eq!(idx.status(), IndexStatus::Ready);
    }

    #[test]
    fn refresh_file_replaces_old_entries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("foo.rs");
        std::fs::write(&path, "fn first() {}").unwrap();
        let ts = Arc::new(WorkspaceTreesitter::new());
        let idx = Arc::new(SymbolIndex::new());
        refresh_file(&path, &ts, &idx).unwrap();
        assert_eq!(idx.find("first", None, 10).len(), 1);

        // Rewrite and refresh: old entry should be gone.
        std::fs::write(&path, "fn second() {}").unwrap();
        refresh_file(&path, &ts, &idx).unwrap();
        assert_eq!(idx.find("first", None, 10).len(), 0);
        assert_eq!(idx.find("second", None, 10).len(), 1);
    }

    #[test]
    fn deleted_file_is_dropped_on_refresh() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("foo.rs");
        std::fs::write(&path, "fn ghost() {}").unwrap();
        let ts = Arc::new(WorkspaceTreesitter::new());
        let idx = Arc::new(SymbolIndex::new());
        refresh_file(&path, &ts, &idx).unwrap();
        assert_eq!(idx.find("ghost", None, 10).len(), 1);

        std::fs::remove_file(&path).unwrap();
        refresh_file(&path, &ts, &idx).unwrap();
        assert_eq!(idx.find("ghost", None, 10).len(), 0);
    }

    #[test]
    fn build_respects_gitignore() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".gitignore"), "ignored/\n").unwrap();
        std::fs::create_dir(dir.path().join("ignored")).unwrap();
        std::fs::write(dir.path().join("ignored").join("h.rs"), "fn hidden() {}").unwrap();
        std::fs::write(dir.path().join("visible.rs"), "fn visible() {}").unwrap();

        let ts = Arc::new(WorkspaceTreesitter::new());
        let idx = Arc::new(SymbolIndex::new());
        build_full(dir.path(), &ts, &idx, || false);
        assert!(!idx.find("visible", None, 10).is_empty());
        assert!(idx.find("hidden", None, 10).is_empty(), "gitignored file should be skipped");
    }
}

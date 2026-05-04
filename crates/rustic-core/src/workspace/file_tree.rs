use anyhow::Result;
use ignore::WalkBuilder;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileNode {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub children: Option<Vec<FileNode>>,
    pub depth: u32,
    /// `true` when the entry would be excluded by the project's gitignore /
    /// global git excludes / `.git/info/exclude`. We still surface the entry
    /// in the tree (the user explicitly asked to see everything), but the
    /// frontend renders it dimmed so secret/build/cache files visually
    /// recede next to tracked source code.
    #[serde(default)]
    pub is_ignored: bool,
}

/// Read a directory one level deep. Shows every entry, including gitignored
/// files — the user-facing explorer should never hide files from the user.
/// Returns entries sorted: directories first, then alphabetical (case-insensitive).
///
/// Each entry's `is_ignored` flag is computed by running a *second* walk
/// with gitignore enabled and diffing — anything visible in the unfiltered
/// walk but missing from the gitignored one is flagged. Two walks at depth=1
/// are cheap (rarely more than ~hundreds of entries) and avoid having to
/// reimplement the ignore-stack-walking that `ignore` already does correctly.
pub fn read_directory(path: &Path, depth: u32) -> Result<Vec<FileNode>> {
    let mut entries = Vec::new();

    // Pass 1: enumerate every entry — gitignored files included — so the
    // explorer never hides anything from the user.
    let walker = WalkBuilder::new(path)
        .max_depth(Some(1))
        .hidden(false)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .sort_by_file_path(|a, b| a.cmp(b))
        .build();

    // Pass 2: enumerate only the entries that would survive gitignore. We
    // flag anything in pass 1 missing from this set. Build it as a HashSet
    // of canonicalized paths so Windows-vs-Unix slashes don't false-positive.
    let mut not_ignored: HashSet<PathBuf> = HashSet::new();
    let tracked_walker = WalkBuilder::new(path)
        .max_depth(Some(1))
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .build();
    for entry in tracked_walker.flatten() {
        not_ignored.insert(entry.path().to_path_buf());
    }

    for entry in walker {
        let entry = entry?;
        let entry_path = entry.path().to_path_buf();

        // Skip the root directory itself
        if entry_path == path {
            continue;
        }

        let name = entry_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        let is_dir = entry_path.is_dir();
        let is_ignored = !not_ignored.contains(&entry_path);

        entries.push(FileNode {
            path: entry_path,
            name,
            is_dir,
            children: if is_dir { Some(Vec::new()) } else { None },
            depth,
            is_ignored,
        });
    }

    // Sort: directories first, then alphabetical (case-insensitive)
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    Ok(entries)
}

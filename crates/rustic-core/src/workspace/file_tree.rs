use anyhow::Result;
use ignore::WalkBuilder;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileNode {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub children: Option<Vec<FileNode>>,
    pub depth: u32,
}

/// Read a directory one level deep, respecting .gitignore.
/// Returns entries sorted: directories first, then alphabetical (case-insensitive).
pub fn read_directory(path: &Path, depth: u32) -> Result<Vec<FileNode>> {
    let mut entries = Vec::new();

    let walker = WalkBuilder::new(path)
        .max_depth(Some(1))
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .sort_by_file_path(|a, b| a.cmp(b))
        .build();

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

        // Skip hidden files/dirs starting with .
        // (ignore crate handles .gitignore but not all hidden files)
        let is_dir = entry_path.is_dir();

        entries.push(FileNode {
            path: entry_path,
            name,
            is_dir,
            children: if is_dir { Some(Vec::new()) } else { None },
            depth,
        });
    }

    // Always include .rustic if it exists — it may be gitignored but is project config.
    let rustic_path = path.join(".rustic");
    if rustic_path.exists() && !entries.iter().any(|e| e.name == ".rustic") {
        entries.push(FileNode {
            path: rustic_path,
            name: ".rustic".to_string(),
            is_dir: true,
            children: Some(Vec::new()),
            depth,
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

/// Generate a file-tree representation of a project directory.
///
/// Uses the `ignore` crate plus a hardcoded exclusion list for common bloat
/// directories.  The output is a human-readable tree string suitable for
/// embedding in a system prompt.
///
/// When `include_gitignored` is `false` (the default), `.gitignore` rules are
/// respected so the agent does not see files the user has explicitly chosen to
/// keep out of version control.  When `true` (FullAuto mode or the "Grant
/// access to all files" toggle), gitignore is bypassed and the agent sees the
/// full project tree.
use std::cmp::Ordering;
use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::path::Path;

/// Directories that are always excluded regardless of `.gitignore`.
pub const EXCLUDED_DIRS: &[&str] = &[
    "node_modules",
    "target",
    ".git",
    "dist",
    "build",
    "out",
    "__pycache__",
    ".venv",
    "venv",
    ".env",
    ".next",
    ".nuxt",
    ".cache",
    ".turbo",
    ".parcel-cache",
    "coverage",
    ".idea",
    ".vscode",
    ".DS_Store",
    "Thumbs.db",
];

/// Maximum directory depth (0 = root only, 5 = root + 5 levels of nesting).
const MAX_DEPTH: usize = 5;

/// Maximum number of entries (files + dirs) to include.
const MAX_ENTRIES: usize = 500;

/// Caller-tunable variant of the tree walker. Used by the Global
/// orchestrator's `list_projects`, which needs a compact layout overview
/// across many projects rather than the full per-project tree.
pub fn generate_file_tree_with_limits(
    project_root: &Path,
    include_gitignored: bool,
    max_depth: usize,
    max_entries: usize,
) -> String {
    generate_tree_inner(project_root, include_gitignored, max_depth, max_entries)
}

/// Generate a file tree string for `project_root`.
///
/// Returns something like:
/// ```text
/// Cargo.toml
/// package.json
/// src/
///   components/
///     agent/
///       agent-panel.js
///       chat-view.js
///   lib/
///     tauri-api.js
/// crates/
///   rustic-agent/
///     src/
///       lib.rs
///       system_prompt.rs
/// ```
pub fn generate_file_tree(project_root: &Path, include_gitignored: bool) -> String {
    generate_tree_inner(project_root, include_gitignored, MAX_DEPTH, MAX_ENTRIES)
}

/// Whether a successful call to `tool_name` can add, remove, or rename files,
/// invalidating any cached file-tree snapshot held by a host.
pub fn tool_mutates_file_tree(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "create_file"
            | "move_file"
            | "run_command"
            | "spawn_subagent"
            | "edit_notebook"
            | "image_create"
            | "video_create"
            | "animate"
    )
}

/// Stable in-process fingerprint of a rendered tree. Hosts compare this
/// between turns to decide whether the layout actually changed before paying
/// to re-send the tree to the model.
pub fn tree_fingerprint(tree: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    tree.hash(&mut hasher);
    hasher.finish()
}

/// Entries every top-level subtree is guaranteed, before leftover budget is
/// redistributed. Without a floor, one huge vendored directory sorted early in
/// the alphabet consumes the whole budget and hides real source directories.
const MIN_SUBTREE_ENTRIES: usize = 20;

/// One immediate child of the project root.
struct TopEntry {
    name: String,
    is_dir: bool,
}

/// Rendered lines for one top-level subtree plus whether its budget ran out.
struct SubtreeResult {
    lines: Vec<String>,
    count: usize,
    hit_cap: bool,
}

fn walker_for(
    root: &Path,
    include_gitignored: bool,
    max_depth: usize,
    excluded: HashSet<&'static str>,
) -> ignore::Walk {
    let respect_gitignore = !include_gitignored;
    ignore::WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(respect_gitignore)
        .git_global(respect_gitignore)
        .git_exclude(respect_gitignore)
        .max_depth(Some(max_depth))
        .filter_entry(move |entry| {
            if entry.depth() == 0 {
                return true;
            }
            !excluded.contains(entry.file_name().to_string_lossy().as_ref())
        })
        .sort_by_file_path(|a, b| {
            let a_is_dir = a.is_dir();
            let b_is_dir = b.is_dir();
            match (a_is_dir, b_is_dir) {
                (true, false) => Ordering::Less,
                (false, true) => Ordering::Greater,
                _ => a
                    .file_name()
                    .map(|n| n.to_ascii_lowercase())
                    .cmp(&b.file_name().map(|n| n.to_ascii_lowercase())),
            }
        })
        .build()
}

fn excluded_set() -> HashSet<&'static str> {
    EXCLUDED_DIRS.iter().copied().collect()
}

/// List the project root's immediate children, directories first.
fn list_top_level(root: &Path, include_gitignored: bool) -> Vec<TopEntry> {
    let mut out = Vec::new();
    for result in walker_for(root, include_gitignored, 1, excluded_set()) {
        let Ok(entry) = result else { continue };
        if entry.depth() == 0 {
            continue;
        }
        out.push(TopEntry {
            name: entry.file_name().to_string_lossy().to_string(),
            is_dir: entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false),
        });
    }
    out
}

/// Render one top-level directory's contents, stopping at `cap` entries.
fn walk_subtree(
    dir: &Path,
    include_gitignored: bool,
    max_depth: usize,
    cap: usize,
) -> SubtreeResult {
    let mut lines = Vec::new();
    let mut hit_cap = false;
    for result in walker_for(dir, include_gitignored, max_depth, excluded_set()) {
        let Ok(entry) = result else { continue };
        if entry.depth() == 0 {
            continue;
        }
        if lines.len() >= cap {
            hit_cap = true;
            break;
        }
        let mut line = String::new();
        for _ in 0..entry.depth() {
            line.push_str("  ");
        }
        line.push_str(&entry.file_name().to_string_lossy());
        if entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
            line.push('/');
        }
        lines.push(line);
    }
    let count = lines.len();
    SubtreeResult {
        lines,
        count,
        hit_cap,
    }
}

fn generate_tree_inner(
    project_root: &Path,
    include_gitignored: bool,
    max_depth: usize,
    max_entries: usize,
) -> String {
    let top = list_top_level(project_root, include_gitignored);
    let top_dirs: Vec<&TopEntry> = top.iter().filter(|e| e.is_dir).collect();
    let top_files: Vec<&TopEntry> = top.iter().filter(|e| !e.is_dir).collect();

    let mut remaining = max_entries.saturating_sub(top.len());
    let base_cap = if top_dirs.is_empty() {
        0
    } else {
        (remaining / top_dirs.len()).max(MIN_SUBTREE_ENTRIES)
    };

    let mut results: Vec<SubtreeResult> = Vec::with_capacity(top_dirs.len());
    for dir in &top_dirs {
        let cap = base_cap.min(remaining);
        let result = walk_subtree(
            &project_root.join(&dir.name),
            include_gitignored,
            max_depth,
            cap,
        );
        remaining = remaining.saturating_sub(result.count);
        results.push(result);
    }

    // Subtrees that fit under their share leave budget on the table; hand it to
    // the ones that were cut off so a large source directory still gets listed.
    let capped: Vec<usize> = results
        .iter()
        .enumerate()
        .filter(|(_, r)| r.hit_cap)
        .map(|(i, _)| i)
        .collect();
    if !capped.is_empty() && remaining >= MIN_SUBTREE_ENTRIES {
        let extra = remaining / capped.len();
        if extra > 0 {
            for i in capped {
                let result = walk_subtree(
                    &project_root.join(&top_dirs[i].name),
                    include_gitignored,
                    max_depth,
                    results[i].count + extra,
                );
                remaining = remaining.saturating_sub(result.count - results[i].count);
                results[i] = result;
            }
        }
    }

    let mut out = String::with_capacity(max_entries * 40);
    let mut any_elided = false;
    for (i, dir) in top_dirs.iter().enumerate() {
        out.push_str(&dir.name);
        out.push_str("/\n");
        for line in &results[i].lines {
            out.push_str(line);
            out.push('\n');
        }
        if results[i].hit_cap {
            any_elided = true;
            out.push_str(&format!(
                "  ... (more under {}/ not shown — use list_directory or glob)\n",
                dir.name
            ));
        }
    }
    for file in top_files {
        out.push_str(&file.name);
        out.push('\n');
    }

    if any_elided {
        out.push_str(&format!(
            "\n(tree capped at ~{} entries, depth {}; directories marked \"more ... not shown\" \
             are incomplete — never assume a file is absent because it isn't listed here)\n",
            max_entries, max_depth
        ));
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(path: &Path) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, b"x").unwrap();
    }

    #[test]
    fn rustic_dir_is_visible() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join(".rustic/memory/MEMORY.md"));
        touch(&root.join("src/lib.rs"));

        let tree = generate_file_tree(root, false);
        assert!(
            tree.contains(".rustic/"),
            ".rustic must be listed — the agent writes to it constantly. Got:\n{}",
            tree
        );
        assert!(
            tree.contains("MEMORY.md"),
            ".rustic contents must be walked, not just the dir name. Got:\n{}",
            tree
        );
    }

    #[test]
    fn excluded_dirs_stay_hidden() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join("node_modules/pkg/index.js"));
        touch(&root.join("target/debug/build.rs"));
        touch(&root.join("src/main.rs"));

        let tree = generate_file_tree(root, false);
        assert!(!tree.contains("index.js"), "node_modules must stay hidden");
        assert!(!tree.contains("debug/"), "target must stay hidden");
        assert!(tree.contains("main.rs"));
    }

    #[test]
    fn large_subtree_cannot_starve_siblings() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // `references` sorts before `src` / `src-tauri` and is huge — under the
        // old first-come-first-served cap it consumed the whole budget.
        for i in 0..400 {
            touch(&root.join(format!("references/vendor/file_{:03}.txt", i)));
        }
        touch(&root.join("src/main.js"));
        touch(&root.join("src-tauri/lib.rs"));
        touch(&root.join("rustic-server/server.rs"));

        let tree = generate_file_tree_with_limits(root, false, 5, 100);

        assert!(
            tree.contains("main.js"),
            "src/ must survive a huge sibling. Got:\n{}",
            tree
        );
        assert!(
            tree.contains("lib.rs"),
            "src-tauri/ must survive a huge sibling. Got:\n{}",
            tree
        );
        assert!(
            tree.contains("server.rs"),
            "rustic-server/ must survive a huge sibling. Got:\n{}",
            tree
        );
        assert!(
            tree.contains("more under references/ not shown"),
            "the truncated subtree must say so out loud. Got:\n{}",
            tree
        );
    }

    #[test]
    fn fingerprint_tracks_content() {
        assert_eq!(
            tree_fingerprint("src/\n  a.rs\n"),
            tree_fingerprint("src/\n  a.rs\n")
        );
        assert_ne!(
            tree_fingerprint("src/\n  a.rs\n"),
            tree_fingerprint("src/\n  b.rs\n")
        );
    }
}

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
use std::path::Path;

/// Directories that are always excluded regardless of `.gitignore`.
const EXCLUDED_DIRS: &[&str] = &[
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
    ".rustic",
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

/// One node in the collected tree.
struct TreeNode {
    /// Display name (file or directory name).
    name: String,
    /// Whether this entry is a directory.
    is_dir: bool,
    /// Depth relative to root (0 = direct child of project root).
    depth: usize,
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
            | "apply_patch"
            | "run_command"
            | "spawn_subagent"
            | "edit_notebook"
            | "image_create"
            | "video_create"
            | "animate"
    )
}

fn generate_tree_inner(
    project_root: &Path,
    include_gitignored: bool,
    max_depth: usize,
    max_entries: usize,
) -> String {
    let excluded: HashSet<&str> = EXCLUDED_DIRS.iter().copied().collect();

    // Respect .gitignore unless the caller has opted into showing everything
    // (FullAuto mode).
    let respect_gitignore = !include_gitignored;
    let walker = ignore::WalkBuilder::new(project_root)
        .hidden(false) // don't skip dotfiles by default (we handle exclusions ourselves)
        .git_ignore(respect_gitignore)
        .git_global(respect_gitignore)
        .git_exclude(respect_gitignore)
        .max_depth(Some(max_depth + 1)) // +1 because depth 0 is the root itself
        .filter_entry(move |entry| {
            let name = entry.file_name().to_string_lossy();
            // Always allow the root entry itself.
            if entry.depth() == 0 {
                return true;
            }
            // Skip excluded names.
            !excluded.contains(name.as_ref())
        })
        .sort_by_file_path(|a, b| {
            // Directories first, then alphabetical.
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
        .build();

    let mut nodes: Vec<TreeNode> = Vec::new();

    for result in walker {
        let entry = match result {
            Ok(e) => e,
            Err(_) => continue,
        };
        // Skip the root directory itself.
        if entry.depth() == 0 {
            continue;
        }
        if nodes.len() >= max_entries {
            break;
        }

        let depth = entry.depth() - 1; // make direct children depth 0
        let is_dir = entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
        let name = entry.file_name().to_string_lossy().to_string();
        nodes.push(TreeNode {
            name,
            is_dir,
            depth,
        });
    }

    let truncated = nodes.len() >= max_entries;

    // Build the tree string.
    let mut out = String::with_capacity(nodes.len() * 40);
    for node in &nodes {
        // Indent: 2 spaces per depth level.
        for _ in 0..node.depth {
            out.push_str("  ");
        }
        out.push_str(&node.name);
        if node.is_dir {
            out.push('/');
        }
        out.push('\n');
    }

    if truncated {
        out.push_str(&format!(
            "\n... (truncated at {} entries — use read_file or list_directory for a deeper view)\n",
            max_entries
        ));
    }

    out
}

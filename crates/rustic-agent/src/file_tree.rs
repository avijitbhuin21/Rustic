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

/// File names that must never appear in the tree, regardless of the
/// gitignore-bypass toggle: even the *name* tells the model where a secret
/// lives. Mirrors the tier-1 rules of `file_ops::check_sensitive_path`.
pub fn is_sensitive_file_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    if n.starts_with(".updater-signing.key") {
        return true;
    }
    if n == ".env" || n.starts_with(".env.") {
        return !matches!(
            n.as_str(),
            ".env.example" | ".env.sample" | ".env.template" | ".env.dist"
        );
    }
    if ["id_rsa", "id_ed25519", "id_ecdsa", "id_dsa"]
        .iter()
        .any(|k| n.starts_with(k))
    {
        return true;
    }
    if [".pem", ".p12", ".pfx", ".keystore", ".jks"]
        .iter()
        .any(|ext| n.ends_with(ext))
    {
        return true;
    }
    if n.ends_with(".key") && !n.contains("map") && !n.contains("board") {
        return true;
    }
    matches!(
        n.as_str(),
        "credentials" | "credentials.json" | ".netrc" | ".pgpass"
    ) || (n.starts_with("service-account") && n.ends_with(".json"))
}

/// Binary-asset extensions: a directory made only of these carries no
/// navigational value file-by-file, so it renders as one summary line.
const ASSET_EXTS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "ico", "icns", "bmp", "tif", "tiff", "svg", "avif",
    "woff", "woff2", "ttf", "otf", "eot", "mp3", "wav", "ogg", "flac", "m4a", "mp4", "webm",
    "mov", "pdf", "zip", "gz", "bin", "wasm",
];
/// Companion files tolerated (at most one) inside an asset directory, e.g. a
/// font LICENSE.txt or an Android icon XML.
const COMPANION_EXTS: &[&str] = &["txt", "md", "json", "xml", "plist", ""];
/// Data extensions that make a homogeneous directory a fixture dump when it
/// sits under a test-ish directory.
const FIXTURE_EXTS: &[&str] = &[
    "json", "yaml", "yml", "txt", "snap", "xml", "csv", "toml", "html", "md",
];
const FIXTURE_DIR_NAMES: &[&str] = &[
    "tests",
    "test",
    "fixtures",
    "fixture",
    "__fixtures__",
    "__snapshots__",
    "snapshots",
    "testdata",
    "suite",
];
/// Directories smaller than this are always listed file-by-file.
const COLLAPSE_MIN_FILES: usize = 6;

fn ext_of(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => ext.to_ascii_lowercase(),
        _ => String::new(),
    }
}

/// Decide whether `dir` renders as a single summary line instead of its
/// files. Returns the parenthesised summary (without parens) when it does.
/// `root` is the project root, used so fixture detection only looks at
/// path components inside the project.
fn collapse_summary(root: &Path, dir: &Path) -> Option<String> {
    let rd = std::fs::read_dir(dir).ok()?;
    let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
    let mut files = 0usize;
    let mut has_subdir = false;
    for entry in rd.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        let name = entry.file_name().to_string_lossy().to_string();
        if ft.is_dir() {
            if !EXCLUDED_DIRS.contains(&name.as_str()) {
                has_subdir = true;
            }
            continue;
        }
        if EXCLUDED_DIRS.contains(&name.as_str()) || is_sensitive_file_name(&name) {
            continue;
        }
        files += 1;
        *counts.entry(ext_of(&name)).or_default() += 1;
    }
    if has_subdir || files < COLLAPSE_MIN_FILES {
        return None;
    }

    let dir_name = dir.file_name().map(|n| n.to_string_lossy().to_string());
    let parent_is_rustic = dir
        .parent()
        .and_then(|p| p.file_name())
        .map(|n| n == ".rustic")
        .unwrap_or(false);
    if parent_is_rustic && dir_name.as_deref() == Some("memory") {
        let md = counts.get("md").copied().unwrap_or(0);
        if md > 0 {
            return Some(format!(
                "{} .md notes — index preloaded as MEMORY.md; read a note by name",
                md
            ));
        }
    }

    let assets: usize = counts
        .iter()
        .filter(|(ext, _)| ASSET_EXTS.contains(&ext.as_str()))
        .map(|(_, c)| *c)
        .sum();
    let non_assets = files - assets;
    let companions_ok = counts
        .keys()
        .filter(|ext| !ASSET_EXTS.contains(&ext.as_str()))
        .all(|ext| COMPANION_EXTS.contains(&ext.as_str()));
    let asset_dir = assets >= COLLAPSE_MIN_FILES && non_assets <= 1 && companions_ok;

    let in_test_dir = dir
        .strip_prefix(root)
        .map(|rel| {
            rel.components().any(|c| {
                let s = c.as_os_str().to_string_lossy().to_ascii_lowercase();
                FIXTURE_DIR_NAMES.contains(&s.as_str())
            })
        })
        .unwrap_or(false);
    let fixture_dir = in_test_dir
        && counts.len() == 1
        && counts
            .keys()
            .next()
            .map(|e| FIXTURE_EXTS.contains(&e.as_str()))
            .unwrap_or(false);

    if !(asset_dir || fixture_dir) {
        return None;
    }
    let mut parts: Vec<(&String, &usize)> = counts.iter().collect();
    parts.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
    let rendered: Vec<String> = parts
        .into_iter()
        .map(|(ext, c)| {
            if ext.is_empty() {
                format!("{} other", c)
            } else {
                format!("{} {}", c, ext)
            }
        })
        .collect();
    Some(rendered.join(", "))
}

/// Per-walk memo of [`collapse_summary`] keyed by directory, shared between
/// the walker's entry filter (hides a collapsed dir's children) and the
/// renderer (prints the summary next to the dir name).
type CollapseCache = std::sync::Arc<
    std::sync::Mutex<std::collections::HashMap<std::path::PathBuf, Option<String>>>,
>;

fn cached_collapse(cache: &CollapseCache, root: &Path, dir: &Path) -> Option<String> {
    let mut guard = match cache.lock() {
        Ok(g) => g,
        Err(e) => e.into_inner(),
    };
    guard
        .entry(dir.to_path_buf())
        .or_insert_with(|| collapse_summary(root, dir))
        .clone()
}

fn walker_for(
    root: &Path,
    include_gitignored: bool,
    max_depth: usize,
    excluded: HashSet<&'static str>,
    collapse: Option<(std::path::PathBuf, CollapseCache)>,
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
            let name = entry.file_name().to_string_lossy();
            if excluded.contains(name.as_ref()) {
                return false;
            }
            let is_dir = entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
            if !is_dir && is_sensitive_file_name(&name) {
                return false;
            }
            if let (Some((project_root, cache)), Some(parent)) =
                (collapse.as_ref(), entry.path().parent())
            {
                if cached_collapse(cache, project_root, parent).is_some() {
                    return false;
                }
            }
            true
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
    for result in walker_for(root, include_gitignored, 1, excluded_set(), None) {
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
    project_root: &Path,
    dir: &Path,
    include_gitignored: bool,
    max_depth: usize,
    cap: usize,
    cache: &CollapseCache,
) -> SubtreeResult {
    let mut lines = Vec::new();
    let mut hit_cap = false;
    let collapse = Some((project_root.to_path_buf(), cache.clone()));
    for result in walker_for(dir, include_gitignored, max_depth, excluded_set(), collapse) {
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
            if let Some(summary) = cached_collapse(cache, project_root, entry.path()) {
                line.push_str(&format!(" ({})", summary));
            }
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
    let cache: CollapseCache = Default::default();

    // With the gitignore bypass on, top-level dirs that git ignores (vendored
    // references, local data dirs) are listed by name only so the budget goes
    // to real source. `.rustic/` is exempt — the agent works in it constantly.
    let gitignored_dirs: HashSet<String> = if include_gitignored {
        let tracked: HashSet<String> = list_top_level(project_root, false)
            .into_iter()
            .filter(|e| e.is_dir)
            .map(|e| e.name)
            .collect();
        top.iter()
            .filter(|e| e.is_dir && e.name != ".rustic" && !tracked.contains(&e.name))
            .map(|e| e.name.clone())
            .collect()
    } else {
        HashSet::new()
    };
    let summary_for = |name: &str| -> Option<String> {
        if gitignored_dirs.contains(name) {
            return Some("gitignored — contents not expanded; use list_directory or glob".into());
        }
        cached_collapse(&cache, project_root, &project_root.join(name))
    };

    let top_dirs: Vec<&TopEntry> = top.iter().filter(|e| e.is_dir).collect();
    let top_files: Vec<&TopEntry> = top.iter().filter(|e| !e.is_dir).collect();
    let top_summaries: Vec<Option<String>> = top_dirs.iter().map(|d| summary_for(&d.name)).collect();
    let walk_count = top_summaries.iter().filter(|s| s.is_none()).count();

    let mut remaining = max_entries.saturating_sub(top.len());
    let base_cap = if walk_count == 0 {
        0
    } else {
        (remaining / walk_count).max(MIN_SUBTREE_ENTRIES)
    };

    let mut results: Vec<SubtreeResult> = Vec::with_capacity(top_dirs.len());
    for (i, dir) in top_dirs.iter().enumerate() {
        if top_summaries[i].is_some() {
            results.push(SubtreeResult {
                lines: Vec::new(),
                count: 0,
                hit_cap: false,
            });
            continue;
        }
        let cap = base_cap.min(remaining);
        let result = walk_subtree(
            project_root,
            &project_root.join(&dir.name),
            include_gitignored,
            max_depth,
            cap,
            &cache,
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
                    project_root,
                    &project_root.join(&top_dirs[i].name),
                    include_gitignored,
                    max_depth,
                    results[i].count + extra,
                    &cache,
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
        out.push('/');
        if let Some(summary) = &top_summaries[i] {
            out.push_str(&format!(" ({})", summary));
        }
        out.push('\n');
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
        if is_sensitive_file_name(&file.name) {
            continue;
        }
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

    #[test]
    fn asset_and_fixture_dirs_collapse_to_one_line() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for i in 0..10 {
            touch(&root.join(format!("app/icons/icon{}.png", i)));
        }
        touch(&root.join("app/icons/icon.icns"));
        for i in 0..7 {
            touch(&root.join(format!("app/tests/suite/case_{}.json", i)));
        }
        for i in 0..7 {
            touch(&root.join(format!("app/src/mod_{}.json", i)));
        }
        touch(&root.join("app/src/main.rs"));

        let tree = generate_file_tree(root, false);
        assert!(tree.contains("icons/ (10 png, 1 icns)"), "Got:\n{}", tree);
        assert!(!tree.contains("icon3.png"), "Got:\n{}", tree);
        assert!(tree.contains("suite/ (7 json)"), "Got:\n{}", tree);
        assert!(!tree.contains("case_1.json"), "Got:\n{}", tree);
        // Source dirs (not under a test dir, mixed content) stay expanded.
        assert!(tree.contains("mod_1.json") && tree.contains("main.rs"), "Got:\n{}", tree);
    }

    #[test]
    fn memory_dir_collapses_once_large() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join(".rustic/memory/MEMORY.md"));
        for i in 0..8 {
            touch(&root.join(format!(".rustic/memory/note-{}.md", i)));
        }
        let tree = generate_file_tree(root, false);
        assert!(
            tree.contains("memory/ (9 .md notes — index preloaded as MEMORY.md"),
            "Got:\n{}",
            tree
        );
        assert!(!tree.contains("note-3.md"), "Got:\n{}", tree);
    }

    #[test]
    fn sensitive_files_hidden_even_with_gitignore_bypass() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join(".updater-signing.key"));
        touch(&root.join(".updater-signing.key.pub"));
        touch(&root.join(".env.local"));
        touch(&root.join(".env.example"));
        touch(&root.join("certs/server.pem"));
        touch(&root.join("certs/README.md"));
        touch(&root.join("deploy/id_rsa"));
        touch(&root.join("deploy/keymap.key"));
        touch(&root.join("src/main.rs"));

        for bypass in [false, true] {
            let tree = generate_file_tree(root, bypass);
            for secret in [".updater-signing.key", ".env.local", "server.pem", "id_rsa"] {
                assert!(!tree.contains(secret), "{} leaked (bypass={}):\n{}", secret, bypass, tree);
            }
            assert!(tree.contains(".env.example"), "Got:\n{}", tree);
            assert!(tree.contains("keymap.key"), "Got:\n{}", tree);
            assert!(tree.contains("README.md"), "Got:\n{}", tree);
        }
    }

    #[test]
    fn gitignored_top_level_dirs_collapse_when_bypassed() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join(".gitignore"), "references/\n.rustic/\n").unwrap();
        touch(&root.join("references/big/a.ts"));
        touch(&root.join(".rustic/memory/MEMORY.md"));
        touch(&root.join("src/main.rs"));

        let hidden = generate_file_tree(root, false);
        assert!(!hidden.contains("references"), "Got:\n{}", hidden);

        let tree = generate_file_tree(root, true);
        assert!(
            tree.contains("references/ (gitignored — contents not expanded"),
            "Got:\n{}",
            tree
        );
        assert!(!tree.contains("a.ts"), "Got:\n{}", tree);
        // .rustic is exempt from the gitignored collapse.
        assert!(tree.contains("MEMORY.md"), "Got:\n{}", tree);
        assert!(tree.contains("main.rs"));
    }
}

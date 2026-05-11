//! Gitignore-filtered worktree walker for the changed-files sweep.
//!
//! Used by the bash sweep worker to discover candidate files without paying
//! `git add -A` cost. Two-stage filter: the `ignore` crate handles `.gitignore`,
//! `.git/info/exclude`, global excludes, and hidden-file rules; we layer a
//! hard denylist on top for well-known noise that nobody bothers to gitignore
//! (e.g. `node_modules` in projects without a `.gitignore` at all).
//!
//! The walker is stat-only (no content reads). Callers feed the resulting
//! `(path, mtime, size)` tuples into the mtime filter, then hash only what
//! actually changed.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use ignore::WalkBuilder;

/// Directory names that are skipped even if not gitignored. Matches the list
/// from the timing-spike example. Tunable; intentionally short so we don't
/// surprise users by silently ignoring real directories.
pub const HARD_DENY_DIRS: &[&str] = &[
    "node_modules",
    "target",
    "dist",
    "build",
    ".git",
    ".next",
    ".turbo",
    ".cache",
    "__pycache__",
    ".venv",
    "venv",
    ".idea",
    ".vscode",
];

#[derive(Debug, Clone)]
pub struct WalkedFile {
    /// Absolute path on disk.
    pub abs_path: PathBuf,
    /// Path relative to the walk root, with forward slashes — this is the
    /// canonical form stored in the snapshot index across platforms.
    pub rel_path: String,
    pub size: u64,
    pub mtime: SystemTime,
}

/// Walk `root` returning every file the sweep should consider, with stat
/// metadata captured during the walk. Errors during enumeration of individual
/// entries are swallowed (logged via `tracing`) — one unreadable file
/// shouldn't tank the whole sweep.
pub fn walk_for_sweep(root: &Path) -> Vec<WalkedFile> {
    let mut out = Vec::new();
    let walker = WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(true)
        .ignore(true)
        .parents(true)
        .filter_entry(|entry| {
            if entry.file_type().is_some_and(|ft| ft.is_dir()) {
                if let Some(name) = entry.file_name().to_str() {
                    if HARD_DENY_DIRS.iter().any(|d| *d == name) {
                        return false;
                    }
                }
            }
            true
        })
        .build();

    for result in walker {
        let entry = match result {
            Ok(e) => e,
            Err(e) => {
                tracing::debug!(?e, "walk entry error");
                continue;
            }
        };
        let Some(ft) = entry.file_type() else { continue };
        if !ft.is_file() {
            continue;
        }
        let abs_path = entry.path().to_path_buf();
        let metadata = match entry.metadata() {
            Ok(m) => m,
            Err(e) => {
                tracing::debug!(path = %abs_path.display(), ?e, "metadata error");
                continue;
            }
        };
        let mtime = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        let rel_path = match abs_path.strip_prefix(root) {
            Ok(rel) => normalize_rel(rel),
            Err(_) => continue, // outside root somehow; skip
        };
        out.push(WalkedFile {
            abs_path,
            rel_path,
            size: metadata.len(),
            mtime,
        });
    }
    out
}

/// Render a relative path into the canonical snapshot key form: forward
/// slashes, no leading `./`. On Windows this collapses backslashes; on Unix
/// it's a no-op for typical paths.
pub fn normalize_rel(rel: &Path) -> String {
    let mut s = String::new();
    let mut first = true;
    for comp in rel.components() {
        match comp {
            std::path::Component::Normal(part) => {
                if !first {
                    s.push('/');
                }
                first = false;
                s.push_str(&part.to_string_lossy());
            }
            std::path::Component::CurDir => continue,
            // ParentDir / RootDir / Prefix shouldn't appear in a strip_prefix
            // result; if they do, fall back to lossy representation.
            other => {
                if !first {
                    s.push('/');
                }
                first = false;
                s.push_str(&other.as_os_str().to_string_lossy());
            }
        }
    }
    s
}

/// Resolve a relative snapshot path back to an absolute path under `root`.
/// Inverse of `normalize_rel` — accepts forward slashes and joins them onto
/// the root using the platform-native separator.
pub fn join_rel(root: &Path, rel: &str) -> PathBuf {
    let mut buf = root.to_path_buf();
    for part in rel.split('/').filter(|p| !p.is_empty()) {
        buf.push(part);
    }
    buf
}

/// Filter `walked` to entries whose `mtime` is at or after `since`. This is
/// the candidate set that needs hashing.
pub fn changed_since(walked: Vec<WalkedFile>, since: SystemTime) -> Vec<WalkedFile> {
    walked.into_iter().filter(|w| w.mtime >= since).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;
    use std::time::{Duration, SystemTime};

    fn write_file(p: &Path, content: &[u8]) {
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let mut f = fs::File::create(p).unwrap();
        f.write_all(content).unwrap();
    }

    #[test]
    fn walk_skips_hard_denied_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write_file(&root.join("src/main.rs"), b"fn main() {}");
        write_file(&root.join("node_modules/lodash/index.js"), b"// noise");
        write_file(&root.join("target/debug/build_artifact"), b"binary");
        write_file(&root.join(".git/HEAD"), b"ref: refs/heads/main");
        write_file(&root.join("docs/readme.md"), b"# hi");

        let walked = walk_for_sweep(root);
        let rels: Vec<_> = walked.iter().map(|w| w.rel_path.clone()).collect();
        assert!(rels.contains(&"src/main.rs".to_string()));
        assert!(rels.contains(&"docs/readme.md".to_string()));
        assert!(!rels.iter().any(|r| r.starts_with("node_modules/")));
        assert!(!rels.iter().any(|r| r.starts_with("target/")));
        assert!(!rels.iter().any(|r| r.starts_with(".git/")));
    }

    #[test]
    fn walk_respects_gitignore() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // The `ignore` crate's `git_ignore(true)` only applies .gitignore rules
        // when there's a containing git repo. In production this is always
        // true for user worktrees, but tests need an explicit .git/ marker.
        // (HARD_DENY_DIRS skips .git/ contents from the listing, but its
        // *presence* is what enables .gitignore parsing.)
        fs::create_dir_all(root.join(".git")).unwrap();
        write_file(&root.join(".gitignore"), b"secret.txt\nbuild_out/\n");
        write_file(&root.join("ok.txt"), b"ok");
        write_file(&root.join("secret.txt"), b"hidden");
        write_file(&root.join("build_out/x"), b"x");

        let walked = walk_for_sweep(root);
        let rels: Vec<_> = walked.iter().map(|w| w.rel_path.clone()).collect();
        assert!(rels.contains(&"ok.txt".to_string()));
        assert!(rels.contains(&".gitignore".to_string()));
        assert!(!rels.iter().any(|r| r == "secret.txt"));
        assert!(!rels.iter().any(|r| r.starts_with("build_out/")));
    }

    #[test]
    fn changed_since_filters_by_mtime() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write_file(&root.join("a"), b"1");
        write_file(&root.join("b"), b"2");

        // Walk once to capture timestamps, then filter against an old cutoff.
        let walked = walk_for_sweep(root);
        let old_cutoff = SystemTime::UNIX_EPOCH;
        let after_old = changed_since(walked.clone(), old_cutoff);
        assert_eq!(after_old.len(), walked.len());

        // Cutoff in the future drops everything.
        let future_cutoff = SystemTime::now() + Duration::from_secs(3600);
        let after_future = changed_since(walked, future_cutoff);
        assert!(after_future.is_empty());
    }

    #[test]
    fn normalize_and_join_roundtrip() {
        let rel = "src/sub/file.rs";
        let joined = join_rel(Path::new("/tmp/root"), rel);
        let renormed = normalize_rel(joined.strip_prefix("/tmp/root").unwrap());
        assert_eq!(renormed, rel);
    }

    #[test]
    fn normalize_strips_leading_curdir() {
        let p = Path::new("./src/main.rs");
        assert_eq!(normalize_rel(p), "src/main.rs");
    }
}

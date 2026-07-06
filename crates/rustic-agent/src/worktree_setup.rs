//! Post-create setup shared by every fresh git worktree — task isolation
//! worktrees (rustic-app) and isolated sub-agent worktrees (subagent_tools)
//! run the same steps: `.env*` allowlist copy, `.worktreeinclude` manifest
//! copy, configured directory links, and husky hooksPath pinning.

use std::path::Path;
use std::sync::{Arc, Mutex};

use rustic_db::Database;
use rustic_git::GitRepo;

/// Settings key holding directories to link from the main checkout into new
/// worktrees (JSON array of repo-relative paths, e.g. `["node_modules"]`).
/// Nothing is linked by default — explicit opt-in, mirroring Claude Code's
/// `worktree.symlinkDirectories`.
pub const SYMLINK_DIRS_KEY: &str = "worktree_symlink_directories";

/// Run every post-create step on a fresh worktree. `db` gates the
/// settings-driven directory links — pass `None` when no database handle is
/// available (links are skipped). Best-effort throughout: failures log and
/// skip, never fail worktree creation.
pub fn post_create_setup(db: Option<&Arc<Mutex<Database>>>, project_root: &Path, wt_path: &Path) {
    copy_env_allowlist(project_root, wt_path);
    copy_worktreeinclude_files(project_root, wt_path);
    if let Some(db) = db {
        link_configured_dirs(db, project_root, wt_path);
    }
    propagate_hooks_path(project_root, wt_path);
}

/// Copy top-level gitignored env files (`.env*`) from the main checkout into
/// a fresh worktree so dev servers keep working.
fn copy_env_allowlist(project_root: &Path, wt_path: &Path) {
    let Ok(entries) = std::fs::read_dir(project_root) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with(".env") {
            continue;
        }
        let src = entry.path();
        if !src.is_file() {
            continue;
        }
        let dst = wt_path.join(&name);
        if !dst.exists() {
            let _ = std::fs::copy(&src, &dst);
        }
    }
}

/// Copy gitignored files matched by a `.worktreeinclude` manifest (gitignore
/// syntax) from the main checkout into a fresh worktree. Only files that are
/// BOTH gitignored and matched by a pattern are copied — tracked files are
/// already checked out. Mirrors Claude Code's `.worktreeinclude` convention:
/// `git ls-files -oi --directory` collapses fully-ignored dirs to one entry,
/// and a collapsed dir is only expanded (second scoped `ls-files`) when a
/// pattern explicitly reaches into it, so `node_modules/` never forces a
/// full tree walk. Best-effort: failures log and skip, never fail creation.
fn copy_worktreeinclude_files(project_root: &Path, wt_path: &Path) {
    let Ok(content) = std::fs::read_to_string(project_root.join(".worktreeinclude")) else {
        return;
    };
    let patterns: Vec<&str> = content
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect();
    if patterns.is_empty() {
        return;
    }

    let mut builder = ignore::gitignore::GitignoreBuilder::new(project_root);
    for p in &patterns {
        let _ = builder.add_line(None, p);
    }
    let Ok(matcher) = builder.build() else {
        return;
    };
    let Ok(repo) = GitRepo::open(project_root) else {
        return;
    };
    let Ok(entries) = repo.list_ignored(&[]) else {
        return;
    };

    let mut files: Vec<String> = Vec::new();
    let mut collapsed_dirs: Vec<String> = Vec::new();
    for e in entries {
        if e.ends_with('/') {
            collapsed_dirs.push(e);
        } else if matcher.matched(&e, false).is_ignore() {
            files.push(e);
        }
    }

    let dirs_to_expand: Vec<&str> = collapsed_dirs
        .iter()
        .filter(|dir| {
            if matcher.matched(dir.trim_end_matches('/'), true).is_ignore() {
                return true;
            }
            patterns.iter().any(|p| {
                let normalized = p.strip_prefix('/').unwrap_or(p);
                if normalized.starts_with(dir.as_str()) {
                    return true;
                }
                match normalized.find(['*', '?', '[']) {
                    Some(idx) if idx > 0 => dir.starts_with(&normalized[..idx]),
                    _ => false,
                }
            })
        })
        .map(|d| d.as_str())
        .collect();
    if !dirs_to_expand.is_empty() {
        if let Ok(expanded) = repo.list_ignored(&dirs_to_expand) {
            for e in expanded {
                if !e.ends_with('/') && matcher.matched(&e, false).is_ignore() {
                    files.push(e);
                }
            }
        }
    }

    let mut copied = 0usize;
    for rel in &files {
        let src = project_root.join(rel);
        let dst = wt_path.join(rel);
        if dst.exists() || !src.is_file() {
            continue;
        }
        if let Some(parent) = dst.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::copy(&src, &dst) {
            Ok(_) => copied += 1,
            Err(e) => tracing::warn!(%rel, %e, "worktreeinclude: copy failed"),
        }
    }
    if copied > 0 {
        tracing::info!(
            copied,
            "worktreeinclude: copied gitignored files into worktree"
        );
    }
}

/// Link directories listed in the `worktree_symlink_directories` setting from
/// the main checkout into a fresh worktree (dependency dirs like
/// `node_modules`, `target`, `.venv`) to avoid re-installing per task.
/// Opt-in and empty by default. Rejects absolute paths and `..` traversal;
/// missing sources and existing destinations are skipped silently.
fn link_configured_dirs(db: &Arc<Mutex<Database>>, project_root: &Path, wt_path: &Path) {
    let dirs: Vec<String> = {
        let guard = match db.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        guard
            .get_setting(SYMLINK_DIRS_KEY)
            .ok()
            .flatten()
            .and_then(|v| serde_json::from_str(&v).ok())
            .unwrap_or_default()
    };
    for dir in dirs {
        let rel = Path::new(&dir);
        let traverses = rel
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir));
        if rel.is_absolute() || traverses {
            tracing::warn!(%dir, "worktree symlink dir rejected: path traversal");
            continue;
        }
        let src = project_root.join(rel);
        let dst = wt_path.join(rel);
        if !src.is_dir() || dst.exists() {
            continue;
        }
        if let Some(parent) = dst.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = link_dir(&src, &dst) {
            tracing::warn!(%dir, %e, "worktree symlink dir failed");
        }
    }
}

/// Create a directory link from `dst` to `src`: a symlink on unix, an NTFS
/// junction on Windows (junctions need no admin rights or developer mode).
#[cfg(unix)]
fn link_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(src, dst)
}

#[cfg(windows)]
fn link_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let out = std::process::Command::new("cmd")
        .arg("/C")
        .arg("mklink")
        .arg("/J")
        .arg(dst)
        .arg(src)
        .creation_flags(CREATE_NO_WINDOW)
        .output()?;
    if out.status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ))
    }
}

/// Husky-style repos set `core.hooksPath = .husky` (a RELATIVE path) in the
/// shared repo config; from a worktree git resolves it against the worktree
/// root, where dev dependencies may be missing, so every `git commit` the
/// agent or terminal runs there fails. Pin the absolute main-checkout path
/// instead (mirrors Claude Code). Default `.git/hooks` needs no fix — linked
/// worktrees already share the main repo's hooks dir.
fn propagate_hooks_path(project_root: &Path, wt_path: &Path) {
    let husky = project_root.join(".husky");
    if !husky.is_dir() {
        return;
    }
    let Ok(wt) = GitRepo::open(wt_path) else {
        return;
    };
    let desired = husky.to_string_lossy().into_owned();
    if wt.config_get("core.hooksPath").as_deref() == Some(desired.as_str()) {
        return;
    }
    if let Err(e) = wt.config_set("core.hooksPath", &desired) {
        tracing::warn!(%e, "worktree: failed to pin core.hooksPath");
    }
}

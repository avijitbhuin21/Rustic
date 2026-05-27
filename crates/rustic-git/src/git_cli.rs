//! Thin subprocess wrapper around the `git` CLI for operations gix doesn't yet
//! implement (merge, rebase, worktree creation) plus state-mutating ops where
//! gix's surface is significantly lower-level than libgit2's. See
//! [docs/educated-guesses/003-rustic-git-merge-rebase-strategy.md](../../docs/educated-guesses/003-rustic-git-merge-rebase-strategy.md)
//! and 006 for the rationale.
//!
//! The fallback assumes `git` is on PATH. When it isn't, the error returned
//! is a clear, actionable message — see `GIT_NOT_FOUND_MESSAGE` — so the UI
//! can surface "install git" guidance to the user instead of a cryptic OS
//! error.

use anyhow::Result;
use std::io;
use std::path::Path;
use std::process::Command;

/// Stable, user-facing error string used whenever the `git` binary can't be
/// found on PATH. The Tauri frontend matches against this prefix to decide
/// whether to render the "install git" guidance vs a generic git-command-
/// failed toast. Keep the wording stable across releases.
pub const GIT_NOT_FOUND_MESSAGE: &str =
    "Git is not installed (or not on PATH). \
     Please install Git from https://git-scm.com/downloads and make sure \
     the `git` command is available, then restart Rustic.";

/// One-shot check that callers (e.g. the host on startup) can use to detect
/// missing git up front rather than waiting for the first VCS action to fail.
/// Cheap — `git --version` is sub-50ms on every supported platform.
pub fn is_git_available() -> bool {
    let mut cmd = Command::new("git");
    cmd.arg("--version");

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    cmd.output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

/// Map a `std::io::Error` from `Command::output()` into an actionable
/// anyhow::Error. The most common — and the one users hit when git isn't
/// installed — is `ErrorKind::NotFound`; we return [`GIT_NOT_FOUND_MESSAGE`]
/// verbatim in that case so the frontend can pattern-match on it.
pub(crate) fn spawn_error(e: io::Error) -> anyhow::Error {
    if e.kind() == io::ErrorKind::NotFound {
        anyhow::anyhow!("{}", GIT_NOT_FOUND_MESSAGE)
    } else {
        anyhow::Error::new(e).context("failed to spawn `git`")
    }
}

/// Run `git <args>` inside `repo_path` and capture stdout. Returns Err on
/// non-zero exit (with stderr in the message) or when `git` isn't on PATH
/// (with the `GIT_NOT_FOUND_MESSAGE` for the UI to pattern-match).
pub(crate) fn run(repo_path: &Path, args: &[&str]) -> Result<String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(repo_path)
        .args(args);

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let output = cmd.output().map_err(spawn_error)?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "git {} failed (exit {}): {}",
            args.join(" "),
            output.status.code().unwrap_or(-1),
            stderr.trim()
        );
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Variant that discards stdout — for commands run for their side effects.
pub(crate) fn run_silent(repo_path: &Path, args: &[&str]) -> Result<()> {
    run(repo_path, args).map(|_| ())
}

/// Returns the subset of `paths` that `git add` would reject. We use
/// `git add --dry-run` per path rather than `git check-ignore` because the
/// two diverge for already-tracked files whose parent directory matches an
/// ignore rule: `check-ignore` reports "not ignored" (it skips tracked
/// paths), but `git add` still aborts the entire batch with "paths are
/// ignored by .gitignore" — which is exactly the failure mode we're trying
/// to filter out. The dry-run mirrors the real `git add` behaviour exactly.
///
/// N forks for N paths; fine for typical change-sets and only invoked when
/// the user actually stages.
pub(crate) fn rejected_by_add(repo_path: &Path, paths: &[&str]) -> Result<Vec<String>> {
    let mut rejected = Vec::new();
    for p in paths {
        let mut cmd = Command::new("git");
        cmd.arg("-C")
            .arg(repo_path)
            .args(["add", "--dry-run", "--"])
            .arg(p);

        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let output = cmd.output().map_err(spawn_error)?;
        if !output.status.success() {
            // Any non-zero from `git add --dry-run` means the real add would
            // also fail. We treat all such paths uniformly as "rejected" —
            // the most common cause is .gitignore matches, but missing-path
            // / permission errors land here too and should also be skipped
            // (otherwise the batch would fail for the same reason).
            rejected.push((*p).to_string());
        }
    }
    Ok(rejected)
}


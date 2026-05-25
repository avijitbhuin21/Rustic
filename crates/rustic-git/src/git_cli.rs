//! Thin subprocess wrapper around the `git` CLI for operations gix doesn't yet
//! implement (merge, rebase, worktree creation). See
//! [docs/educated-guesses/003-rustic-git-merge-rebase-strategy.md](../../docs/educated-guesses/003-rustic-git-merge-rebase-strategy.md)
//! for the rationale.
//!
//! The fallback assumes `git` is on PATH. The editor wouldn't be very useful
//! without a working git anyway, but the error message surfaces that
//! requirement clearly when it's missing.

use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

/// Run `git <args>` inside `repo_path` and capture stdout. Returns Err on
/// non-zero exit, with stderr in the message.
pub(crate) fn run(repo_path: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .args(args)
        .output()
        .with_context(|| {
            "failed to spawn `git` — make sure git is installed and on PATH"
        })?;

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

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

/// Run `git <args>` with `stdin_data` piped to stdin, capturing the full
/// Output (status + stdout + stderr) WITHOUT failing on non-zero exit — some
/// callers (dry-run probing) need to inspect the failure. `LC_ALL=C` forces
/// untranslated messages so callers can parse stderr reliably.
///
/// stdin is written from a dedicated thread and stderr drained from another,
/// so a child that fills one pipe while we're busy with a different one can
/// never deadlock (the default `wait_with_output` reads the pipes
/// sequentially, which deadlocks once stderr exceeds the 64 KB pipe buffer —
/// easy to hit when a dry-run rejects thousands of ignored paths).
pub(crate) fn run_with_stdin(
    repo_path: &Path,
    args: &[&str],
    stdin_data: &str,
) -> Result<std::process::Output> {
    use std::io::{Read, Write};
    use std::process::Stdio;

    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(repo_path)
        .args(args)
        .env("LC_ALL", "C")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = cmd.spawn().map_err(spawn_error)?;

    let mut stdin = child.stdin.take().expect("stdin piped");
    let data = stdin_data.as_bytes().to_vec();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&data);
        // stdin drops here, closing the pipe so git sees EOF.
    });

    let mut stderr_pipe = child.stderr.take().expect("stderr piped");
    let stderr_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr_pipe.read_to_end(&mut buf);
        buf
    });

    let mut stdout = Vec::new();
    if let Some(mut out) = child.stdout.take() {
        let _ = out.read_to_end(&mut stdout);
    }

    let status = child
        .wait()
        .map_err(|e| anyhow::Error::new(e).context("waiting for git"))?;
    let _ = writer.join();
    let stderr = stderr_reader.join().unwrap_or_default();

    Ok(std::process::Output {
        status,
        stdout,
        stderr,
    })
}

/// Run `git <args>` and stream stdout line-by-line, invoking `on_line` with
/// the running line count. Used for progress reporting on long operations
/// (`git add -A --verbose` prints one line per file it stages). Errors carry
/// stderr like [`run`].
pub(crate) fn run_streaming_lines(
    repo_path: &Path,
    args: &[&str],
    on_line: &mut dyn FnMut(u64),
) -> Result<()> {
    use std::io::{BufRead, BufReader, Read};
    use std::process::Stdio;

    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(repo_path)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = cmd.spawn().map_err(spawn_error)?;

    let mut stderr_pipe = child.stderr.take().expect("stderr piped");
    let stderr_reader = std::thread::spawn(move || {
        let mut buf = String::new();
        let _ = stderr_pipe.read_to_string(&mut buf);
        buf
    });

    let stdout = child.stdout.take().expect("stdout piped");
    let mut count: u64 = 0;
    for line in BufReader::new(stdout).lines() {
        if line.is_err() {
            break;
        }
        count += 1;
        on_line(count);
    }

    let status = child
        .wait()
        .map_err(|e| anyhow::Error::new(e).context("waiting for git"))?;
    let stderr = stderr_reader.join().unwrap_or_default();
    if !status.success() {
        anyhow::bail!(
            "git {} failed (exit {}): {}",
            args.join(" "),
            status.code().unwrap_or(-1),
            stderr.trim()
        );
    }
    Ok(())
}

/// Run `git <args>` (optionally inside `repo_path`) and stream its STDERR —
/// where git writes sideband progress ("Receiving objects: 42% (12000/90000)",
/// "Updating files: 18% (16200/90000)") — invoking `on_progress` with each
/// update. Progress lines are terminated by `\r` (in-place terminal updates),
/// finals by `\n`, so the reader splits on both. The last few stderr lines are
/// kept for the error message on non-zero exit.
///
/// Callers must include `--progress` in `args` — git suppresses progress when
/// stderr isn't a TTY otherwise.
pub(crate) fn run_streaming_progress(
    repo_path: Option<&Path>,
    args: &[&str],
    on_progress: &mut dyn FnMut(&str),
) -> Result<()> {
    use std::io::Read;
    use std::process::Stdio;

    let mut cmd = Command::new("git");
    if let Some(p) = repo_path {
        cmd.arg("-C").arg(p);
    }
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = cmd.spawn().map_err(spawn_error)?;

    // stdout (merge summaries, ref updates) drained on a side thread so the
    // child can't block on a full pipe while we're reading stderr.
    let mut stdout_pipe = child.stdout.take().expect("stdout piped");
    let stdout_reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stdout_pipe.read_to_string(&mut s);
        s
    });

    let mut stderr_pipe = child.stderr.take().expect("stderr piped");
    let mut tail: std::collections::VecDeque<String> = std::collections::VecDeque::new();
    let mut acc: Vec<u8> = Vec::new();
    let mut buf = [0u8; 4096];
    let flush = |acc: &mut Vec<u8>,
                 tail: &mut std::collections::VecDeque<String>,
                 on_progress: &mut dyn FnMut(&str)| {
        if acc.is_empty() {
            return;
        }
        let line = String::from_utf8_lossy(acc).trim().to_string();
        acc.clear();
        if line.is_empty() {
            return;
        }
        on_progress(&line);
        if tail.len() >= 20 {
            tail.pop_front();
        }
        tail.push_back(line);
    };
    loop {
        let n = match stderr_pipe.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => break,
        };
        for &b in &buf[..n] {
            if b == b'\r' || b == b'\n' {
                flush(&mut acc, &mut tail, on_progress);
            } else {
                acc.push(b);
            }
        }
    }
    flush(&mut acc, &mut tail, on_progress);

    let status = child
        .wait()
        .map_err(|e| anyhow::Error::new(e).context("waiting for git"))?;
    let _ = stdout_reader.join();

    if !status.success() {
        let detail: Vec<String> = tail.into_iter().collect();
        anyhow::bail!(
            "git {} failed (exit {}): {}",
            args.join(" "),
            status.code().unwrap_or(-1),
            detail.join("\n").trim()
        );
    }
    Ok(())
}

/// Returns the subset of `paths` that `git add` would reject. We use
/// `git add --dry-run` rather than `git check-ignore` because the two
/// diverge for already-tracked files whose parent directory matches an
/// ignore rule: `check-ignore` reports "not ignored" (it skips tracked
/// paths), but `git add` still aborts the entire batch with "paths are
/// ignored by .gitignore" — which is exactly the failure mode we're trying
/// to filter out. The dry-run mirrors the real `git add` behaviour exactly.
///
/// One batched dry-run (paths via stdin — no command-line length limit, no
/// per-path forks; the old per-path probe spawned N gits and froze the UI
/// for ~30s on a 500-file stage). On failure the rejected paths are read
/// from stderr (`LC_ALL=C` keeps the format stable); a per-path probe
/// remains as the fallback when stderr is unparseable.
pub(crate) fn rejected_by_add(repo_path: &Path, paths: &[&str]) -> Result<Vec<String>> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }

    let mut remaining: Vec<&str> = paths.to_vec();
    let mut rejected: Vec<String> = Vec::new();

    // Each pass either succeeds (done) or identifies at least one reject to
    // exclude from the next pass. Bounded; unparseable stderr falls through
    // to the per-path probe below.
    for _ in 0..5 {
        if remaining.is_empty() {
            return Ok(rejected);
        }
        let input = remaining.join("\n");
        let out = run_with_stdin(
            repo_path,
            &["add", "--dry-run", "--pathspec-from-file=-"],
            &input,
        )?;
        if out.status.success() {
            return Ok(rejected);
        }
        let stderr = String::from_utf8_lossy(&out.stderr);
        let newly = rejects_from_stderr(&stderr, &remaining);
        if newly.is_empty() {
            break;
        }
        let newly_set: std::collections::HashSet<&str> =
            newly.iter().map(String::as_str).collect();
        remaining.retain(|p| !newly_set.contains(p));
        rejected.extend(newly);
    }

    // Fallback: probe each remaining path individually (original behaviour —
    // any dry-run failure means the real add would fail too, so skip it).
    for p in remaining {
        let out = run_with_stdin(repo_path, &["add", "--dry-run", "--", p], "")?;
        if !out.status.success() {
            rejected.push(p.to_string());
        }
    }
    Ok(rejected)
}

/// Pick out which of `candidates` a failed `git add --dry-run` complained
/// about. The ignored-paths block lists one bare path per line; pathspec
/// errors quote the path (`pathspec 'x' did not match`). Matching candidate
/// paths against stderr lines avoids parsing exact message wording.
fn rejects_from_stderr(stderr: &str, candidates: &[&str]) -> Vec<String> {
    let lines: Vec<&str> = stderr.lines().map(str::trim).collect();
    let mut out = Vec::new();
    for p in candidates {
        let quoted = format!("'{}'", p);
        if lines.iter().any(|l| *l == *p || l.contains(&quoted)) {
            out.push((*p).to_string());
        }
    }
    out
}


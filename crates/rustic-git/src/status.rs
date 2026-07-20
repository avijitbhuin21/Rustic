use crate::repo::GitRepo;
use anyhow::Result;
use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub enum StatusType {
    New,
    Modified,
    Deleted,
    Renamed,
    Untracked,
    Conflicted,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileStatus {
    pub path: String,
    pub status: StatusType,
    pub is_staged: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct GitStatus {
    pub branch: String,
    pub files: Vec<FileStatus>,
    /// Total entry counts across the FULL working tree — independent of how many
    /// rows `files` actually carries (which may be capped by a `limit`). Lets the
    /// SCM panel show "82,431 changes" while only shipping/rendering a window.
    pub staged_count: usize,
    pub unstaged_count: usize,
    pub untracked_count: usize,
    /// True when `files` was truncated by a `limit` (more entries exist).
    pub truncated: bool,
}

impl GitRepo {
    /// Read working-tree status by parsing `git status --porcelain=v2`. We
    /// use the CLI here per docs/educated-guesses/006-cli-fallback-scope-expanded.md
    /// — gix's status iterator is significantly lower-level than libgit2's
    /// flat status flags, and the porcelain v2 format is a stable, well-spec'd
    /// contract.
    pub fn status(&self) -> Result<GitStatus> {
        self.status_limited(None)
    }

    /// Like [`status`](Self::status) but caps the returned `files` to `limit`
    /// entries (keeping the full counts in `*_count`). A repo where `node_modules`
    /// got staged/untracked can carry tens of thousands of entries; serializing
    /// and rendering them all freezes the UI. `git status --porcelain=v2` emits
    /// tracked changes (record types 1/2/u) BEFORE untracked (?), so a cap always
    /// keeps the small, meaningful staged/modified set and only truncates the
    /// (usually huge) untracked tail. `None` returns everything (the desktop
    /// internal callers like `discard_changes` rely on the full list).
    pub fn status_limited(&self, limit: Option<usize>) -> Result<GitStatus> {
        let branch = self.head_branch().unwrap_or_else(|_| "HEAD".to_string());
        let work_dir = self.work_dir()?;
        let out = crate::git_cli::run(
            &work_dir,
            &["status", "--porcelain=v2", "--untracked-files=all"],
        )?;

        let mut files = Vec::new();
        for line in out.lines() {
            if line.is_empty() {
                continue;
            }
            parse_porcelain_line(line, &mut files);
        }

        // Counts over the full parse (entry counts — a "MM" file yields two
        // entries, matching how `files` and the UI sections count rows).
        let mut staged_count = 0;
        let mut unstaged_count = 0;
        let mut untracked_count = 0;
        for f in &files {
            match f.status {
                StatusType::Untracked => untracked_count += 1,
                _ if f.is_staged => staged_count += 1,
                _ => unstaged_count += 1,
            }
        }

        let truncated = limit.is_some_and(|n| files.len() > n);
        if let Some(n) = limit {
            files.truncate(n);
        }

        Ok(GitStatus {
            branch,
            files,
            staged_count,
            unstaged_count,
            untracked_count,
            truncated,
        })
    }

    /// Stage the entire working tree (modifications, deletions, and untracked),
    /// like `git add -A` (which honours .gitignore). The repo-wide counterpart
    /// to [`stage`](Self::stage) — used by the SCM "Stage all" button so it acts
    /// on every change without the frontend enumerating (possibly tens of
    /// thousands of) paths.
    pub fn stage_all(&self) -> Result<()> {
        let work_dir = self.work_dir()?;
        crate::git_cli::run_silent(&work_dir, &["add", "-A"])
    }

    /// Like [`stage_all`](Self::stage_all), but reports progress: `git add -A
    /// --verbose` prints one line per file staged, and `on_progress` is called
    /// with the running count. On a 90k-file initial commit this is the only
    /// signal the user gets that anything is happening — `git add` itself can
    /// run for minutes while it hashes objects.
    pub fn stage_all_with_progress(&self, on_progress: &mut dyn FnMut(u64)) -> Result<()> {
        let work_dir = self.work_dir()?;
        crate::git_cli::run_streaming_lines(&work_dir, &["add", "-A", "--verbose"], on_progress)
    }

    /// Unstage the entire index — the repo-wide counterpart to
    /// [`unstage`](Self::unstage). `git reset` unstages everything against HEAD;
    /// a fresh repo with no commits has no HEAD, so that errors and we fall back
    /// to clearing the index directly (`git rm -r --cached`), which unstages all
    /// while leaving the worktree files untouched. The no-HEAD case matters here
    /// because the classic trigger is `git add .` over a huge tree in a
    /// just-initialised repo.
    pub fn unstage_all(&self) -> Result<()> {
        let work_dir = self.work_dir()?;
        if crate::git_cli::run_silent(&work_dir, &["reset", "-q"]).is_err() {
            crate::git_cli::run_silent(&work_dir, &["rm", "-r", "--cached", "-q", "--", "."])?;
        }
        Ok(())
    }

    /// Discard ALL unstaged worktree modifications and delete ALL untracked
    /// files, leaving staged changes intact — the repo-wide counterpart to
    /// [`discard_changes`](Self::discard_changes). Reverts tracked worktree
    /// edits (`git restore --worktree`) then removes untracked files/dirs
    /// (`git clean -fd`, which honours .gitignore so ignored artefacts survive).
    pub fn discard_all(&self) -> Result<()> {
        let work_dir = self.work_dir()?;
        crate::git_cli::run_silent(&work_dir, &["restore", "--worktree", "--", "."])?;
        crate::git_cli::run_silent(&work_dir, &["clean", "-fd"])?;
        Ok(())
    }

    /// Stage the given paths. Paths that match a .gitignore rule are filtered
    /// out before invoking `git add` (so a single ignored path can't abort the
    /// whole batch with "paths ignored by .gitignore"). The returned vec lists
    /// the paths that were skipped — callers should surface this to the user
    /// so they understand why their .gitignored worktree/build artefact didn't
    /// get committed.
    pub fn stage(&self, paths: &[String]) -> Result<Vec<String>> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        let work_dir = self.work_dir()?;
        let path_refs: Vec<&str> = paths.iter().map(String::as_str).collect();

        let skipped = crate::git_cli::rejected_by_add(&work_dir, &path_refs)?;
        let skipped_set: std::collections::HashSet<&str> =
            skipped.iter().map(String::as_str).collect();

        let to_stage: Vec<&str> = path_refs
            .iter()
            .copied()
            .filter(|p| !skipped_set.contains(p))
            .collect();

        if !to_stage.is_empty() {
            // Paths ride on stdin (`--pathspec-from-file=-`), not argv — a
            // multi-hundred-path batch overflows the ~32K Windows command
            // line and aborts the whole add.
            let input = to_stage.join("\n");
            let out = crate::git_cli::run_with_stdin(
                &work_dir,
                &["add", "--pathspec-from-file=-"],
                &input,
            )?;
            if !out.status.success() {
                anyhow::bail!(
                    "git add failed (exit {}): {}",
                    out.status.code().unwrap_or(-1),
                    String::from_utf8_lossy(&out.stderr).trim()
                );
            }
        }
        Ok(skipped)
    }

    pub fn unstage(&self, paths: &[String]) -> Result<()> {
        if paths.is_empty() {
            return Ok(());
        }
        let work_dir = self.work_dir()?;
        // `git restore --staged` is the modern unstage command (git >= 2.23).
        // It works for both pre- and post-initial-commit repos, removing the
        // libgit2-era special case for repos without a HEAD. Paths via stdin
        // for the same command-line-length reason as `stage`.
        let input = paths.join("\n");
        let out = crate::git_cli::run_with_stdin(
            &work_dir,
            &["restore", "--staged", "--pathspec-from-file=-"],
            &input,
        )?;
        if !out.status.success() {
            anyhow::bail!(
                "git restore --staged failed (exit {}): {}",
                out.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(())
    }

    /// List gitignored, untracked paths relative to the repo root
    /// (`git ls-files -o -i --exclude-standard --directory`), optionally
    /// scoped to `pathspecs`. Fully-ignored directories collapse to a single
    /// `dir/` entry (trailing slash) instead of listing every file inside.
    pub fn list_ignored(&self, pathspecs: &[&str]) -> Result<Vec<String>> {
        let work_dir = self.work_dir()?;
        let mut args: Vec<&str> = vec![
            "ls-files",
            "--others",
            "--ignored",
            "--exclude-standard",
            "--directory",
        ];
        if !pathspecs.is_empty() {
            args.push("--");
            args.extend_from_slice(pathspecs);
        }
        let out = crate::git_cli::run(&work_dir, &args)?;
        Ok(out
            .lines()
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect())
    }

    /// Create a commit from the current staged state, returning the new
    /// commit's hex OID.
    pub fn commit(&self, message: &str) -> Result<String> {
        let work_dir = self.work_dir()?;
        // Allow committing without staged changes (matches libgit2-era
        // behaviour: previous code happily wrote an empty tree if nothing
        // was staged). `--allow-empty` keeps parity.
        crate::git_cli::run_silent(&work_dir, &["commit", "--allow-empty", "-m", message])?;
        // Read HEAD oid for the returned commit hash.
        let head = self
            .head_oid()
            .ok_or_else(|| anyhow::anyhow!("commit succeeded but HEAD has no oid"))?;
        Ok(head.to_string())
    }

    /// Discard worktree changes for the given paths. Tracked paths are
    /// reverted to HEAD via `git checkout HEAD -- <path>`. Untracked paths
    /// aren't in HEAD, so `checkout` would error with "did not match any
    /// file(s) known to git" — instead we delete them from disk, matching
    /// the SCM panel's "delete any new untracked files" promise.
    pub fn discard_changes(&self, paths: &[String]) -> Result<()> {
        if paths.is_empty() {
            return Ok(());
        }
        let work_dir = self.work_dir()?;

        // Look up current status once to classify each requested path. A
        // path is untracked if any of its status entries is Untracked AND
        // none are tracked — we can't have a tracked-and-untracked path
        // (porcelain v2 won't emit both), so the first matching record is
        // authoritative.
        let status = self.status()?;
        let mut untracked: Vec<&str> = Vec::new();
        let mut tracked: Vec<&str> = Vec::new();
        for p in paths {
            let is_untracked = status
                .files
                .iter()
                .any(|f| f.path == *p && matches!(f.status, StatusType::Untracked));
            if is_untracked {
                untracked.push(p.as_str());
            } else {
                tracked.push(p.as_str());
            }
        }

        if !tracked.is_empty() {
            let mut args: Vec<&str> = vec!["checkout", "HEAD", "--"];
            args.extend(tracked.iter().copied());
            crate::git_cli::run_silent(&work_dir, &args)?;
        }

        for p in untracked {
            let abs = work_dir.join(p);
            // Best-effort: if a file vanished between status read and now
            // (concurrent edit, race), don't fail the whole discard.
            let meta = match std::fs::metadata(&abs) {
                Ok(m) => m,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => {
                    return Err(anyhow::Error::new(e).context(format!("stat {}", abs.display())))
                }
            };
            let res = if meta.is_dir() {
                std::fs::remove_dir_all(&abs)
            } else {
                std::fs::remove_file(&abs)
            };
            if let Err(e) = res {
                if e.kind() != std::io::ErrorKind::NotFound {
                    return Err(anyhow::Error::new(e)
                        .context(format!("delete untracked {}", abs.display())));
                }
            }
        }

        Ok(())
    }
}

/// Parse one line of `git status --porcelain=v2` output. The v2 format is
/// space-separated and starts with a one-character record type (1 = ordinary
/// change, 2 = rename/copy, u = unmerged conflict, ? = untracked, ! = ignored).
/// We map each to the appropriate (StatusType, is_staged) and push to `out`.
/// Ordinary-format `1` lines look like:
///
/// ```text
/// 1 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <path>
/// ```
///
/// where XY is the two-character status code (X = index, Y = worktree).
fn parse_porcelain_line(line: &str, out: &mut Vec<FileStatus>) {
    let mut chars = line.chars();
    let record_type = match chars.next() {
        Some(c) => c,
        None => return,
    };

    match record_type {
        '1' => {
            // "1 XY ..." — parse the XY status code, then walk forward to the path.
            let parts: Vec<&str> = line.splitn(9, ' ').collect();
            if parts.len() < 9 {
                return;
            }
            let xy = parts[1];
            let path = parts[8];
            push_xy(xy, path, out);
        }
        '2' => {
            // Renamed/copied. Format: "2 XY ... <path>\t<orig_path>"
            let parts: Vec<&str> = line.splitn(10, ' ').collect();
            if parts.len() < 10 {
                return;
            }
            let xy = parts[1];
            // The last field contains "<new_path>\t<old_path>".
            let path_field = parts[9];
            let new_path = path_field.split('\t').next().unwrap_or(path_field);
            push_xy(xy, new_path, out);
        }
        'u' => {
            // Unmerged — always treated as conflict, never staged.
            // Format: "u XY <sub> <m1> <m2> <m3> <mW> <h1> <h2> <h3> <path>"
            let parts: Vec<&str> = line.splitn(11, ' ').collect();
            if let Some(path) = parts.get(10) {
                out.push(FileStatus {
                    path: (*path).to_string(),
                    status: StatusType::Conflicted,
                    is_staged: false,
                });
            }
        }
        '?' => {
            // Untracked. Format: "? <path>"
            if let Some(path) = line.get(2..) {
                out.push(FileStatus {
                    path: path.to_string(),
                    status: StatusType::Untracked,
                    is_staged: false,
                });
            }
        }
        _ => {} // ignored / unknown — skip
    }
}

/// Given an XY status code (e.g. "M.", ".M", "MM", "A.", ".D", "R."),
/// emit one or two FileStatus entries (one for the staged side, one for the
/// worktree side, when each is non-`.`).
fn push_xy(xy: &str, path: &str, out: &mut Vec<FileStatus>) {
    let mut chars = xy.chars();
    let x = chars.next().unwrap_or('.');
    let y = chars.next().unwrap_or('.');

    if let Some(status) = code_to_status(x) {
        out.push(FileStatus {
            path: path.to_string(),
            status,
            is_staged: true,
        });
    }
    if let Some(status) = code_to_status(y) {
        out.push(FileStatus {
            path: path.to_string(),
            status,
            is_staged: false,
        });
    }
}

fn code_to_status(c: char) -> Option<StatusType> {
    match c {
        '.' => None,
        'A' => Some(StatusType::New),
        'M' => Some(StatusType::Modified),
        'D' => Some(StatusType::Deleted),
        'R' | 'C' => Some(StatusType::Renamed),
        'U' => Some(StatusType::Conflicted),
        // T (type change) and others are rare; map to Modified rather than skip.
        _ => Some(StatusType::Modified),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ordinary_modified_in_worktree() {
        let mut out = Vec::new();
        parse_porcelain_line(
            "1 .M N... 100644 100644 100644 abc def src/main.rs",
            &mut out,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].path, "src/main.rs");
        assert_eq!(out[0].status, StatusType::Modified);
        assert!(!out[0].is_staged);
    }

    #[test]
    fn parses_ordinary_staged_and_worktree_modified() {
        let mut out = Vec::new();
        parse_porcelain_line(
            "1 MM N... 100644 100644 100644 abc def src/lib.rs",
            &mut out,
        );
        assert_eq!(out.len(), 2);
        assert!(out
            .iter()
            .any(|f| f.is_staged && f.status == StatusType::Modified));
        assert!(out
            .iter()
            .any(|f| !f.is_staged && f.status == StatusType::Modified));
    }

    #[test]
    fn parses_untracked() {
        let mut out = Vec::new();
        parse_porcelain_line("? new_file.txt", &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].path, "new_file.txt");
        assert_eq!(out[0].status, StatusType::Untracked);
    }

    #[test]
    fn parses_renamed_takes_new_path() {
        let mut out = Vec::new();
        parse_porcelain_line(
            "2 R. N... 100644 100644 100644 abc def R100 new.rs\told.rs",
            &mut out,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].path, "new.rs");
        assert_eq!(out[0].status, StatusType::Renamed);
        assert!(out[0].is_staged);
    }

    #[test]
    fn parses_conflict() {
        let mut out = Vec::new();
        parse_porcelain_line(
            "u UU N... 100644 100644 100644 100644 abc def ghi conflicted.rs",
            &mut out,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].path, "conflicted.rs");
        assert_eq!(out[0].status, StatusType::Conflicted);
    }
}

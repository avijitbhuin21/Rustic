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
}

impl GitRepo {
    /// Read working-tree status by parsing `git status --porcelain=v2`. We
    /// use the CLI here per docs/educated-guesses/006-cli-fallback-scope-expanded.md
    /// — gix's status iterator is significantly lower-level than libgit2's
    /// flat status flags, and the porcelain v2 format is a stable, well-spec'd
    /// contract.
    pub fn status(&self) -> Result<GitStatus> {
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
        Ok(GitStatus { branch, files })
    }

    pub fn stage(&self, paths: &[String]) -> Result<()> {
        if paths.is_empty() {
            return Ok(());
        }
        let work_dir = self.work_dir()?;
        let mut args: Vec<&str> = vec!["add", "--"];
        for p in paths {
            args.push(p.as_str());
        }
        crate::git_cli::run_silent(&work_dir, &args)
    }

    pub fn unstage(&self, paths: &[String]) -> Result<()> {
        if paths.is_empty() {
            return Ok(());
        }
        let work_dir = self.work_dir()?;
        // `git restore --staged` is the modern unstage command (git >= 2.23).
        // It works for both pre- and post-initial-commit repos, removing the
        // libgit2-era special case for repos without a HEAD.
        let mut args: Vec<&str> = vec!["restore", "--staged", "--"];
        for p in paths {
            args.push(p.as_str());
        }
        crate::git_cli::run_silent(&work_dir, &args)
    }

    /// Create a commit from the current staged state, returning the new
    /// commit's hex OID.
    pub fn commit(&self, message: &str) -> Result<String> {
        let work_dir = self.work_dir()?;
        // Allow committing without staged changes (matches libgit2-era
        // behaviour: previous code happily wrote an empty tree if nothing
        // was staged). `--allow-empty` keeps parity.
        crate::git_cli::run_silent(
            &work_dir,
            &["commit", "--allow-empty", "-m", message],
        )?;
        // Read HEAD oid for the returned commit hash.
        let head = self
            .head_oid()
            .ok_or_else(|| anyhow::anyhow!("commit succeeded but HEAD has no oid"))?;
        Ok(head.to_string())
    }

    /// Discard worktree changes for the given paths (revert to HEAD).
    pub fn discard_changes(&self, paths: &[String]) -> Result<()> {
        if paths.is_empty() {
            return Ok(());
        }
        let work_dir = self.work_dir()?;
        let mut args: Vec<&str> = vec!["checkout", "HEAD", "--"];
        for p in paths {
            args.push(p.as_str());
        }
        crate::git_cli::run_silent(&work_dir, &args)
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
        parse_porcelain_line("1 .M N... 100644 100644 100644 abc def src/main.rs", &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].path, "src/main.rs");
        assert_eq!(out[0].status, StatusType::Modified);
        assert!(!out[0].is_staged);
    }

    #[test]
    fn parses_ordinary_staged_and_worktree_modified() {
        let mut out = Vec::new();
        parse_porcelain_line("1 MM N... 100644 100644 100644 abc def src/lib.rs", &mut out);
        assert_eq!(out.len(), 2);
        assert!(out.iter().any(|f| f.is_staged && f.status == StatusType::Modified));
        assert!(out.iter().any(|f| !f.is_staged && f.status == StatusType::Modified));
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

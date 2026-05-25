use crate::diff::{parse_unified_text, FileDiff};
use crate::repo::GitRepo;
use anyhow::{Context, Result};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct CommitInfo {
    pub oid: String,
    pub short_id: String,
    pub message: String,
    pub author_name: String,
    pub author_email: String,
    pub timestamp: i64,
    pub parent_count: usize,
    /// Branch names pointing at this commit
    pub refs: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommitFileChange {
    pub path: String,
    /// "added", "modified", "deleted", "renamed"
    pub status: String,
    pub additions: usize,
    pub deletions: usize,
}

impl GitRepo {
    /// Walk HEAD backwards, returning up to `max_count` commits.
    pub fn log(&self, max_count: usize) -> Result<Vec<CommitInfo>> {
        let head_oid = match self.head_oid() {
            Some(o) => o,
            None => return Ok(Vec::new()),
        };

        let ref_map = self.build_ref_map();

        let mut commits = Vec::new();
        let walk = self
            .repo
            .rev_walk([head_oid])
            .sorting(gix::revision::walk::Sorting::ByCommitTime(
                gix::traverse::commit::simple::CommitTimeOrder::NewestFirst,
            ))
            .all()
            .context("failed to start revwalk")?;

        for (i, info) in walk.enumerate() {
            if i >= max_count {
                break;
            }
            let info = info?;
            let oid = info.id;
            let commit = self.repo.find_commit(oid)?;

            let oid_str = oid.to_string();
            let short_id = oid_str.chars().take(7).collect::<String>();

            let message = commit
                .message_raw_sloppy()
                .to_string()
                .trim()
                .to_string();
            let author = commit.author()?;
            let author_name = author.name.to_string();
            let author_email = author.email.to_string();
            let timestamp = author.time()?.seconds;
            let parent_count = commit.parent_ids().count();

            let refs = ref_map.get(&oid_str).cloned().unwrap_or_default();

            commits.push(CommitInfo {
                oid: oid_str,
                short_id,
                message,
                author_name,
                author_email,
                timestamp,
                parent_count,
                refs,
            });
        }

        Ok(commits)
    }

    /// Files changed in a specific commit, with per-file additions/deletions.
    /// Implemented via `git show --numstat --name-status <oid>` to avoid
    /// reimplementing libgit2's two-pass per-line iteration over a Diff.
    pub fn commit_files(&self, oid_str: &str) -> Result<Vec<CommitFileChange>> {
        let work_dir = self.work_dir()?;
        let out = crate::git_cli::run(
            &work_dir,
            &[
                "show",
                "--no-color",
                "--numstat",
                "--name-status",
                "--format=",
                oid_str,
            ],
        )?;

        Ok(parse_show_summary(&out))
    }

    /// Unified diff for one file in one commit.
    pub fn commit_file_diff(&self, oid_str: &str, path: &str) -> Result<FileDiff> {
        let work_dir = self.work_dir()?;
        let text = crate::git_cli::run(
            &work_dir,
            &[
                "show",
                "--no-color",
                "-U3",
                "--format=",
                oid_str,
                "--",
                path,
            ],
        )?;
        let mut diffs = parse_unified_text(&text);
        Ok(diffs.pop().unwrap_or(FileDiff {
            file_path: path.to_string(),
            hunks: Vec::new(),
            additions: 0,
            deletions: 0,
        }))
    }

    /// Map commit OID hex string → list of local branch names pointing at it.
    pub(crate) fn build_ref_map(&self) -> std::collections::HashMap<String, Vec<String>> {
        let mut map: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        if let Ok(refs_platform) = self.repo.references() {
            if let Ok(local) = refs_platform.local_branches() {
                for branch in local.flatten() {
                    let name = branch.name().as_bstr().to_string();
                    let short = name
                        .strip_prefix("refs/heads/")
                        .unwrap_or(&name)
                        .to_string();
                    let target_oid = branch.id().detach().to_string();
                    map.entry(target_oid).or_default().push(short);
                }
            }
        }
        map
    }
}

/// Parse the merged output of `git show --numstat --name-status --format=`.
/// The two outputs are concatenated:
///
/// ```text
/// M       path/to/changed.rs
/// A       path/to/new.rs
/// D       path/to/old.rs
/// R100    src/old.rs      src/new.rs
/// <blank>
/// 5       3       path/to/changed.rs
/// 10      0       path/to/new.rs
/// 0       8       path/to/old.rs
/// 20      5       src/new.rs
/// ```
///
/// We zip them by path. Binary additions/deletions show as `-\t-\tpath` in
/// numstat; we report (0, 0) in that case.
fn parse_show_summary(text: &str) -> Vec<CommitFileChange> {
    let mut name_status: Vec<(String, String)> = Vec::new();
    let mut numstat: Vec<(usize, usize, String)> = Vec::new();

    for line in text.lines() {
        if line.is_empty() {
            continue;
        }
        // numstat lines start with digits or `-` (binary).
        let first = line.chars().next().unwrap_or(' ');
        if first.is_ascii_digit() || first == '-' {
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() < 3 {
                continue;
            }
            let adds = parts[0].parse::<usize>().unwrap_or(0);
            let dels = parts[1].parse::<usize>().unwrap_or(0);
            let path = parts[parts.len() - 1].to_string();
            numstat.push((adds, dels, path));
        } else {
            // name-status: "M\tpath" or "R100\told\tnew"
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() < 2 {
                continue;
            }
            let status_letter = parts[0].chars().next().unwrap_or('M');
            let status = match status_letter {
                'A' => "added",
                'D' => "deleted",
                'R' => "renamed",
                'C' => "copied",
                'T' => "modified",
                _ => "modified",
            };
            // Rename/copy lines have two paths — take the new (last) one.
            let path = parts[parts.len() - 1].to_string();
            name_status.push((status.to_string(), path));
        }
    }

    let mut out = Vec::with_capacity(name_status.len());
    for (status, path) in name_status {
        let (additions, deletions) = numstat
            .iter()
            .find(|(_, _, p)| p == &path)
            .map(|(a, d, _)| (*a, *d))
            .unwrap_or((0, 0));
        out.push(CommitFileChange {
            path,
            status,
            additions,
            deletions,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_show_summary_zips_name_and_numstat() {
        let text = "\
M\tsrc/main.rs
A\tsrc/new.rs
D\tsrc/old.rs

5\t3\tsrc/main.rs
10\t0\tsrc/new.rs
0\t8\tsrc/old.rs
";
        let out = parse_show_summary(text);
        assert_eq!(out.len(), 3);

        let main = out.iter().find(|c| c.path == "src/main.rs").unwrap();
        assert_eq!(main.status, "modified");
        assert_eq!(main.additions, 5);
        assert_eq!(main.deletions, 3);

        let new = out.iter().find(|c| c.path == "src/new.rs").unwrap();
        assert_eq!(new.status, "added");
        assert_eq!(new.additions, 10);
        assert_eq!(new.deletions, 0);

        let old = out.iter().find(|c| c.path == "src/old.rs").unwrap();
        assert_eq!(old.status, "deleted");
        assert_eq!(old.additions, 0);
        assert_eq!(old.deletions, 8);
    }

    #[test]
    fn parse_show_summary_handles_binary() {
        let text = "\
M\timage.png

-\t-\timage.png
";
        let out = parse_show_summary(text);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].path, "image.png");
        assert_eq!(out[0].additions, 0);
        assert_eq!(out[0].deletions, 0);
    }

    #[test]
    fn parse_show_summary_handles_rename() {
        let text = "\
R100\tsrc/old.rs\tsrc/new.rs

0\t0\tsrc/new.rs
";
        let out = parse_show_summary(text);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].status, "renamed");
        assert_eq!(out[0].path, "src/new.rs");
    }
}

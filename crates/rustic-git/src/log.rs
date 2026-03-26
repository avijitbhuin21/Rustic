use crate::diff::FileDiff;
use crate::repo::GitRepo;
use anyhow::Result;
use git2::{DiffOptions, Oid, Sort};
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
    /// Get commit log for the current branch.
    /// Returns up to `max_count` commits starting from HEAD.
    pub fn log(&self, max_count: usize) -> Result<Vec<CommitInfo>> {
        if !self.has_commits() {
            return Ok(Vec::new());
        }

        let mut revwalk = self.repo.revwalk()?;
        revwalk.set_sorting(Sort::TIME)?;
        revwalk.push_head()?;

        // Build a map of oid -> branch names for decoration
        let ref_map = self.build_ref_map();

        let mut commits = Vec::new();
        for (i, oid_result) in revwalk.enumerate() {
            if i >= max_count {
                break;
            }
            let oid = oid_result?;
            let commit = self.repo.find_commit(oid)?;

            let short_id = &oid.to_string()[..7];
            let message = commit.message().unwrap_or("").trim().to_string();
            let author = commit.author();
            let author_name = author.name().unwrap_or("Unknown").to_string();
            let author_email = author.email().unwrap_or("").to_string();
            let timestamp = commit.time().seconds();
            let parent_count = commit.parent_count();

            let refs = ref_map
                .get(&oid.to_string())
                .cloned()
                .unwrap_or_default();

            commits.push(CommitInfo {
                oid: oid.to_string(),
                short_id: short_id.to_string(),
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

    /// Get the list of files changed in a specific commit.
    pub fn commit_files(&self, oid_str: &str) -> Result<Vec<CommitFileChange>> {
        let oid = Oid::from_str(oid_str)?;
        let commit = self.repo.find_commit(oid)?;
        let tree = commit.tree()?;

        let parent_tree = if commit.parent_count() > 0 {
            Some(commit.parent(0)?.tree()?)
        } else {
            None
        };

        let mut diff_opts = DiffOptions::new();
        let diff = self.repo.diff_tree_to_tree(
            parent_tree.as_ref(),
            Some(&tree),
            Some(&mut diff_opts),
        )?;

        let num_deltas = diff.deltas().len();
        let mut changes = Vec::with_capacity(num_deltas);

        // First pass: collect file entries with status
        for i in 0..num_deltas {
            let delta = diff.get_delta(i).unwrap();
            let path = delta
                .new_file()
                .path()
                .or_else(|| delta.old_file().path())
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();

            let status = match delta.status() {
                git2::Delta::Added => "added",
                git2::Delta::Deleted => "deleted",
                git2::Delta::Modified => "modified",
                git2::Delta::Renamed => "renamed",
                git2::Delta::Copied => "copied",
                _ => "modified",
            };

            changes.push(CommitFileChange {
                path,
                status: status.to_string(),
                additions: 0,
                deletions: 0,
            });
        }

        // Second pass: count additions/deletions per file using print
        let mut current_idx: usize = 0;
        diff.print(git2::DiffFormat::Patch, |delta, _hunk, line| {
            let path = delta
                .new_file()
                .path()
                .or_else(|| delta.old_file().path())
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();

            // Find the matching change entry
            if current_idx < changes.len() && changes[current_idx].path != path {
                // Moved to next file, scan forward
                for i in 0..changes.len() {
                    if changes[i].path == path {
                        current_idx = i;
                        break;
                    }
                }
            }

            if current_idx < changes.len() && changes[current_idx].path == path {
                match line.origin() {
                    '+' => changes[current_idx].additions += 1,
                    '-' => changes[current_idx].deletions += 1,
                    _ => {}
                }
            }

            true
        })?;

        Ok(changes)
    }

    /// Get the diff for a specific file in a specific commit.
    pub fn commit_file_diff(&self, oid_str: &str, path: &str) -> Result<FileDiff> {
        let oid = Oid::from_str(oid_str)?;
        let commit = self.repo.find_commit(oid)?;
        let tree = commit.tree()?;

        let parent_tree = if commit.parent_count() > 0 {
            Some(commit.parent(0)?.tree()?)
        } else {
            None
        };

        let mut diff_opts = DiffOptions::new();
        diff_opts.pathspec(path);
        let diff = self.repo.diff_tree_to_tree(
            parent_tree.as_ref(),
            Some(&tree),
            Some(&mut diff_opts),
        )?;

        let mut file_diffs = Self::parse_diff(&diff)?;
        Ok(file_diffs.pop().unwrap_or(FileDiff {
            file_path: path.to_string(),
            hunks: Vec::new(),
            additions: 0,
            deletions: 0,
        }))
    }

    /// Build a map of commit OID -> list of branch/tag names
    fn build_ref_map(&self) -> std::collections::HashMap<String, Vec<String>> {
        let mut map = std::collections::HashMap::new();

        if let Ok(branches) = self.repo.branches(None) {
            for branch_result in branches {
                if let Ok((branch, _btype)) = branch_result {
                    if let Ok(Some(name)) = branch.name() {
                        if let Some(target) = branch.get().target() {
                            map.entry(target.to_string())
                                .or_insert_with(Vec::new)
                                .push(name.to_string());
                        }
                    }
                }
            }
        }

        map
    }
}

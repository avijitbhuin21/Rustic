use crate::repo::GitRepo;
use anyhow::Result;
use git2::{DiffFormat, DiffOptions};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct DiffLine {
    pub origin: char,    // '+', '-', ' '
    pub content: String,
    pub old_lineno: Option<u32>,
    pub new_lineno: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiffHunk {
    pub header: String,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileDiff {
    pub file_path: String,
    pub hunks: Vec<DiffHunk>,
    pub additions: usize,
    pub deletions: usize,
}

impl GitRepo {
    /// Get diff for a specific file (unstaged changes vs index).
    pub fn diff_file(&self, path: &str) -> Result<FileDiff> {
        // Refresh the index from disk so we compare against the latest state
        let mut index = self.repo.index()?;
        index.read(false)?;

        let mut opts = DiffOptions::new();
        opts.pathspec(path);
        // Include untracked files and their content so new files show a diff
        opts.include_untracked(true);
        opts.recurse_untracked_dirs(true);

        let diff = self.repo.diff_index_to_workdir(Some(&index), Some(&mut opts))?;
        let mut file_diffs = Self::parse_diff(&diff)?;

        // Fallback: if no unstaged diff found, try staged diff (HEAD vs index)
        if file_diffs.is_empty() || file_diffs.last().map_or(true, |fd| fd.hunks.is_empty()) {
            if let Ok(head) = self.repo.head() {
                if let Ok(head_tree) = head.peel_to_tree() {
                    let mut staged_opts = DiffOptions::new();
                    staged_opts.pathspec(path);
                    if let Ok(staged_diff) = self.repo.diff_tree_to_index(
                        Some(&head_tree),
                        Some(&index),
                        Some(&mut staged_opts),
                    ) {
                        let mut staged_file_diffs = Self::parse_diff(&staged_diff)?;
                        if let Some(fd) = staged_file_diffs.pop() {
                            if !fd.hunks.is_empty() {
                                return Ok(fd);
                            }
                        }
                    }
                }
            }
        }

        Ok(file_diffs.pop().unwrap_or(FileDiff {
            file_path: path.to_string(),
            hunks: Vec::new(),
            additions: 0,
            deletions: 0,
        }))
    }

    /// Get diff for all staged changes.
    pub fn diff_staged(&self) -> Result<Vec<FileDiff>> {
        let mut index = self.repo.index()?;
        index.read(false)?;

        let head_tree = self.repo.head()?.peel_to_tree()?;
        let diff = self.repo.diff_tree_to_index(Some(&head_tree), Some(&index), None)?;
        Self::parse_diff(&diff)
    }

    pub(crate) fn parse_diff(diff: &git2::Diff) -> Result<Vec<FileDiff>> {
        let mut file_diffs: Vec<FileDiff> = Vec::new();

        diff.print(DiffFormat::Patch, |delta, hunk, line| {
            let path = delta
                .new_file()
                .path()
                .or_else(|| delta.old_file().path())
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();

            // Ensure we have a FileDiff for this file
            if file_diffs.last().map(|fd| &fd.file_path) != Some(&path) {
                file_diffs.push(FileDiff {
                    file_path: path,
                    hunks: Vec::new(),
                    additions: 0,
                    deletions: 0,
                });
            }

            let fd = file_diffs.last_mut().unwrap();

            match line.origin() {
                'H' => {
                    // Hunk header
                    let header = hunk
                        .map(|h| String::from_utf8_lossy(h.header()).trim().to_string())
                        .unwrap_or_default();
                    fd.hunks.push(DiffHunk {
                        header,
                        lines: Vec::new(),
                    });
                }
                '+' | '-' | ' ' => {
                    // Ensure there's a hunk
                    if fd.hunks.is_empty() {
                        let header = hunk
                            .map(|h| String::from_utf8_lossy(h.header()).trim().to_string())
                            .unwrap_or_default();
                        fd.hunks.push(DiffHunk {
                            header,
                            lines: Vec::new(),
                        });
                    }

                    let origin = line.origin();
                    if origin == '+' {
                        fd.additions += 1;
                    } else if origin == '-' {
                        fd.deletions += 1;
                    }

                    if let Some(hunk_data) = fd.hunks.last_mut() {
                        hunk_data.lines.push(DiffLine {
                            origin,
                            content: String::from_utf8_lossy(line.content()).to_string(),
                            old_lineno: line.old_lineno(),
                            new_lineno: line.new_lineno(),
                        });
                    }
                }
                _ => {} // skip file headers etc.
            }

            true
        })?;

        Ok(file_diffs)
    }
}

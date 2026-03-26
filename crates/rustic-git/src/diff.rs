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
        let mut opts = DiffOptions::new();
        opts.pathspec(path);

        let diff = self.repo.diff_index_to_workdir(None, Some(&mut opts))?;
        let mut file_diffs = Self::parse_diff(&diff)?;

        Ok(file_diffs.pop().unwrap_or(FileDiff {
            file_path: path.to_string(),
            hunks: Vec::new(),
            additions: 0,
            deletions: 0,
        }))
    }

    /// Get diff for all staged changes.
    pub fn diff_staged(&self) -> Result<Vec<FileDiff>> {
        let head_tree = self.repo.head()?.peel_to_tree()?;
        let diff = self.repo.diff_tree_to_index(Some(&head_tree), None, None)?;
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

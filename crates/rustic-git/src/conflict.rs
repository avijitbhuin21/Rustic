use crate::repo::GitRepo;
use anyhow::Result;
use serde::Serialize;
use std::fs;

#[derive(Debug, Clone, Serialize)]
pub enum ConflictSide {
    Ours,
    Theirs,
    Both,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConflictHunk {
    pub ours: String,
    pub theirs: String,
    pub start_line: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConflictFile {
    pub path: String,
    pub hunks: Vec<ConflictHunk>,
    pub content: String,
}

impl GitRepo {
    /// Return one entry per conflicted file in the working tree. Each entry
    /// includes the raw file content and any in-content `<<<<<<<` markers
    /// parsed into structured hunks.
    pub fn get_conflicts(&self) -> Result<Vec<ConflictFile>> {
        let work_dir = self.work_dir()?;
        // Use `git diff --name-only --diff-filter=U` to list the unmerged
        // paths — clean, well-defined, and matches what
        // libgit2's `index.conflicts()` reported.
        let out = crate::git_cli::run(&work_dir, &["diff", "--name-only", "--diff-filter=U"])?;

        let mut conflict_files = Vec::new();
        for path in out.lines() {
            if path.is_empty() {
                continue;
            }
            let file_path = work_dir.join(path);
            let content = fs::read_to_string(&file_path).unwrap_or_default();
            let hunks = parse_conflict_markers(&content);
            conflict_files.push(ConflictFile {
                path: path.to_string(),
                hunks,
                content,
            });
        }
        Ok(conflict_files)
    }

    pub fn resolve_conflict(&self, path: &str, resolved_content: &str) -> Result<()> {
        let work_dir = self.work_dir()?;
        let file_path = work_dir.join(path);
        crate::io_util::atomic_write(&file_path, resolved_content.as_bytes())?;

        // Stage the resolved file — `git add <path>` resolves the index entry
        // from the conflict state to a single-stage entry.
        crate::git_cli::run_silent(&work_dir, &["add", "--", path])
    }

    pub fn resolve_conflict_side(&self, path: &str, side: &str) -> Result<()> {
        let work_dir = self.work_dir()?;
        let file_path = work_dir.join(path);
        let content = fs::read_to_string(&file_path)?;
        let resolved = resolve_by_side(&content, side);
        self.resolve_conflict(path, &resolved)
    }

    pub fn has_conflicts(&self) -> Result<bool> {
        let work_dir = self.work_dir()?;
        let out = crate::git_cli::run(&work_dir, &["diff", "--name-only", "--diff-filter=U"])?;
        Ok(out.lines().any(|l| !l.trim().is_empty()))
    }

    /// Finalise a merge by committing the resolved state. Errors when any
    /// conflict remains unresolved.
    pub fn merge_commit(&self) -> Result<String> {
        if self.has_conflicts()? {
            anyhow::bail!("Cannot commit: unresolved conflicts remain");
        }
        let work_dir = self.work_dir()?;
        // `git commit --no-edit` finalises the merge using the prepared
        // MERGE_MSG. Works whether MERGE_HEAD is set or not (falls back to
        // a regular commit with the staged state).
        let mut cmd = std::process::Command::new("git");
        cmd.arg("-C")
            .arg(&work_dir)
            .args(["commit", "--no-edit", "--allow-empty"])
            .env("GIT_EDITOR", "true");

        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let output = cmd.output().map_err(crate::git_cli::spawn_error)?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("git commit --no-edit failed: {}", stderr.trim());
        }

        let head = self
            .head_oid()
            .ok_or_else(|| anyhow::anyhow!("commit succeeded but HEAD has no oid"))?;
        Ok(head.to_string())
    }
}

fn parse_conflict_markers(content: &str) -> Vec<ConflictHunk> {
    let mut hunks = Vec::new();
    let mut in_ours = false;
    let mut in_theirs = false;
    let mut ours_buf = String::new();
    let mut theirs_buf = String::new();
    let mut start_line = 0;

    for (i, line) in content.lines().enumerate() {
        if line.starts_with("<<<<<<<") {
            in_ours = true;
            start_line = i;
            ours_buf.clear();
            theirs_buf.clear();
        } else if line.starts_with("=======") && in_ours {
            in_ours = false;
            in_theirs = true;
        } else if line.starts_with(">>>>>>>") && in_theirs {
            in_theirs = false;
            hunks.push(ConflictHunk {
                ours: ours_buf.clone(),
                theirs: theirs_buf.clone(),
                start_line,
            });
        } else if in_ours {
            if !ours_buf.is_empty() {
                ours_buf.push('\n');
            }
            ours_buf.push_str(line);
        } else if in_theirs {
            if !theirs_buf.is_empty() {
                theirs_buf.push('\n');
            }
            theirs_buf.push_str(line);
        }
    }

    hunks
}

fn resolve_by_side(content: &str, side: &str) -> String {
    let mut result = String::new();
    let mut in_ours = false;
    let mut in_theirs = false;

    for line in content.lines() {
        if line.starts_with("<<<<<<<") {
            in_ours = true;
            continue;
        } else if line.starts_with("=======") && in_ours {
            in_ours = false;
            in_theirs = true;
            continue;
        } else if line.starts_with(">>>>>>>") && in_theirs {
            in_theirs = false;
            continue;
        }

        if in_ours && (side == "ours" || side == "both") {
            result.push_str(line);
            result.push('\n');
        } else if in_theirs && (side == "theirs" || side == "both") {
            result.push_str(line);
            result.push('\n');
        } else if !in_ours && !in_theirs {
            result.push_str(line);
            result.push('\n');
        }
    }

    result
}

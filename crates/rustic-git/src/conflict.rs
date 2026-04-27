use crate::repo::GitRepo;
use anyhow::{Result, Context};
use serde::Serialize;
use std::fs;
use std::path::Path;

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
    pub fn get_conflicts(&self) -> Result<Vec<ConflictFile>> {
        let index = self.repo.index()?;
        let mut conflict_files = Vec::new();
        let workdir = self.repo.workdir().context("No working directory")?;

        let conflicts = index.conflicts()?;
        for conflict in conflicts {
            let conflict = conflict?;
            let path = conflict
                .our
                .as_ref()
                .or(conflict.their.as_ref())
                .map(|e| String::from_utf8_lossy(&e.path).to_string())
                .unwrap_or_default();

            let file_path = workdir.join(&path);
            let content = fs::read_to_string(&file_path).unwrap_or_default();
            let hunks = parse_conflict_markers(&content);

            conflict_files.push(ConflictFile {
                path,
                hunks,
                content,
            });
        }

        Ok(conflict_files)
    }

    pub fn resolve_conflict(&self, path: &str, resolved_content: &str) -> Result<()> {
        let workdir = self.repo.workdir().context("No working directory")?;
        let file_path = workdir.join(path);
        crate::io_util::atomic_write(&file_path, resolved_content.as_bytes())?;

        let mut index = self.repo.index()?;
        index.add_path(Path::new(path))?;
        index.write()?;
        Ok(())
    }

    pub fn resolve_conflict_side(&self, path: &str, side: &str) -> Result<()> {
        let workdir = self.repo.workdir().context("No working directory")?;
        let file_path = workdir.join(path);
        let content = fs::read_to_string(&file_path)?;

        let resolved = resolve_by_side(&content, side);
        self.resolve_conflict(path, &resolved)
    }

    pub fn has_conflicts(&self) -> Result<bool> {
        let index = self.repo.index()?;
        Ok(index.has_conflicts())
    }

    pub fn merge_commit(&self) -> Result<String> {
        let index = self.repo.index()?;
        if index.has_conflicts() {
            return Err(anyhow::anyhow!("Cannot commit: unresolved conflicts remain"));
        }

        let mut index = self.repo.index()?;
        let oid = index.write_tree()?;
        let tree = self.repo.find_tree(oid)?;
        let sig = self.repo.signature()?;
        let head_commit = self.repo.head()?.peel_to_commit()?;

        // Check if MERGE_HEAD exists (we're in a merge)
        let merge_head_path = self.repo.path().join("MERGE_HEAD");
        let parents = if merge_head_path.exists() {
            let merge_head_content = fs::read_to_string(&merge_head_path)?;
            let merge_oid = git2::Oid::from_str(merge_head_content.trim())?;
            let merge_commit = self.repo.find_commit(merge_oid)?;
            vec![head_commit.clone(), merge_commit]
        } else {
            vec![head_commit.clone()]
        };

        let parent_refs: Vec<&git2::Commit> = parents.iter().collect();
        let commit_oid = self.repo.commit(
            Some("HEAD"),
            &sig,
            &sig,
            "Merge: resolve conflicts",
            &tree,
            &parent_refs,
        )?;

        self.repo.cleanup_state()?;
        Ok(commit_oid.to_string())
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

use crate::repo::GitRepo;
use anyhow::Result;
use git2::{Status, StatusOptions};
use serde::Serialize;
use std::path::Path;

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
    pub fn status(&self) -> Result<GitStatus> {
        let branch = self.head_branch().unwrap_or_else(|_| "HEAD".to_string());

        let mut opts = StatusOptions::new();
        opts.include_untracked(true)
            .recurse_untracked_dirs(true)
            .include_unmodified(false);

        let statuses = self.repo.statuses(Some(&mut opts))?;
        let mut files = Vec::new();

        for entry in statuses.iter() {
            let path = entry.path().unwrap_or("").to_string();
            let status = entry.status();

            // Index (staged) changes
            if status.intersects(Status::INDEX_NEW) {
                files.push(FileStatus { path: path.clone(), status: StatusType::New, is_staged: true });
            }
            if status.intersects(Status::INDEX_MODIFIED) {
                files.push(FileStatus { path: path.clone(), status: StatusType::Modified, is_staged: true });
            }
            if status.intersects(Status::INDEX_DELETED) {
                files.push(FileStatus { path: path.clone(), status: StatusType::Deleted, is_staged: true });
            }
            if status.intersects(Status::INDEX_RENAMED) {
                files.push(FileStatus { path: path.clone(), status: StatusType::Renamed, is_staged: true });
            }

            // Worktree (unstaged) changes
            if status.intersects(Status::WT_NEW) {
                files.push(FileStatus { path: path.clone(), status: StatusType::Untracked, is_staged: false });
            }
            if status.intersects(Status::WT_MODIFIED) {
                files.push(FileStatus { path: path.clone(), status: StatusType::Modified, is_staged: false });
            }
            if status.intersects(Status::WT_DELETED) {
                files.push(FileStatus { path: path.clone(), status: StatusType::Deleted, is_staged: false });
            }
            if status.intersects(Status::WT_RENAMED) {
                files.push(FileStatus { path: path.clone(), status: StatusType::Renamed, is_staged: false });
            }
            if status.intersects(Status::CONFLICTED) {
                files.push(FileStatus { path, status: StatusType::Conflicted, is_staged: false });
            }
        }

        Ok(GitStatus { branch, files })
    }

    pub fn stage(&self, paths: &[String]) -> Result<()> {
        let mut index = self.repo.index()?;
        for path in paths {
            index.add_path(Path::new(path))?;
        }
        index.write()?;
        Ok(())
    }

    pub fn unstage(&self, paths: &[String]) -> Result<()> {
        let head = self.repo.head()?.peel_to_tree()?;
        self.repo.reset_default(Some(&head.into_object()), paths.iter().map(Path::new))?;
        Ok(())
    }

    pub fn commit(&self, message: &str) -> Result<String> {
        let mut index = self.repo.index()?;
        let oid = index.write_tree()?;
        let tree = self.repo.find_tree(oid)?;
        let sig = self.repo.signature()?;

        let parent_commit = self.repo.head()?.peel_to_commit()?;
        let commit_oid = self.repo.commit(
            Some("HEAD"),
            &sig,
            &sig,
            message,
            &tree,
            &[&parent_commit],
        )?;

        Ok(commit_oid.to_string())
    }

    pub fn discard_changes(&self, paths: &[String]) -> Result<()> {
        let mut checkout_builder = git2::build::CheckoutBuilder::new();
        for path in paths {
            checkout_builder.path(path);
        }
        checkout_builder.force();
        self.repo.checkout_head(Some(&mut checkout_builder))?;
        Ok(())
    }
}

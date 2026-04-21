use anyhow::{Result, Context};
use git2::Repository;
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
pub struct BranchInfo {
    pub name: String,
    pub is_head: bool,
    pub is_remote: bool,
}

pub struct GitRepo {
    pub(crate) repo: Repository,
}

impl GitRepo {
    pub fn open(path: &Path) -> Result<Self> {
        let repo = Repository::discover(path)?;
        Ok(Self { repo })
    }

    pub fn head_branch(&self) -> Result<String> {
        match self.repo.head() {
            Ok(head) => {
                let name = head
                    .shorthand()
                    .unwrap_or("HEAD (detached)")
                    .to_string();
                Ok(name)
            }
            Err(e) if e.code() == git2::ErrorCode::UnbornBranch => {
                // No commits yet — read the unborn branch name from HEAD
                if let Ok(head_ref) = self.repo.find_reference("HEAD") {
                    if let Some(target) = head_ref.symbolic_target() {
                        if let Some(name) = target.strip_prefix("refs/heads/") {
                            return Ok(name.to_string());
                        }
                    }
                }
                Ok("main".to_string())
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Returns true if the repository has at least one commit.
    pub fn has_commits(&self) -> bool {
        self.repo.head().map(|h| h.target().is_some()).unwrap_or(false)
    }

    pub fn branches(&self) -> Result<Vec<BranchInfo>> {
        let mut result = Vec::new();
        let branches = self.repo.branches(None)?;

        for branch in branches {
            let (branch, branch_type) = branch?;
            let name = branch
                .name()?
                .unwrap_or("(invalid)")
                .to_string();
            let is_head = branch.is_head();
            let is_remote = branch_type == git2::BranchType::Remote;

            result.push(BranchInfo {
                name,
                is_head,
                is_remote,
            });
        }

        Ok(result)
    }

    pub fn init(path: &Path) -> Result<Self> {
        let repo = Repository::init(path)?;
        Ok(Self { repo })
    }

    pub fn checkout_branch(&self, name: &str) -> Result<()> {
        let (object, reference) = self.repo.revparse_ext(&format!("refs/heads/{}", name))?;
        self.repo.checkout_tree(&object, None)?;
        self.repo.set_head(
            reference
                .context("Branch reference not found")?
                .name()
                .context("Invalid reference name")?,
        )?;
        Ok(())
    }

    pub fn create_branch(&self, name: &str, checkout: bool) -> Result<()> {
        let head = self.repo.head()?.peel_to_commit()?;
        self.repo.branch(name, &head, false)?;
        if checkout {
            self.checkout_branch(name)?;
        }
        Ok(())
    }

    /// Soft-reset HEAD to its first parent — "undo the last commit". The
    /// working tree and index are left untouched, so the undone commit's
    /// changes reappear as staged changes ready to be re-committed or
    /// unstaged. Errors out if HEAD is a root commit (nothing to undo) or if
    /// HEAD is a merge (first-parent semantics aren't what the user wants
    /// for a merge — they should use a dedicated revert flow).
    pub fn undo_last_commit(&self) -> Result<()> {
        let head_commit = self.repo.head()?.peel_to_commit()?;

        if head_commit.parent_count() == 0 {
            anyhow::bail!("Cannot undo the initial commit — HEAD has no parent.");
        }
        if head_commit.parent_count() > 1 {
            anyhow::bail!("Cannot undo a merge commit via soft reset. Use git revert instead.");
        }

        let parent = head_commit.parent(0)?;
        let parent_obj = parent.as_object();
        self.repo.reset(parent_obj, git2::ResetType::Soft, None)?;
        Ok(())
    }
}

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
        let head = self.repo.head()?;
        let name = head
            .shorthand()
            .unwrap_or("HEAD (detached)")
            .to_string();
        Ok(name)
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
}

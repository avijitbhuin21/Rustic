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

    /// P1.4: list the names of every linked worktree attached to this repo.
    /// The main worktree (where `.git` lives directly) is not included.
    pub fn worktrees(&self) -> Result<Vec<String>> {
        let names = self.repo.worktrees()?;
        let mut out = Vec::with_capacity(names.len());
        for i in 0..names.len() {
            if let Some(n) = names.get(i) {
                out.push(n.to_string());
            }
        }
        Ok(out)
    }

    /// C3.7: resolve a worktree name to its on-disk absolute path. Returns
    /// `None` when no worktree by that name exists (likely renamed/removed
    /// out-of-band) or when the libgit2 lookup fails for any reason —
    /// callers iterate `worktrees()` then filter via this method, so a
    /// missing entry is just dropped from the list rather than aborting.
    pub fn worktree_path(&self, name: &str) -> Option<std::path::PathBuf> {
        self.repo
            .find_worktree(name)
            .ok()
            .map(|wt| wt.path().to_path_buf())
    }

    /// P1.4: create a new worktree under `path`. The branch named `branch`
    /// is created (or reused) and checked out into the worktree. Returns the
    /// absolute path the worktree ended up at.
    ///
    /// `name` must be unique among existing worktrees. `branch` is created
    /// from the current HEAD if it doesn't already exist; if it exists, the
    /// worktree checks it out as-is (git2 errors out if the branch is
    /// already checked out elsewhere, which is the protection we want).
    pub fn add_worktree(
        &self,
        name: &str,
        path: &Path,
        branch: Option<&str>,
    ) -> Result<std::path::PathBuf> {
        if name.trim().is_empty() {
            anyhow::bail!("worktree name cannot be empty");
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }

        let branch_name = branch.unwrap_or(name);
        // Build a Reference for the branch we want the worktree to check
        // out. If the branch doesn't exist yet, create it from HEAD first.
        let head_commit = self
            .repo
            .head()
            .and_then(|h| h.peel_to_commit())
            .context("repository has no HEAD commit; cannot create a worktree")?;
        let branch_ref = match self.repo.find_branch(branch_name, git2::BranchType::Local) {
            Ok(b) => b.into_reference(),
            Err(_) => {
                let new_branch = self.repo.branch(branch_name, &head_commit, false)?;
                new_branch.into_reference()
            }
        };

        let mut opts = git2::WorktreeAddOptions::new();
        opts.reference(Some(&branch_ref));

        let wt = self.repo.worktree(name, path, Some(&opts))?;
        Ok(wt.path().to_path_buf())
    }

    /// P1.4: prune a worktree by name. Removes the on-disk working directory
    /// and the administrative `.git/worktrees/<name>` entry. Errors out if
    /// the worktree has uncommitted changes unless `force` is true.
    pub fn remove_worktree(&self, name: &str, force: bool) -> Result<()> {
        let wt = self.repo.find_worktree(name)?;
        let path = wt.path().to_path_buf();

        if !force {
            // Best-effort dirty-check: open the worktree as its own Repository
            // and inspect its status. If anything is modified or untracked
            // (other than ignored), refuse the prune so we don't lose work.
            if path.exists() {
                if let Ok(sub) = Repository::open(&path) {
                    let mut opts = git2::StatusOptions::new();
                    opts.include_untracked(true).include_ignored(false);
                    if let Ok(statuses) = sub.statuses(Some(&mut opts)) {
                        if !statuses.is_empty() {
                            anyhow::bail!(
                                "worktree '{}' has uncommitted changes; pass force=true to remove anyway",
                                name
                            );
                        }
                    }
                }
            }
        }

        // Remove the working directory contents first so the prune doesn't
        // refuse on "directory not empty". git2's `prune` only touches the
        // admin metadata, not the worktree files.
        if path.exists() {
            std::fs::remove_dir_all(&path).ok();
        }

        let mut prune_opts = git2::WorktreePruneOptions::new();
        prune_opts.valid(true).working_tree(true).locked(force);
        wt.prune(Some(&mut prune_opts))?;
        Ok(())
    }
}

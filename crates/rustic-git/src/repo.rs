use anyhow::{Context, Result};
use gix::ObjectId;
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
pub struct BranchInfo {
    pub name: String,
    pub is_head: bool,
    pub is_remote: bool,
}

pub struct GitRepo {
    pub(crate) repo: gix::Repository,
}

impl GitRepo {
    pub fn open(path: &Path) -> Result<Self> {
        let repo = gix::discover(path)
            .with_context(|| format!("failed to discover git repo at {}", path.display()))?;
        Ok(Self { repo })
    }

    /// Returns the current branch's shorthand name, or `main` if HEAD points
    /// at an unborn branch (fresh `git init` with no commits yet).
    pub fn head_branch(&self) -> Result<String> {
        let head = self.repo.head().context("failed to read HEAD")?;
        match head.kind {
            gix::head::Kind::Symbolic(ref r) => {
                // Symbolic HEAD (the common case): strip `refs/heads/` from the
                // target reference name. Works for both borne and unborn branches.
                let name = r.name.as_bstr().to_string();
                Ok(name
                    .strip_prefix("refs/heads/")
                    .unwrap_or(&name)
                    .to_string())
            }
            gix::head::Kind::Unborn(ref r) => {
                // Unborn HEAD: the name we'll create on the first commit.
                let name = r.as_bstr().to_string();
                Ok(name
                    .strip_prefix("refs/heads/")
                    .unwrap_or(&name)
                    .to_string())
            }
            gix::head::Kind::Detached { .. } => Ok("HEAD (detached)".to_string()),
        }
    }

    /// True if the repository has at least one commit reachable from HEAD.
    pub fn has_commits(&self) -> bool {
        self.repo
            .head()
            .map(|h| h.try_into_peeled_id().ok().flatten().is_some())
            .unwrap_or(false)
    }

    /// List local + remote branches. Order is unspecified.
    pub fn branches(&self) -> Result<Vec<BranchInfo>> {
        let head_oid = self.head_oid();

        let mut result = Vec::new();
        let platform = self.repo.references().context("references platform")?;

        let local_iter = platform.local_branches().context("local_branches iter")?;
        for branch in local_iter {
            let branch = match branch {
                Ok(b) => b,
                Err(e) => {
                    tracing_warn_branch_iter(&e);
                    continue;
                }
            };
            let name = strip_refs_prefix(branch.name().as_bstr(), "refs/heads/");
            let target = branch.id().detach();
            let is_head = head_oid == Some(target);
            result.push(BranchInfo {
                name,
                is_head,
                is_remote: false,
            });
        }

        let remote_iter = platform.remote_branches().context("remote_branches iter")?;
        for branch in remote_iter {
            let branch = match branch {
                Ok(b) => b,
                Err(e) => {
                    tracing_warn_branch_iter(&e);
                    continue;
                }
            };
            let name = strip_refs_prefix(branch.name().as_bstr(), "refs/remotes/");
            result.push(BranchInfo {
                name,
                is_head: false,
                is_remote: true,
            });
        }

        Ok(result)
    }

    pub fn init(path: &Path) -> Result<Self> {
        let repo = gix::init(path)
            .with_context(|| format!("failed to init git repo at {}", path.display()))?;
        Ok(Self { repo })
    }

    /// Check out `refs/heads/<name>`. Updates HEAD and overwrites tracked
    /// worktree files to match the branch's tree. Untracked files are left
    /// alone; uncommitted modifications to tracked files are clobbered (same
    /// behaviour as the libgit2 implementation).
    pub fn checkout_branch(&self, name: &str) -> Result<()> {
        // gix's checkout primitives are lower-level than libgit2's; spawning
        // the git CLI is both simpler and more semantically correct (real
        // git's safety checks are battle-tested against unicode paths,
        // submodules, sparse checkout, etc.).
        let work_dir = self.work_dir()?;
        crate::git_cli::run_silent(&work_dir, &["checkout", name])
    }

    /// Create branch `name` at HEAD's commit. If `checkout` is true, the
    /// branch is checked out into the worktree after creation.
    pub fn create_branch(&self, name: &str, checkout: bool) -> Result<()> {
        let head_oid = self
            .head_oid()
            .context("repository has no HEAD commit; cannot create a branch")?;

        let ref_name = format!("refs/heads/{}", name);
        // PreviousValue::MustNotExist mirrors libgit2's `force = false`.
        self.repo
            .reference(
                ref_name.as_str(),
                head_oid,
                gix::refs::transaction::PreviousValue::MustNotExist,
                format!("create branch {}", name),
            )
            .with_context(|| format!("failed to create branch {}", name))?;

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
        let head_id = self
            .head_oid()
            .context("repository has no HEAD commit; nothing to undo")?;
        let head_commit = self.repo.find_commit(head_id)?;

        let parent_ids: Vec<_> = head_commit.parent_ids().collect();
        if parent_ids.is_empty() {
            anyhow::bail!("Cannot undo the initial commit — HEAD has no parent.");
        }
        if parent_ids.len() > 1 {
            anyhow::bail!("Cannot undo a merge commit via soft reset. Use git revert instead.");
        }

        let parent_id = parent_ids[0].detach();

        // Soft reset = point HEAD's symbolic target ref at the parent commit.
        // Worktree and index stay where they are.
        let head = self.repo.head()?;
        let target_ref_name = match head.kind {
            gix::head::Kind::Symbolic(r) => r.name.to_owned(),
            _ => anyhow::bail!("HEAD is detached or unborn; cannot soft-reset"),
        };

        self.repo
            .reference(
                target_ref_name.as_bstr().to_string().as_str(),
                parent_id,
                gix::refs::transaction::PreviousValue::Any,
                "soft reset (undo last commit)",
            )
            .context("failed to move HEAD branch to parent commit")?;

        Ok(())
    }

    /// P1.4: list every linked worktree attached to this repo. Main worktree
    /// (the one where `.git` lives directly) is not included.
    pub fn worktrees(&self) -> Result<Vec<String>> {
        let wts = self.repo.worktrees()?;
        Ok(wts.into_iter().map(|wt| wt.id().to_string()).collect())
    }

    /// C3.7: resolve a worktree name to its on-disk absolute path. Returns
    /// `None` when no worktree by that name exists or when the lookup fails.
    pub fn worktree_path(&self, name: &str) -> Option<std::path::PathBuf> {
        self.repo
            .worktrees()
            .ok()?
            .into_iter()
            .find(|wt| wt.id() == name)
            .and_then(|wt| wt.base().ok())
    }

    /// P1.4: create a new worktree under `path`. The branch `branch` is
    /// created (or reused) and checked out into the worktree. Returns the
    /// absolute path the worktree ended up at.
    ///
    /// Implemented via the `git` CLI: gix's worktree mutation surface is
    /// lower-level, and `git worktree add` already handles every edge case
    /// (existing branch, locked worktree, unicode path) we'd otherwise have
    /// to reimplement.
    /// Add a DETACHED worktree at `path`, checked out at `commit` (oid or
    /// refspec). No branch is created or checked out: the worktree's HEAD
    /// points straight at the commit, so task isolation leaves no trace in
    /// `git branch` output and nothing a default push refspec would upload.
    /// Returns the absolute path the worktree ended up at.
    ///
    /// Implemented via the `git` CLI: gix's worktree mutation surface is
    /// lower-level, and `git worktree add` already handles every edge case
    /// (locked worktree, unicode path) we'd otherwise have to reimplement.
    pub fn add_worktree_detached(&self, path: &Path, commit: &str) -> Result<std::path::PathBuf> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }

        let work_dir = self.work_dir()?;
        let path_str = path.to_string_lossy().into_owned();
        crate::git_cli::run_silent(
            &work_dir,
            &["worktree", "add", "--detach", &path_str, commit],
        )?;

        Ok(path.to_path_buf())
    }

    /// Detach HEAD in THIS worktree (`git checkout --detach`): keeps the
    /// commit and working tree as-is, drops only the branch association.
    pub fn detach_head(&self) -> Result<()> {
        let work_dir = self.work_dir()?;
        crate::git_cli::run_silent(&work_dir, &["checkout", "--detach"])
    }

    /// P1.4: prune a worktree by name. Removes the on-disk working directory
    /// and the admin entry. Errors if uncommitted changes exist unless force.
    pub fn remove_worktree(&self, name: &str, force: bool) -> Result<()> {
        let work_dir = self.work_dir()?;
        let mut args: Vec<&str> = vec!["worktree", "remove"];
        if force {
            args.push("--force");
        }
        args.push(name);
        crate::git_cli::run_silent(&work_dir, &args)?;
        Ok(())
    }

    /// Stage everything in THIS worktree (including untracked files) and
    /// return a binary patch of the index against `base` — the worktree's
    /// complete delta from its fork point. Intended for throwaway worktrees:
    /// staging is a visible side effect.
    pub fn diff_patch_from(&self, base: &str) -> Result<String> {
        let work_dir = self.work_dir()?;
        crate::git_cli::run_silent(&work_dir, &["add", "-A"])?;
        crate::git_cli::run(
            &work_dir,
            &["diff", "--cached", "--binary", "--full-index", base],
        )
    }

    /// Apply a binary patch to THIS worktree as uncommitted changes. The
    /// patch is validated with `git apply --check` first, so on conflict the
    /// working tree is left untouched and an error is returned.
    pub fn apply_patch_checked(&self, patch: &str) -> Result<()> {
        let work_dir = self.work_dir()?;
        let check = crate::git_cli::run_with_stdin(
            &work_dir,
            &["apply", "--check", "--binary", "--whitespace=nowarn", "-"],
            patch,
        )?;
        if !check.status.success() {
            anyhow::bail!(
                "patch does not apply cleanly: {}",
                String::from_utf8_lossy(&check.stderr).trim()
            );
        }
        let out = crate::git_cli::run_with_stdin(
            &work_dir,
            &["apply", "--binary", "--whitespace=nowarn", "-"],
            patch,
        )?;
        if !out.status.success() {
            anyhow::bail!(
                "git apply failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(())
    }

    /// Fast-forward-only merge of `refspec` into the CURRENT branch — updates
    /// ref, index, and working tree together. Errors when ff is impossible.
    pub fn merge_ff_only(&self, refspec: &str) -> Result<()> {
        let work_dir = self.work_dir()?;
        crate::git_cli::run_silent(&work_dir, &["merge", "--ff-only", refspec])
    }

    /// True when a rebase is in progress in THIS worktree (rebase-merge or
    /// rebase-apply state dir exists under the worktree's git dir).
    pub fn rebase_in_progress(&self) -> Result<bool> {
        let work_dir = self.work_dir()?;
        for state_dir in ["rebase-merge", "rebase-apply"] {
            let out = crate::git_cli::run(&work_dir, &["rev-parse", "--git-path", state_dir])?;
            let p = std::path::PathBuf::from(out.trim());
            let abs = if p.is_absolute() { p } else { work_dir.join(p) };
            if abs.exists() {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Resolve a refspec (branch name, oid, `HEAD`, ...) to a full commit oid.
    pub fn rev_parse(&self, refspec: &str) -> Result<String> {
        let work_dir = self.work_dir()?;
        let out = crate::git_cli::run(&work_dir, &["rev-parse", "--verify", refspec])?;
        Ok(out.trim().to_string())
    }

    /// Read a git config value for this repo/worktree. Returns `None` when
    /// the key is unset (or git failed — callers treat both as "not set").
    pub fn config_get(&self, key: &str) -> Option<String> {
        let work_dir = self.work_dir().ok()?;
        crate::git_cli::run(&work_dir, &["config", "--get", key])
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    /// Write a git config value (repository scope — shared across worktrees).
    pub fn config_set(&self, key: &str, value: &str) -> Result<()> {
        let work_dir = self.work_dir()?;
        crate::git_cli::run_silent(&work_dir, &["config", key, value])
    }

    /// Force-delete local branch `name` (`git branch -D`).
    pub fn delete_branch(&self, name: &str) -> Result<()> {
        let work_dir = self.work_dir()?;
        crate::git_cli::run_silent(&work_dir, &["branch", "-D", name])
    }

    /// Atomically move `refs/heads/<branch>` to `new_oid`; when `expected_old_oid`
    /// is given the update is a compare-and-swap that fails if the ref moved.
    pub fn update_branch_ref(
        &self,
        branch: &str,
        new_oid: &str,
        expected_old_oid: Option<&str>,
    ) -> Result<()> {
        let work_dir = self.work_dir()?;
        let ref_name = format!("refs/heads/{}", branch);
        let mut args: Vec<&str> = vec!["update-ref", &ref_name, new_oid];
        if let Some(old) = expected_old_oid {
            args.push(old);
        }
        crate::git_cli::run_silent(&work_dir, &args)
    }

    /// Drop stale worktree admin entries whose directories vanished
    /// (`git worktree prune`).
    pub fn prune_worktrees(&self) -> Result<()> {
        let work_dir = self.work_dir()?;
        crate::git_cli::run_silent(&work_dir, &["worktree", "prune"])
    }

    /// Best common ancestor of two commits (`git merge-base A B`).
    pub fn merge_base(&self, a: &str, b: &str) -> Result<String> {
        let work_dir = self.work_dir()?;
        let out = crate::git_cli::run(&work_dir, &["merge-base", a, b])?;
        Ok(out.trim().to_string())
    }

    /// True when commit `ancestor` is an ancestor of commit `descendant`.
    pub fn is_ancestor(&self, ancestor: &str, descendant: &str) -> Result<bool> {
        let work_dir = self.work_dir()?;
        let code = crate::git_cli::run_status(
            &work_dir,
            &["merge-base", "--is-ancestor", ancestor, descendant],
        )?;
        match code {
            0 => Ok(true),
            1 => Ok(false),
            c => anyhow::bail!("git merge-base --is-ancestor failed (exit {})", c),
        }
    }

    /// Squash everything after `base_oid` on the CURRENT branch into a single
    /// commit with `message`; returns the new commit oid. No-op guard: errors
    /// if HEAD equals `base_oid` (nothing to squash).
    pub fn squash_to_one(&self, base_oid: &str, message: &str) -> Result<String> {
        let work_dir = self.work_dir()?;
        let head = self.rev_parse("HEAD")?;
        if head == base_oid {
            anyhow::bail!("nothing to squash: HEAD is already at {}", base_oid);
        }
        crate::git_cli::run_silent(&work_dir, &["reset", "--soft", base_oid])?;
        crate::git_cli::run_silent(&work_dir, &["commit", "-m", message])?;
        self.rev_parse("HEAD")
    }

    /// Hard-reset the CURRENT branch/HEAD to `refspec` (ref, index, and
    /// working tree together).
    pub fn reset_hard(&self, refspec: &str) -> Result<()> {
        let work_dir = self.work_dir()?;
        crate::git_cli::run_silent(&work_dir, &["reset", "--hard", refspec])
    }

    // ---------- internal helpers ----------

    /// HEAD commit oid if reachable; None for unborn HEAD.
    pub(crate) fn head_oid(&self) -> Option<ObjectId> {
        self.repo
            .head()
            .ok()?
            .try_into_peeled_id()
            .ok()
            .flatten()
            .map(|id| id.detach())
    }

    /// Worktree path (where the user's files live). Errors for bare repos.
    pub(crate) fn work_dir(&self) -> Result<std::path::PathBuf> {
        self.repo
            .workdir()
            .map(|p| p.to_path_buf())
            .context("repository is bare (no working directory)")
    }
}

/// Best-effort log+drop for a branch-iter error. We don't have tracing as a
/// hard dep here; suppress for now and re-enable later if we wire one in.
fn tracing_warn_branch_iter<E: std::fmt::Debug>(_e: &E) {
    // intentionally empty; production observability lives in callers
}

/// Strip a known refs prefix off a reference name, falling back to the full
/// name when the prefix doesn't match.
fn strip_refs_prefix(name: &gix::bstr::BStr, prefix: &str) -> String {
    let s = name.to_string();
    s.strip_prefix(prefix).unwrap_or(&s).to_string()
}

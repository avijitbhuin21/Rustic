use crate::log::CommitInfo;
use crate::repo::GitRepo;
use anyhow::{Result, Context};
use git2::{Cred, FetchOptions, PushOptions, RemoteCallbacks, Sort};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct AheadBehind {
    pub ahead: usize,
    pub behind: usize,
}

fn make_callbacks(token: Option<&str>) -> RemoteCallbacks<'_> {
    let mut callbacks = RemoteCallbacks::new();
    let token = token.map(|t| t.to_string());
    callbacks.credentials(move |_url, username_from_url, allowed_types| {
        // Try SSH agent first
        if allowed_types.contains(git2::CredentialType::SSH_KEY) {
            if let Some(user) = username_from_url {
                return Cred::ssh_key_from_agent(user);
            }
        }
        // Try token-based HTTPS auth
        if allowed_types.contains(git2::CredentialType::USER_PASS_PLAINTEXT) {
            if let Some(ref tok) = token {
                return Cred::userpass_plaintext(tok, "");
            }
            // Fall back to git credential helper
            return Cred::credential_helper(
                &git2::Config::open_default().unwrap(),
                _url,
                username_from_url,
            );
        }
        Cred::default()
    });
    callbacks
}

impl GitRepo {
    pub fn push(&self, token: Option<&str>) -> Result<()> {
        let head = self.repo.head()?;
        let branch_name = head.shorthand().context("Detached HEAD cannot push")?.to_string();
        let refspec = format!("refs/heads/{}:refs/heads/{}", branch_name, branch_name);

        let mut remote = self.repo.find_remote("origin")
            .context("No 'origin' remote found")?;

        let callbacks = make_callbacks(token);
        let mut push_opts = PushOptions::new();
        push_opts.remote_callbacks(callbacks);

        remote.push(&[&refspec], Some(&mut push_opts))?;
        Ok(())
    }

    pub fn pull(&self, token: Option<&str>) -> Result<()> {
        // Fetch first
        self.fetch(token)?;

        // Then merge the tracking branch
        let head = self.repo.head()?;
        let branch_name = head.shorthand().context("Detached HEAD")?.to_string();

        let fetch_head = self.repo.find_reference("FETCH_HEAD")?;
        let fetch_commit = self.repo.reference_to_annotated_commit(&fetch_head)?;

        let (analysis, _) = self.repo.merge_analysis(&[&fetch_commit])?;

        if analysis.is_up_to_date() {
            return Ok(());
        }

        if analysis.is_fast_forward() {
            let refname = format!("refs/heads/{}", branch_name);
            let mut reference = self.repo.find_reference(&refname)?;
            reference.set_target(fetch_commit.id(), "pull: fast-forward")?;
            self.repo.set_head(&refname)?;
            self.repo.checkout_head(Some(
                git2::build::CheckoutBuilder::default().force(),
            ))?;
            return Ok(());
        }

        // Normal merge
        self.repo.merge(&[&fetch_commit], None, None)?;

        // Check for conflicts
        let index = self.repo.index()?;
        if index.has_conflicts() {
            return Err(anyhow::anyhow!("Merge conflicts detected. Resolve conflicts and commit."));
        }

        // Auto-commit the merge
        let mut index = self.repo.index()?;
        let oid = index.write_tree()?;
        let tree = self.repo.find_tree(oid)?;
        let sig = self.repo.signature()?;
        let head_commit = self.repo.head()?.peel_to_commit()?;
        let fetch_commit_obj = self.repo.find_commit(fetch_commit.id())?;
        self.repo.commit(
            Some("HEAD"),
            &sig,
            &sig,
            &format!("Merge remote-tracking branch 'origin/{}'", branch_name),
            &tree,
            &[&head_commit, &fetch_commit_obj],
        )?;
        self.repo.cleanup_state()?;

        Ok(())
    }

    pub fn fetch(&self, token: Option<&str>) -> Result<()> {
        let mut remote = self.repo.find_remote("origin")
            .context("No 'origin' remote found")?;

        let callbacks = make_callbacks(token);
        let mut fetch_opts = FetchOptions::new();
        fetch_opts.remote_callbacks(callbacks);

        remote.fetch::<&str>(&[], Some(&mut fetch_opts), None)?;
        Ok(())
    }

    /// List commits on HEAD that aren't yet on `origin/<current-branch>`.
    /// Returns an empty vec if the upstream tracking ref doesn't exist — that
    /// usually means the branch has never been pushed, in which case the UI
    /// relies on the normal Push flow to publish the branch rather than listing
    /// every commit on the branch. Caps at `max_count` to keep the UI responsive
    /// if someone accumulates a huge unpushed backlog.
    pub fn unpushed_commits(&self, max_count: usize) -> Result<Vec<CommitInfo>> {
        if !self.has_commits() {
            return Ok(Vec::new());
        }

        let head = self.repo.head()?;
        let branch_name = head.shorthand().context("Detached HEAD")?;
        let upstream_name = format!("refs/remotes/origin/{}", branch_name);

        let upstream_oid = match self.repo.find_reference(&upstream_name) {
            Ok(r) => match r.target() {
                Some(oid) => oid,
                None => return Ok(Vec::new()),
            },
            Err(_) => return Ok(Vec::new()),
        };

        let mut revwalk = self.repo.revwalk()?;
        revwalk.set_sorting(Sort::TIME)?;
        revwalk.push_head()?;
        revwalk.hide(upstream_oid)?;

        let mut commits = Vec::new();
        for (i, oid_result) in revwalk.enumerate() {
            if i >= max_count {
                break;
            }
            let oid = oid_result?;
            let commit = self.repo.find_commit(oid)?;
            let short_id = oid.to_string()[..7].to_string();
            let message = commit.message().unwrap_or("").trim().to_string();
            let author = commit.author();
            let author_name = author.name().unwrap_or("Unknown").to_string();
            let author_email = author.email().unwrap_or("").to_string();
            let timestamp = commit.time().seconds();
            let parent_count = commit.parent_count();

            commits.push(CommitInfo {
                oid: oid.to_string(),
                short_id,
                message,
                author_name,
                author_email,
                timestamp,
                parent_count,
                refs: Vec::new(),
            });
        }

        Ok(commits)
    }

    pub fn ahead_behind(&self) -> Result<AheadBehind> {
        let head = self.repo.head()?;
        let branch_name = head.shorthand().context("Detached HEAD")?;
        let local_oid = head.target().context("No HEAD target")?;

        let upstream_name = format!("refs/remotes/origin/{}", branch_name);
        let upstream_ref = self.repo.find_reference(&upstream_name);

        match upstream_ref {
            Ok(r) => {
                let upstream_oid = r.target().context("No upstream target")?;
                let (ahead, behind) = self.repo.graph_ahead_behind(local_oid, upstream_oid)?;
                Ok(AheadBehind { ahead, behind })
            }
            Err(_) => Ok(AheadBehind { ahead: 0, behind: 0 }),
        }
    }

    pub fn rebase(&self, onto_branch: &str) -> Result<()> {
        let onto_ref = self.repo.find_reference(&format!("refs/heads/{}", onto_branch))?;
        let onto_annotated = self.repo.reference_to_annotated_commit(&onto_ref)?;

        let mut rebase = self.repo.rebase(None, Some(&onto_annotated), None, None)?;
        let sig = self.repo.signature()?;

        while let Some(op) = rebase.next() {
            op?;
            let index = self.repo.index()?;
            if index.has_conflicts() {
                return Err(anyhow::anyhow!(
                    "Rebase conflict. Resolve conflicts then continue rebase."
                ));
            }
            rebase.commit(None, &sig, None)?;
        }

        rebase.finish(Some(&sig))?;
        Ok(())
    }

    pub fn rebase_continue(&self) -> Result<()> {
        let mut rebase = self.repo.open_rebase(None)?;
        let sig = self.repo.signature()?;

        // Commit current resolved step
        rebase.commit(None, &sig, None)?;

        // Continue remaining steps
        while let Some(op) = rebase.next() {
            op?;
            let index = self.repo.index()?;
            if index.has_conflicts() {
                return Err(anyhow::anyhow!(
                    "Rebase conflict. Resolve conflicts then continue rebase."
                ));
            }
            rebase.commit(None, &sig, None)?;
        }

        rebase.finish(Some(&sig))?;
        Ok(())
    }

    pub fn rebase_abort(&self) -> Result<()> {
        let mut rebase = self.repo.open_rebase(None)?;
        rebase.abort()?;
        Ok(())
    }

    pub fn add_remote(&self, name: &str, url: &str) -> Result<()> {
        self.repo.remote(name, url)?;
        Ok(())
    }

    pub fn get_remote_url(&self) -> Result<Option<String>> {
        match self.repo.find_remote("origin") {
            Ok(remote) => Ok(remote.url().map(|u| u.to_string())),
            Err(_) => Ok(None),
        }
    }
}

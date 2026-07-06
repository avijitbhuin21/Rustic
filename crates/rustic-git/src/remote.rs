use crate::log::CommitInfo;
use crate::repo::GitRepo;
use anyhow::Result;
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
pub struct AheadBehind {
    pub ahead: usize,
    pub behind: usize,
    /// `false` when the current branch has no remote tracking ref at all —
    /// meaning it has never been pushed. The UI shows "Publish Branch"
    /// instead of the normal push/pull indicators.
    pub has_upstream: bool,
}

/// Build the environment variables carrying token auth for a git subprocess.
/// The token becomes an `http.extraHeader` config entry injected via
/// `GIT_CONFIG_COUNT`/`GIT_CONFIG_KEY_n`/`GIT_CONFIG_VALUE_n` (git >= 2.31)
/// instead of `-c http.extraHeader=...` argv — command lines are visible to
/// every process on the machine (`ps`, Task Manager, WMI), environments are
/// not. Wraps in an Authorization Basic header (NOT Bearer — GitHub's git
/// smart-HTTP endpoint accepts Basic only; Bearer is for the REST API and
/// yields `remote: invalid credentials` against the git endpoint). The
/// username is `x-access-token`, the documented placeholder for GitHub
/// OAuth/PAT tokens. Returns an empty Vec when no token was supplied. The
/// header path is preferred over baking the token into the URL because
/// URL-embedded credentials end up in git's reflog and stderr.
///
/// Also disables `credential.helper` for the command when we have a token —
/// otherwise Git Credential Manager (the default `helper=manager` on Windows
/// installs of Git) will pop a native "Connect to GitHub" modal whenever the
/// server replies 401, and that modal blocks the `git.exe` subprocess
/// indefinitely (freezing Rustic since the modal is parented to it). Failing
/// fast with a 401 error is much better UX than an interactive hang inside a
/// helper Rustic doesn't control.
fn token_envs(token: Option<&str>) -> Vec<(String, String)> {
    match token {
        Some(t) if !t.is_empty() => {
            use base64::Engine;
            let encoded =
                base64::engine::general_purpose::STANDARD.encode(format!("x-access-token:{}", t));
            vec![
                ("GIT_CONFIG_COUNT".to_string(), "2".to_string()),
                (
                    "GIT_CONFIG_KEY_0".to_string(),
                    "http.extraHeader".to_string(),
                ),
                (
                    "GIT_CONFIG_VALUE_0".to_string(),
                    format!("Authorization: Basic {}", encoded),
                ),
                (
                    "GIT_CONFIG_KEY_1".to_string(),
                    "credential.helper".to_string(),
                ),
                // Empty value clears all configured helpers for this invocation.
                ("GIT_CONFIG_VALUE_1".to_string(), String::new()),
            ]
        }
        _ => Vec::new(),
    }
}

impl GitRepo {
    pub fn push(&self, token: Option<&str>) -> Result<()> {
        self.push_with_progress(token, &mut |_| {})
    }

    /// [`push`](Self::push) with live progress: `on_progress` receives git's
    /// own sideband lines ("Compressing objects: 64% …", "Writing objects: …").
    pub fn push_with_progress(
        &self,
        token: Option<&str>,
        on_progress: &mut dyn FnMut(&str),
    ) -> Result<()> {
        let branch = self.head_branch_strict()?;
        let work_dir = self.work_dir()?;
        let envs = token_envs(token);
        crate::git_cli::run_streaming_progress(
            Some(&work_dir),
            &["push", "--progress", "origin", &branch],
            &envs,
            on_progress,
        )
    }

    pub fn pull(&self, token: Option<&str>) -> Result<()> {
        self.pull_with_progress(token, &mut |_| {})
    }

    /// [`pull`](Self::pull) with live progress: receives both the network
    /// phase ("Receiving objects: 42% (12000/90000)") and the checkout phase
    /// ("Updating files: 18% (16200/90000)") — the latter is the "how many
    /// files have landed on disk" signal for huge pulls.
    pub fn pull_with_progress(
        &self,
        token: Option<&str>,
        on_progress: &mut dyn FnMut(&str),
    ) -> Result<()> {
        let branch = self.head_branch_strict()?;
        let work_dir = self.work_dir()?;
        let envs = token_envs(token);
        crate::git_cli::run_streaming_progress(
            Some(&work_dir),
            &["pull", "--progress", "origin", &branch],
            &envs,
            on_progress,
        )
    }

    pub fn fetch(&self, token: Option<&str>) -> Result<()> {
        self.fetch_with_progress(token, &mut |_| {})
    }

    /// [`fetch`](Self::fetch) with live sideband progress.
    pub fn fetch_with_progress(
        &self,
        token: Option<&str>,
        on_progress: &mut dyn FnMut(&str),
    ) -> Result<()> {
        let work_dir = self.work_dir()?;
        let envs = token_envs(token);
        crate::git_cli::run_streaming_progress(
            Some(&work_dir),
            &["fetch", "--progress", "origin"],
            &envs,
            on_progress,
        )
    }

    /// List commits on HEAD not yet on `origin/<current-branch>`. Returns an
    /// empty Vec if the upstream tracking ref doesn't exist (branch never
    /// pushed). Caps at `max_count`.
    ///
    /// Uses `git rev-list` for the HEAD ^upstream walk because gix 0.83's
    /// `Platform` doesn't expose a `hide`-equivalent that matches libgit2's
    /// semantics; rev-list is the canonical command and `find_commit` is
    /// still done through gix for the metadata read.
    pub fn unpushed_commits(&self, max_count: usize) -> Result<Vec<CommitInfo>> {
        let head_oid = match self.head_oid() {
            Some(_) => (),
            None => return Ok(Vec::new()),
        };
        let _ = head_oid; // we don't need the value, just the existence check

        let branch = self.head_branch_strict()?;
        let upstream = format!("refs/remotes/origin/{}", branch);
        // Bail with empty result if upstream doesn't exist.
        if self.repo.find_reference(upstream.as_str()).is_err() {
            return Ok(Vec::new());
        }

        let work_dir = self.work_dir()?;
        let max = max_count.to_string();
        let revspec = format!("HEAD ^{}", upstream);
        // We can't easily split "HEAD ^refs/..." into separate args because
        // both need to be in one logical revision spec; rev-list takes each
        // as its own argument:
        let out = crate::git_cli::run(
            &work_dir,
            &[
                "rev-list",
                "--max-count",
                &max,
                "HEAD",
                &format!("^{}", upstream),
            ],
        )?;
        let _ = revspec;

        let mut commits = Vec::new();
        for oid_str in out.lines().filter(|l| !l.is_empty()) {
            let oid = match gix::ObjectId::from_hex(oid_str.as_bytes()) {
                Ok(o) => o,
                Err(_) => continue,
            };
            let commit = self.repo.find_commit(oid)?;
            let short_id = oid_str.chars().take(7).collect::<String>();
            let message = commit.message_raw_sloppy().to_string().trim().to_string();
            let author = commit.author()?;
            let author_name = author.name.to_string();
            let author_email = author.email.to_string();
            let timestamp = author.time()?.seconds;
            let parent_count = commit.parent_ids().count();

            commits.push(CommitInfo {
                oid: oid_str.to_string(),
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

    /// Count commits ahead/behind upstream. Returns `has_upstream: false`
    /// when the current branch has no tracking ref.
    pub fn ahead_behind(&self) -> Result<AheadBehind> {
        let head_oid = match self.head_oid() {
            Some(o) => o,
            None => {
                return Ok(AheadBehind {
                    ahead: 0,
                    behind: 0,
                    has_upstream: false,
                })
            }
        };
        let branch = self.head_branch_strict()?;
        let upstream_name = format!("refs/remotes/origin/{}", branch);
        let upstream_oid = match self.repo.find_reference(upstream_name.as_str()) {
            Ok(r) => r
                .target()
                .try_id()
                .map(|id| id.to_owned())
                .ok_or_else(|| anyhow::anyhow!("upstream has no oid"))?,
            Err(_) => {
                return Ok(AheadBehind {
                    ahead: 0,
                    behind: 0,
                    has_upstream: false,
                });
            }
        };

        // gix exposes ahead-behind via revision::graph or revwalk with
        // pruning. Easiest: use `git rev-list --left-right --count` and
        // parse — same semantics as libgit2's graph_ahead_behind. Keeps
        // this in the CLI bucket so we don't have to manage another gix
        // graph type here.
        let work_dir = self.work_dir()?;
        let out = crate::git_cli::run(
            &work_dir,
            &[
                "rev-list",
                "--left-right",
                "--count",
                &format!("{}...{}", head_oid, upstream_oid),
            ],
        )?;
        let trimmed = out.trim();
        let mut parts = trimmed.split_whitespace();
        let ahead: usize = parts.next().unwrap_or("0").parse().unwrap_or(0);
        let behind: usize = parts.next().unwrap_or("0").parse().unwrap_or(0);
        Ok(AheadBehind {
            ahead,
            behind,
            has_upstream: true,
        })
    }

    /// Push the current branch to origin and set it as the upstream.
    pub fn publish_branch(&self, token: Option<&str>) -> Result<()> {
        self.publish_branch_with_progress(token, &mut |_| {})
    }

    /// [`publish_branch`](Self::publish_branch) with live sideband progress.
    pub fn publish_branch_with_progress(
        &self,
        token: Option<&str>,
        on_progress: &mut dyn FnMut(&str),
    ) -> Result<()> {
        let branch = self.head_branch_strict()?;
        let work_dir = self.work_dir()?;
        let envs = token_envs(token);
        crate::git_cli::run_streaming_progress(
            Some(&work_dir),
            &["push", "--progress", "--set-upstream", "origin", &branch],
            &envs,
            on_progress,
        )
    }

    pub fn rebase(&self, onto_branch: &str) -> Result<()> {
        let work_dir = self.work_dir()?;
        crate::git_cli::run_silent(&work_dir, &["rebase", onto_branch])
    }

    pub fn rebase_continue(&self) -> Result<()> {
        let work_dir = self.work_dir()?;
        // GIT_EDITOR=true so the rebase doesn't try to open an editor for
        // edit/reword steps — we want it to keep the original message.
        let mut cmd = std::process::Command::new("git");
        cmd.arg("-C")
            .arg(&work_dir)
            .args(["rebase", "--continue"])
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
            anyhow::bail!("git rebase --continue failed: {}", stderr.trim());
        }
        Ok(())
    }

    pub fn rebase_abort(&self) -> Result<()> {
        let work_dir = self.work_dir()?;
        crate::git_cli::run_silent(&work_dir, &["rebase", "--abort"])
    }

    pub fn add_remote(&self, name: &str, url: &str) -> Result<()> {
        let work_dir = self.work_dir()?;
        crate::git_cli::run_silent(&work_dir, &["remote", "add", name, url])
    }

    /// URL of the `origin` remote, or None when no such remote exists.
    pub fn get_remote_url(&self) -> Result<Option<String>> {
        match self.repo.find_remote("origin") {
            Ok(remote) => Ok(remote
                .url(gix::remote::Direction::Fetch)
                .map(|u| u.to_bstring().to_string())),
            Err(_) => Ok(None),
        }
    }

    // ---------- internal helpers ----------

    /// HEAD's branch name, or Err if HEAD is detached. Many remote
    /// operations require a non-detached HEAD because they push/pull a
    /// branch by name.
    fn head_branch_strict(&self) -> Result<String> {
        let head = self.repo.head()?;
        match head.kind {
            gix::head::Kind::Symbolic(ref r) => {
                let name = r.name.as_bstr().to_string();
                Ok(name
                    .strip_prefix("refs/heads/")
                    .unwrap_or(&name)
                    .to_string())
            }
            gix::head::Kind::Unborn(ref r) => {
                let name = r.as_bstr().to_string();
                Ok(name
                    .strip_prefix("refs/heads/")
                    .unwrap_or(&name)
                    .to_string())
            }
            gix::head::Kind::Detached { .. } => {
                anyhow::bail!("Detached HEAD has no branch name")
            }
        }
    }
}

/// Clone a remote repository into `target_dir`.
pub fn clone_repo(url: &str, target_dir: &Path, token: Option<&str>) -> Result<GitRepo> {
    clone_repo_with_progress(url, target_dir, token, &mut |_| {})
}

/// [`clone_repo`] with live sideband progress ("Receiving objects: …",
/// "Updating files: …"). Run with no `-C` since the target doesn't exist yet.
pub fn clone_repo_with_progress(
    url: &str,
    target_dir: &Path,
    token: Option<&str>,
    on_progress: &mut dyn FnMut(&str),
) -> Result<GitRepo> {
    if let Some(parent) = target_dir.parent() {
        std::fs::create_dir_all(parent).ok();
    }

    let target_str = target_dir.to_string_lossy().into_owned();
    let envs = token_envs(token);

    crate::git_cli::run_streaming_progress(
        None,
        &["clone", "--progress", url, &target_str],
        &envs,
        on_progress,
    )?;
    GitRepo::open(target_dir)
}

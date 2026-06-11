//! GitHub auto-issue-resolve (rustic-server only).
//!
//! Flow: a repo webhook (auto-created via the signed-in user's OAuth token
//! when the feature is enabled for a project) delivers `issues` /
//! `issue_comment` events to `/webhook/github`. Labeled issues are pulled
//! into `issues/issue-<n>/` inside the project, queued, and worked one at a
//! time by a dedicated fixer task per issue. Clarification questions the
//! agent raises via `ask_user` are posted as issue comments (the turn
//! suspends — zero tokens while waiting); the reporter's reply comment
//! resumes the same task. Fixes are committed locally, never pushed.

pub mod client;
pub mod ingest;
pub mod webhook;
pub mod worker;

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use rustic_app::context::AppContext;
use rustic_app::sync_ext::MutexExt;

use crate::context::ServerContext;

/// Settings-table key for [`GithubAutoConfig`].
pub const GLOBAL_CONFIG_KEY: &str = "github_auto_config";
/// Secrets account holding the webhook HMAC secret (auto-generated).
pub const WEBHOOK_SECRET_ACCOUNT: &str = "github_webhook_secret";

/// Global feature config (settings key `github_auto_config`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubAutoConfig {
    /// Master switch. Off ⇒ webhooks are acknowledged but ignored and the
    /// worker idles, regardless of per-project flags.
    #[serde(default)]
    pub enabled: bool,
    /// Public origin of this server (e.g. `https://rustic.example.com`),
    /// used to build the webhook URL GitHub calls back on.
    #[serde(default)]
    pub public_base_url: String,
    /// Only issues carrying this label are auto-processed.
    #[serde(default = "default_label")]
    pub label: String,
}

fn default_label() -> String {
    "rustic".to_string()
}

impl Default for GithubAutoConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            public_base_url: String::new(),
            label: default_label(),
        }
    }
}

/// Per-project config (settings key `github_auto_project:<project_id>`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GithubProjectConfig {
    #[serde(default)]
    pub enabled: bool,
    /// Per-issue spend cap in USD; `None` = uncapped.
    #[serde(default)]
    pub cost_cap_usd: Option<f64>,
    /// Model override for fixer tasks; `None` = project/global default.
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub provider_type: Option<String>,
    /// Repo webhook id created at enable time (deleted on disable).
    #[serde(default)]
    pub webhook_id: Option<i64>,
    /// `owner/repo` resolved from the project's `origin` remote at enable time.
    #[serde(default)]
    pub repo_full_name: Option<String>,
}

pub fn project_config_key(project_id: &str) -> String {
    format!("github_auto_project:{project_id}")
}

pub fn load_global_config(ctx: &ServerContext) -> GithubAutoConfig {
    let db = ctx.state.db.lock_safe();
    db.get_setting(GLOBAL_CONFIG_KEY)
        .ok()
        .flatten()
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default()
}

pub fn save_global_config(ctx: &ServerContext, cfg: &GithubAutoConfig) -> Result<(), String> {
    let json = serde_json::to_string(cfg).map_err(|e| e.to_string())?;
    let db = ctx.state.db.lock_safe();
    db.set_setting(GLOBAL_CONFIG_KEY, &json).map_err(|e| e.to_string())
}

pub fn load_project_config(ctx: &ServerContext, project_id: &str) -> GithubProjectConfig {
    let db = ctx.state.db.lock_safe();
    db.get_setting(&project_config_key(project_id))
        .ok()
        .flatten()
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default()
}

pub fn save_project_config(
    ctx: &ServerContext,
    project_id: &str,
    cfg: &GithubProjectConfig,
) -> Result<(), String> {
    let json = serde_json::to_string(cfg).map_err(|e| e.to_string())?;
    let db = ctx.state.db.lock_safe();
    db.set_setting(&project_config_key(project_id), &json)
        .map_err(|e| e.to_string())
}

/// The webhook HMAC secret; generated on first use and kept in the secret
/// store so it survives restarts.
pub fn webhook_secret(ctx: &ServerContext) -> Result<String, String> {
    if let Ok(Some(s)) = ctx.secrets().get(WEBHOOK_SECRET_ACCOUNT) {
        if !s.is_empty() {
            return Ok(s);
        }
    }
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    let secret = hex::encode(bytes);
    ctx.secrets()
        .set(WEBHOOK_SECRET_ACCOUNT, &secret)
        .map_err(|e| format!("failed to persist webhook secret: {e}"))?;
    Ok(secret)
}

/// Extract `owner/repo` from any of the common GitHub remote URL shapes:
/// `https://github.com/owner/repo(.git)`, `git@github.com:owner/repo(.git)`,
/// `ssh://git@github.com/owner/repo(.git)`. Non-GitHub remotes → `None`.
pub fn parse_github_remote(url: &str) -> Option<String> {
    let url = url.trim();
    let rest = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("http://github.com/"))
        .or_else(|| url.strip_prefix("git@github.com:"))
        .or_else(|| url.strip_prefix("ssh://git@github.com/"))
        .or_else(|| url.strip_prefix("git://github.com/"))?;
    let rest = rest.strip_suffix(".git").unwrap_or(rest);
    let rest = rest.trim_end_matches('/');
    let mut parts = rest.splitn(2, '/');
    let owner = parts.next()?;
    let repo = parts.next()?;
    if owner.is_empty() || repo.is_empty() || repo.contains('/') {
        return None;
    }
    Some(format!("{owner}/{repo}"))
}

/// Resolved project info for a webhook delivery.
pub struct MatchedProject {
    pub project_id: String,
    pub project_name: String,
    pub root_path: std::path::PathBuf,
}

/// Find the workspace project whose `origin` remote points at
/// `repo_full_name`. Only projects with auto-resolve enabled match.
pub fn match_project_for_repo(ctx: &ServerContext, repo_full_name: &str) -> Option<MatchedProject> {
    let projects: Vec<(String, String, std::path::PathBuf)> = {
        let ws = ctx.state.workspace.lock_safe();
        ws.list_projects()
            .into_iter()
            .map(|p| (p.id.to_string(), p.name.clone(), p.root_path.clone()))
            .collect()
    };
    for (id, name, root) in projects {
        let cfg = load_project_config(ctx, &id);
        if !cfg.enabled {
            continue;
        }
        // Prefer the repo cached at enable time; fall back to reading the
        // remote (covers projects whose remote changed after enabling).
        let repo = cfg.repo_full_name.clone().or_else(|| {
            rustic_git::GitRepo::open(&root)
                .ok()
                .and_then(|r| r.get_remote_url().ok())
                .flatten()
                .and_then(|u| parse_github_remote(&u))
        });
        if repo.as_deref() == Some(repo_full_name) {
            return Some(MatchedProject {
                project_id: id,
                project_name: name,
                root_path: root,
            });
        }
    }
    None
}

/// Render the agent's `ask_user` questions as a GitHub-comment body.
pub fn format_questions_comment(questions: &Value) -> String {
    let mut out = String::from(
        "I'm working on this issue and need a clarification before continuing:\n",
    );
    match questions.as_array() {
        Some(arr) if !arr.is_empty() => {
            for (i, q) in arr.iter().enumerate() {
                let text = q["text"].as_str().unwrap_or("(question)");
                out.push_str(&format!("\n{}. **{}**", i + 1, text));
                if let Some(opts) = q["options"].as_array() {
                    if !opts.is_empty() {
                        let list = opts
                            .iter()
                            .filter_map(|o| o.as_str())
                            .collect::<Vec<_>>()
                            .join(" / ");
                        out.push_str(&format!("\n   _Options: {list}_"));
                    }
                }
            }
        }
        _ => out.push_str(&format!("\n{questions}")),
    }
    out.push_str("\n\nReply to this comment and I'll pick it up from there.");
    out
}

/// Called from the executor thread in `agent_chat::send_message` right after
/// `run_turn` returns with a captured `ask_user` suspension, BEFORE the final
/// task status is written (so the worker never observes a terminal status
/// without the waiting_reply marker). Persists the suspension and posts the
/// questions as an issue comment. DB write comes first — if the comment post
/// fails the reply can still resume the task (e.g. a human relays it).
pub async fn handle_ask_user_suspension(
    ctx: &ServerContext,
    task_id: &str,
    susp: &rustic_agent::task::SuspendedAskUser,
) {
    let issue = {
        let db = ctx.state.db.lock_safe();
        db.get_github_issue_by_task(task_id).ok().flatten()
    };
    let Some(issue) = issue else {
        tracing::warn!(task = %task_id, "ask_user suspension on a task with no GitHub binding — ignored");
        return;
    };
    let questions_json =
        serde_json::to_string(&susp.questions).unwrap_or_else(|_| "[]".to_string());
    {
        let db = ctx.state.db.lock_safe();
        if let Err(e) =
            db.set_github_issue_pending_ask(issue.id, &susp.tool_use_id, &questions_json)
        {
            tracing::error!(task = %task_id, issue = issue.issue_number, %e, "failed to persist ask_user suspension");
        }
    }
    let body = format_questions_comment(&susp.questions);
    match client::GithubClient::from_ctx(ctx) {
        Ok(client) => {
            if let Err(e) = client
                .post_comment(&issue.repo_full_name, issue.issue_number, &body)
                .await
            {
                tracing::error!(issue = issue.issue_number, %e, "failed to post question comment");
                let db = ctx.state.db.lock_safe();
                let _ = db.set_github_issue_status(
                    issue.id,
                    "waiting_reply",
                    &format!("question comment failed to post: {e}"),
                );
            }
        }
        Err(e) => {
            tracing::error!(issue = issue.issue_number, %e, "no GitHub client for question comment");
        }
    }
    ctx.hub.publish(
        "github-issue-updated",
        serde_json::json!({ "issueId": issue.id }),
    );
}

/// Shared notifier handle: the webhook enqueues + pings, the worker wakes.
pub type GithubNotify = Arc<tokio::sync::Notify>;

#[cfg(test)]
mod tests {
    use super::parse_github_remote;

    #[test]
    fn parses_remote_shapes() {
        for url in [
            "https://github.com/me/repo.git",
            "https://github.com/me/repo",
            "git@github.com:me/repo.git",
            "ssh://git@github.com/me/repo.git",
            "https://github.com/me/repo/",
        ] {
            assert_eq!(parse_github_remote(url).as_deref(), Some("me/repo"), "url: {url}");
        }
        assert_eq!(parse_github_remote("https://gitlab.com/me/repo.git"), None);
        assert_eq!(parse_github_remote("https://github.com/onlyowner"), None);
    }
}

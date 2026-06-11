//! Minimal GitHub REST client for the auto-issue-resolve feature.
//!
//! Reuses the OAuth token from the existing GitHub sign-in (device flow in
//! `commands::git` — scope `repo user:email`, which covers issues, comments,
//! reactions and repository webhooks). No GitHub App registration needed.

use serde_json::{json, Value};

use rustic_app::sync_ext::MutexExt;

use crate::context::ServerContext;

/// Invisible marker appended to every comment the agent posts. Webhook
/// processing drops any comment containing it, which is what prevents the
/// "our own comment triggers another run" loop even though comments are
/// posted from the user's own account.
pub const AGENT_COMMENT_MARKER: &str = "<!-- rustic-agent -->";

const API: &str = "https://api.github.com";
const UA: &str = "Rustic-IDE";

pub struct GithubClient {
    token: String,
    http: reqwest::Client,
}

impl GithubClient {
    /// Build a client from the signed-in GitHub token. `Err` when the user
    /// has not connected GitHub in the Git panel.
    pub fn from_ctx(ctx: &ServerContext) -> Result<Self, String> {
        let token = ctx
            .state
            .git_token
            .lock_safe()
            .clone()
            .ok_or_else(|| "Not signed in to GitHub. Connect GitHub in the Git panel first.".to_string())?;
        Ok(Self {
            token,
            http: reqwest::Client::new(),
        })
    }

    fn req(&self, method: reqwest::Method, url: &str) -> reqwest::RequestBuilder {
        self.http
            .request(method, url)
            .header("Authorization", format!("Bearer {}", self.token))
            .header("User-Agent", UA)
            .header("Accept", "application/vnd.github+json")
    }

    async fn expect_json(resp: reqwest::Response, what: &str) -> Result<Value, String> {
        let status = resp.status();
        let body: Value = resp.json().await.unwrap_or_default();
        if !status.is_success() {
            let msg = body["message"].as_str().unwrap_or("unknown error");
            return Err(format!("GitHub {what} failed ({status}): {msg}"));
        }
        Ok(body)
    }

    /// `GET /user` — the signed-in account's login.
    pub async fn login(&self) -> Result<String, String> {
        let resp = self
            .req(reqwest::Method::GET, &format!("{API}/user"))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let body = Self::expect_json(resp, "user lookup").await?;
        Ok(body["login"].as_str().unwrap_or_default().to_string())
    }

    /// `GET /repos/{repo}/issues/{n}` — current issue state (fresher than the
    /// webhook payload by the time the worker runs).
    pub async fn get_issue(&self, repo: &str, number: i64) -> Result<Value, String> {
        let resp = self
            .req(reqwest::Method::GET, &format!("{API}/repos/{repo}/issues/{number}"))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        Self::expect_json(resp, "issue fetch").await
    }

    /// `POST /repos/{repo}/issues/{n}/comments`. The agent marker is appended
    /// automatically. Returns the new comment id.
    pub async fn post_comment(&self, repo: &str, number: i64, body: &str) -> Result<i64, String> {
        let full = format!("{body}\n\n{AGENT_COMMENT_MARKER}");
        let resp = self
            .req(
                reqwest::Method::POST,
                &format!("{API}/repos/{repo}/issues/{number}/comments"),
            )
            .json(&json!({ "body": full }))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let body = Self::expect_json(resp, "comment post").await?;
        Ok(body["id"].as_i64().unwrap_or_default())
    }

    /// `POST /repos/{repo}/issues/{n}/reactions` — `content` is one of
    /// `+1 -1 laugh confused heart hooray rocket eyes`.
    pub async fn add_reaction(&self, repo: &str, number: i64, content: &str) -> Result<(), String> {
        let resp = self
            .req(
                reqwest::Method::POST,
                &format!("{API}/repos/{repo}/issues/{number}/reactions"),
            )
            .json(&json!({ "content": content }))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        Self::expect_json(resp, "reaction").await.map(|_| ())
    }

    /// `POST /repos/{repo}/hooks` — create the issues/issue_comment webhook
    /// pointing at this server. Returns the hook id (kept for cleanup).
    /// GitHub rejects duplicates with 422 "Hook already exists" — surfaced
    /// verbatim so the settings UI can explain.
    pub async fn create_webhook(
        &self,
        repo: &str,
        webhook_url: &str,
        secret: &str,
    ) -> Result<i64, String> {
        let resp = self
            .req(reqwest::Method::POST, &format!("{API}/repos/{repo}/hooks"))
            .json(&json!({
                "name": "web",
                "active": true,
                "events": ["issues", "issue_comment"],
                "config": {
                    "url": webhook_url,
                    "content_type": "json",
                    "secret": secret,
                    "insecure_ssl": "0",
                }
            }))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let body = Self::expect_json(resp, "webhook create").await?;
        body["id"]
            .as_i64()
            .ok_or_else(|| "GitHub webhook response missing id".to_string())
    }

    /// `DELETE /repos/{repo}/hooks/{id}`. 404 is treated as success (already
    /// deleted on GitHub's side).
    pub async fn delete_webhook(&self, repo: &str, hook_id: i64) -> Result<(), String> {
        let resp = self
            .req(
                reqwest::Method::DELETE,
                &format!("{API}/repos/{repo}/hooks/{hook_id}"),
            )
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if resp.status().is_success() || resp.status() == reqwest::StatusCode::NOT_FOUND {
            Ok(())
        } else {
            Err(format!("GitHub webhook delete failed ({})", resp.status()))
        }
    }

    /// Download an issue attachment (user-images / user-attachments URL).
    /// Returns `(bytes, content_type)`. The token rides along so private-repo
    /// attachments resolve; GitHub redirects to signed storage URLs, which
    /// reqwest follows by default.
    pub async fn download(&self, url: &str) -> Result<(Vec<u8>, String), String> {
        let resp = self
            .http
            .get(url)
            .header("Authorization", format!("Bearer {}", self.token))
            .header("User-Agent", UA)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("attachment download failed ({})", resp.status()));
        }
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/octet-stream")
            .split(';')
            .next()
            .unwrap_or("application/octet-stream")
            .to_string();
        let bytes = resp.bytes().await.map_err(|e| e.to_string())?.to_vec();
        Ok((bytes, content_type))
    }
}

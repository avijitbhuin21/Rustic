//! `/webhook/github` — unauthenticated route whose auth is the HMAC-SHA256
//! delivery signature (`X-Hub-Signature-256`) computed over the raw body with
//! the per-server webhook secret.

use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;

use rustic_app::sync_ext::MutexExt;

use crate::app::Shared;
use crate::context::ServerContext;
use crate::github::{self, client::AGENT_COMMENT_MARKER};

/// Constant-time signature check of `sha256=<hex>` against the body HMAC.
fn signature_valid(secret: &str, body: &[u8], header: Option<&str>) -> bool {
    let Some(header) = header else { return false };
    let Some(hex_sig) = header.strip_prefix("sha256=") else {
        return false;
    };
    let Ok(expected_sig) = hex::decode(hex_sig) else {
        return false;
    };
    let mut mac = match Hmac::<Sha256>::new_from_slice(secret.as_bytes()) {
        Ok(m) => m,
        Err(_) => return false,
    };
    mac.update(body);
    mac.verify_slice(&expected_sig).is_ok()
}

const RECENT_DELIVERY_CAP: usize = 2048;

/// Records a delivery GUID; returns false when it was already seen (a replay).
fn first_delivery(id: &str) -> bool {
    use std::collections::{HashSet, VecDeque};
    use std::sync::{Mutex, OnceLock};
    static SEEN: OnceLock<Mutex<(VecDeque<String>, HashSet<String>)>> = OnceLock::new();
    let lock = SEEN.get_or_init(|| Mutex::new((VecDeque::new(), HashSet::new())));
    let mut guard = lock.lock().unwrap_or_else(|p| p.into_inner());
    let (order, set) = &mut *guard;
    if !set.insert(id.to_string()) {
        return false;
    }
    order.push_back(id.to_string());
    if order.len() > RECENT_DELIVERY_CAP {
        if let Some(old) = order.pop_front() {
            set.remove(&old);
        }
    }
    true
}

pub async fn github_webhook(
    State(shared): State<Arc<Shared>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let ctx = &shared.ctx;

    let secret = match github::webhook_secret(ctx) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(%e, "webhook secret unavailable");
            return (StatusCode::INTERNAL_SERVER_ERROR, "secret unavailable").into_response();
        }
    };
    let sig = headers
        .get("x-hub-signature-256")
        .and_then(|v| v.to_str().ok());
    if !signature_valid(&secret, &body, sig) {
        tracing::warn!("github webhook: signature mismatch — dropping delivery");
        return (StatusCode::UNAUTHORIZED, "bad signature").into_response();
    }
    if let Some(delivery) = headers
        .get("x-github-delivery")
        .and_then(|v| v.to_str().ok())
        .filter(|s| !s.is_empty())
    {
        if !first_delivery(delivery) {
            tracing::warn!(%delivery, "github webhook: duplicate delivery id — ignoring replay");
            return Json(json!({ "ok": true, "outcome": "duplicate-delivery" })).into_response();
        }
    }

    let event = headers
        .get("x-github-event")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let payload: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid json").into_response(),
    };

    match process_event(ctx, &event, &payload) {
        Ok(outcome) => Json(json!({ "ok": true, "outcome": outcome })).into_response(),
        Err(e) => {
            tracing::error!(event = %event, %e, "github webhook processing failed");
            // Still 200 — GitHub retries non-2xx and a retry won't fix a
            // processing bug; the error is in our logs.
            Json(json!({ "ok": false, "error": e })).into_response()
        }
    }
}

/// Decide what (if anything) a delivery means and enqueue work for the
/// issue worker. Returns a short outcome string for logging/response.
fn process_event(
    ctx: &ServerContext,
    event: &str,
    payload: &Value,
) -> Result<&'static str, String> {
    let cfg = github::load_global_config(ctx);
    if !cfg.enabled {
        return Ok("disabled");
    }
    if event == "ping" {
        return Ok("pong");
    }
    // Never act on our own (or any bot's) activity.
    if payload["sender"]["type"].as_str() == Some("Bot") {
        return Ok("ignored-bot");
    }

    let repo = payload["repository"]["full_name"]
        .as_str()
        .ok_or("missing repository.full_name")?
        .to_string();
    let issue = &payload["issue"];
    let number = issue["number"].as_i64().ok_or("missing issue.number")?;
    let action = payload["action"].as_str().unwrap_or("");

    let outcome = match event {
        "issues" => {
            if !matches!(action, "opened" | "edited" | "reopened" | "labeled") {
                return Ok("ignored-action");
            }
            // Label gate: only issues carrying the configured label.
            let has_label = issue["labels"]
                .as_array()
                .map(|arr| {
                    arr.iter().any(|l| {
                        l["name"]
                            .as_str()
                            .map(|n| n.eq_ignore_ascii_case(&cfg.label))
                            .unwrap_or(false)
                    })
                })
                .unwrap_or(false);
            if !has_label {
                return Ok("ignored-unlabeled");
            }
            let Some(project) = github::match_project_for_repo(ctx, &repo) else {
                return Ok("ignored-no-project");
            };
            let title = issue["title"].as_str().unwrap_or("").to_string();
            let url = issue["html_url"].as_str().unwrap_or("").to_string();

            let existing = {
                let db = ctx.state.db.lock_safe();
                db.get_github_issue_by_number(&repo, number)
                    .map_err(|e| e.to_string())?
            };
            match existing {
                None => {
                    let project_cfg = github::load_project_config(ctx, &project.project_id);
                    let db = ctx.state.db.lock_safe();
                    let issue_id = db
                        .upsert_github_issue(
                            &project.project_id,
                            &repo,
                            number,
                            &title,
                            &url,
                            project_cfg.cost_cap_usd,
                        )
                        .map_err(|e| e.to_string())?;
                    db.enqueue_github_event(issue_id, "new_issue", "{}")
                        .map_err(|e| e.to_string())?;
                    "queued-new"
                }
                Some(row) => {
                    // Refresh title/url; then decide whether this is news.
                    {
                        let db = ctx.state.db.lock_safe();
                        let _ = db.upsert_github_issue(
                            &row.project_id,
                            &repo,
                            number,
                            &title,
                            &url,
                            row.cost_cap_usd,
                        );
                    }
                    let is_news = match action {
                        // The label approved the issue as it stood. A later
                        // edit or reopen is acted on only from a trusted account.
                        "edited" | "reopened" => {
                            let (login, association) = issue_event_actor(payload);
                            if !cfg.trusts(login, association) {
                                tracing::info!(
                                    repo = %repo, number, action, login,
                                    "github webhook: issue change by an untrusted account ignored"
                                );
                                return Ok("ignored-untrusted-author");
                            }
                            true
                        }
                        // Re-adding the label to a finished issue = "run again".
                        "labeled" => matches!(row.status.as_str(), "done" | "failed" | "manual"),
                        _ => false,
                    };
                    if is_news {
                        let payload_json = json!({ "action": action }).to_string();
                        let db = ctx.state.db.lock_safe();
                        db.enqueue_github_event(row.id, "issue_update", &payload_json)
                            .map_err(|e| e.to_string())?;
                        "queued-update"
                    } else {
                        "ignored-known"
                    }
                }
            }
        }
        "issue_comment" => {
            if action != "created" {
                return Ok("ignored-action");
            }
            // issue_comment fires for pull requests too — those are not ours.
            if !issue["pull_request"].is_null() {
                return Ok("ignored-pr");
            }
            let body = payload["comment"]["body"].as_str().unwrap_or("");
            if body.contains(AGENT_COMMENT_MARKER) {
                return Ok("ignored-own-comment");
            }
            let existing = {
                let db = ctx.state.db.lock_safe();
                db.get_github_issue_by_number(&repo, number)
                    .map_err(|e| e.to_string())?
            };
            let Some(row) = existing else {
                return Ok("ignored-untracked");
            };
            // A comment becomes the agent's ask_user answer or extra context on
            // an unattended FullAuto task, so only trusted accounts get through.
            let login = payload["comment"]["user"]["login"].as_str().unwrap_or("");
            let association = payload["comment"]["author_association"]
                .as_str()
                .unwrap_or("");
            if !cfg.trusts(login, association) {
                tracing::info!(
                    repo = %repo, number, login, association,
                    "github webhook: comment by an untrusted account ignored"
                );
                return Ok("ignored-untrusted-author");
            }
            let payload_json = json!({
                "body": body,
                "author": if login.is_empty() { "unknown" } else { login },
                "association": association,
                "comment_id": payload["comment"]["id"].as_i64().unwrap_or(0),
            })
            .to_string();
            let db = ctx.state.db.lock_safe();
            db.enqueue_github_event(row.id, "comment", &payload_json)
                .map_err(|e| e.to_string())?;
            "queued-comment"
        }
        _ => return Ok("ignored-event"),
    };

    ctx.github_notify.notify_one();
    ctx.hub.publish("github-queue-changed", json!({}));
    Ok(outcome)
}

/// Login and repo association of whoever triggered an `issues` event.
///
/// The payload carries an association only for the issue's author. GitHub lets
/// nobody else edit or reopen an issue without triage access to the repo, so a
/// sender who is not the author is reported as a collaborator.
fn issue_event_actor(payload: &Value) -> (&str, &str) {
    let sender = payload["sender"]["login"].as_str().unwrap_or("");
    let author = payload["issue"]["user"]["login"].as_str().unwrap_or("");
    if sender.is_empty() {
        return ("", "");
    }
    if sender.eq_ignore_ascii_case(author) {
        let association = payload["issue"]["author_association"]
            .as_str()
            .unwrap_or("");
        (sender, association)
    } else {
        (sender, "COLLABORATOR")
    }
}

#[cfg(test)]
mod tests {
    use super::{issue_event_actor, signature_valid};
    use crate::github::{normalize_logins, GithubAutoConfig};
    use hmac::{Hmac, Mac};
    use serde_json::json;
    use sha2::Sha256;

    #[test]
    fn default_rule_trusts_repo_roles_only() {
        let cfg = GithubAutoConfig::default();
        for association in ["OWNER", "MEMBER", "COLLABORATOR", "owner"] {
            assert!(cfg.trusts("someone", association), "{association}");
        }
        for association in [
            "CONTRIBUTOR",
            "FIRST_TIME_CONTRIBUTOR",
            "FIRST_TIMER",
            "MANNEQUIN",
            "NONE",
            "",
        ] {
            assert!(!cfg.trusts("someone", association), "{association:?}");
        }
    }

    #[test]
    fn a_login_allowlist_replaces_the_role_rule() {
        let cfg = GithubAutoConfig {
            allowed_logins: normalize_logins(&[
                " @Alice ".to_string(),
                "alice".to_string(),
                String::new(),
                "bob".to_string(),
            ]),
            ..GithubAutoConfig::default()
        };
        assert_eq!(
            cfg.allowed_logins,
            vec!["Alice".to_string(), "bob".to_string()]
        );
        assert!(cfg.trusts("alice", "NONE"));
        assert!(cfg.trusts("BOB", ""));
        assert!(!cfg.trusts("carol", "OWNER"));
        assert!(!cfg.trusts("", "OWNER"));
    }

    #[test]
    fn issue_edits_use_the_authors_association_only_for_the_author() {
        let by_author = json!({
            "sender": { "login": "reporter" },
            "issue": { "user": { "login": "Reporter" }, "author_association": "NONE" },
        });
        assert_eq!(issue_event_actor(&by_author), ("reporter", "NONE"));
        assert!(!GithubAutoConfig::default().trusts("reporter", "NONE"));

        let by_maintainer = json!({
            "sender": { "login": "maintainer" },
            "issue": { "user": { "login": "reporter" }, "author_association": "NONE" },
        });
        let (login, association) = issue_event_actor(&by_maintainer);
        assert!(GithubAutoConfig::default().trusts(login, association));

        assert_eq!(issue_event_actor(&json!({})), ("", ""));
        assert!(!GithubAutoConfig::default().trusts("", ""));
    }

    #[test]
    fn signature_roundtrip() {
        let secret = "topsecret";
        let body = br#"{"zen":"Design for failure."}"#;
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        let sig = format!("sha256={}", hex::encode(mac.finalize().into_bytes()));
        assert!(signature_valid(secret, body, Some(&sig)));
        assert!(!signature_valid(secret, body, Some("sha256=deadbeef")));
        assert!(!signature_valid(secret, b"tampered", Some(&sig)));
        assert!(!signature_valid(secret, body, None));
    }

    #[test]
    fn duplicate_delivery_ids_are_rejected() {
        let id = "test-delivery-7f3a";
        assert!(super::first_delivery(id));
        assert!(!super::first_delivery(id));
        assert!(super::first_delivery("test-delivery-other"));
    }
}

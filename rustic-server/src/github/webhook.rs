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
fn process_event(ctx: &ServerContext, event: &str, payload: &Value) -> Result<&'static str, String> {
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
                    arr.iter()
                        .any(|l| l["name"].as_str().map(|n| n.eq_ignore_ascii_case(&cfg.label)).unwrap_or(false))
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
                        "edited" | "reopened" => true,
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
            let payload_json = json!({
                "body": body,
                "author": payload["comment"]["user"]["login"].as_str().unwrap_or("unknown"),
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

#[cfg(test)]
mod tests {
    use super::signature_valid;
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

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
}

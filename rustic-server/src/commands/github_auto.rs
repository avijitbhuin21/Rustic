//! Commands for the GitHub auto-issue-resolve feature: global + per-project
//! config (incl. webhook lifecycle) and the queue panel listing.

use serde::Deserialize;
use serde_json::{json, Value};

use rustic_app::sync_ext::MutexExt;

use crate::api::{ok, parse, ApiError};
use crate::context::ServerContext;
use crate::github::{self, client::GithubClient};

pub async fn dispatch(
    ctx: &ServerContext,
    command: &str,
    args: &Value,
) -> Option<Result<Value, ApiError>> {
    Some(match command {
        "github_auto_get_config" => get_config(ctx),
        "github_auto_set_config" => set_config(ctx, args),
        "github_auto_get_project_config" => get_project_config(ctx, args),
        "github_auto_set_project_config" => set_project_config(ctx, args).await,
        "github_auto_list_issues" => list_issues(ctx, args),
        _ => return None,
    })
}

fn get_config(ctx: &ServerContext) -> Result<Value, ApiError> {
    let cfg = github::load_global_config(ctx);
    let signed_in = ctx.state.git_token.lock_safe().is_some();
    ok(json!({
        "config": cfg,
        "signedIn": signed_in,
        "webhookPath": "/webhook/github",
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetConfigArg {
    enabled: bool,
    public_base_url: String,
    label: String,
}

fn set_config(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: SetConfigArg = parse(args)?;
    let label = a.label.trim().to_string();
    if label.is_empty() {
        return Err(ApiError::bad("Label cannot be empty"));
    }
    let cfg = github::GithubAutoConfig {
        enabled: a.enabled,
        public_base_url: a.public_base_url.trim().trim_end_matches('/').to_string(),
        label,
    };
    github::save_global_config(ctx, &cfg)?;
    if cfg.enabled {
        ctx.github_notify.notify_one();
    }
    ok(cfg)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectArg {
    project_id: String,
}

fn get_project_config(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: ProjectArg = parse(args)?;
    let cfg = github::load_project_config(ctx, &a.project_id);
    // Surface the repo the project's origin remote points at, so the UI can
    // show what would be wired up before the user enables anything.
    let detected_repo = crate::api::project_root(ctx, &a.project_id)
        .ok()
        .and_then(|root| rustic_git::GitRepo::open(std::path::Path::new(&root)).ok())
        .and_then(|r| r.get_remote_url().ok())
        .flatten()
        .and_then(|u| github::parse_github_remote(&u));
    ok(json!({ "config": cfg, "detectedRepo": detected_repo }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetProjectConfigArg {
    project_id: String,
    enabled: bool,
    cost_cap_usd: Option<f64>,
    model: Option<String>,
    provider_type: Option<String>,
}

/// Save per-project config. Enabling creates the repo webhook through the
/// signed-in user's token (idempotent: GitHub's "Hook already exists" is
/// treated as success); disabling deletes it best-effort.
async fn set_project_config(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: SetProjectConfigArg = parse(args)?;
    let mut cfg = github::load_project_config(ctx, &a.project_id);
    let was_enabled = cfg.enabled;
    cfg.cost_cap_usd = a.cost_cap_usd.filter(|c| *c > 0.0);
    cfg.model = a.model.filter(|m| !m.trim().is_empty());
    cfg.provider_type = a.provider_type.filter(|p| !p.trim().is_empty());
    cfg.enabled = a.enabled;

    if a.enabled && !was_enabled {
        let global = github::load_global_config(ctx);
        if global.public_base_url.is_empty() {
            return Err(ApiError::bad(
                "Set the server's public URL in the GitHub auto-resolve settings first \
                 (GitHub needs it to deliver webhooks).",
            ));
        }
        let root = crate::api::project_root(ctx, &a.project_id)?;
        let repo = rustic_git::GitRepo::open(std::path::Path::new(&root))
            .map_err(|e| ApiError::bad(format!("not a git repository: {e}")))?
            .get_remote_url()
            .map_err(|e| ApiError::bad(e.to_string()))?
            .and_then(|u| github::parse_github_remote(&u))
            .ok_or_else(|| {
                ApiError::bad("This project's `origin` remote does not point at GitHub.")
            })?;
        let client = GithubClient::from_ctx(ctx).map_err(ApiError::bad)?;
        let secret = github::webhook_secret(ctx).map_err(ApiError::bad)?;
        let webhook_url = format!("{}/webhook/github", global.public_base_url);
        match client.create_webhook(&repo, &webhook_url, &secret).await {
            Ok(id) => cfg.webhook_id = Some(id),
            Err(e) if e.contains("already exists") => {
                tracing::info!(repo = %repo, "webhook already exists — reusing");
                cfg.webhook_id = None;
            }
            Err(e) => return Err(ApiError::bad(e)),
        }
        cfg.repo_full_name = Some(repo);
    }

    if !a.enabled && was_enabled {
        if let (Some(repo), Some(hook_id)) = (cfg.repo_full_name.clone(), cfg.webhook_id) {
            if let Ok(client) = GithubClient::from_ctx(ctx) {
                if let Err(e) = client.delete_webhook(&repo, hook_id).await {
                    tracing::warn!(repo = %repo, hook_id, %e, "webhook delete failed (left in place)");
                }
            }
        }
        cfg.webhook_id = None;
    }

    github::save_project_config(ctx, &a.project_id, &cfg)?;
    ok(cfg)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListIssuesArg {
    project_id: Option<String>,
}

fn list_issues(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: ListIssuesArg = parse(args)?;
    let db = ctx.state.db.lock_safe();
    let rows = db
        .list_github_issues(a.project_id.as_deref())
        .map_err(|e| e.to_string())?;
    ok(rows)
}

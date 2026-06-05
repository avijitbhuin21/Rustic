//! Git commands. Read ops resolve the project root from the workspace, open a
//! `rustic_git::GitRepo`, and call the same methods the desktop bodies use.
//!
//! State-mutating ops mirror the desktop `#[tauri::command]` bodies in
//! `src-tauri/src/commands/git.rs` exactly, calling the same `rustic_git`
//! methods. The GitHub token lives in `AppState::git_token` (in-memory) and is
//! persisted through `ctx.secrets()` under the same account name desktop uses
//! (`rustic_app::bootstrap::GIT_TOKEN_ACCOUNT`), so the token resolves across
//! both transports.
//!
//! Two desktop commands gate a destructive change behind a native confirmation
//! dialog (`git_set_token`, `git_add_remote`). The server is headless and has
//! no UI to prompt from; the dialog is a desktop-only anti-XSS measure, so the
//! server performs the underlying persistence/remote change directly.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use rustic_app::bootstrap::GIT_TOKEN_ACCOUNT;
use rustic_app::context::AppContext;
use rustic_app::sync_ext::MutexExt;
use rustic_git::GitRepo;

use crate::api::{ok, parse, project_root, ApiError, ProjectArg, ProjectPathArg};
use crate::context::ServerContext;

// ---- local arg structs (camelCase wire format, snake_case fields) ----

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectPathsArg {
    project_id: String,
    paths: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectMessageArg {
    project_id: String,
    message: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectBranchArg {
    project_id: String,
    branch: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectCreateBranchArg {
    project_id: String,
    branch: String,
    checkout: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectRebaseArg {
    project_id: String,
    onto_branch: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResolveConflictArg {
    project_id: String,
    path: String,
    side: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetTokenArg {
    token: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddRemoteArg {
    project_id: String,
    name: String,
    url: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GitignoreArg {
    project_id: String,
    pattern: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LogArg {
    project_id: String,
    max_count: Option<usize>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommitFileDiffArg {
    project_id: String,
    oid: String,
    path: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommitFilesArg {
    project_id: String,
    oid: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CloneArg {
    url: String,
    target_dir: Option<String>,
}

pub async fn dispatch(
    ctx: &ServerContext,
    command: &str,
    args: &Value,
) -> Option<Result<Value, ApiError>> {
    Some(match command {
        // ── seeded read ops ───────────────────────────────────────────
        "git_check_available" => ok(serde_json::json!({ "available": true, "message": null })),
        "git_is_repo" => match parse::<ProjectArg>(args) {
            Ok(a) => match project_root(ctx, &a.project_id) {
                Ok(root) => ok(GitRepo::open(Path::new(&root)).is_ok()),
                Err(e) => Err(e),
            },
            Err(e) => Err(e),
        },
        "git_status" => with_repo(ctx, args, |repo| ok(repo.status().map_err(|e| e.to_string())?)),
        "git_branches" => with_repo(ctx, args, |repo| ok(repo.branches().map_err(|e| e.to_string())?)),
        "git_diff_staged" => with_repo(ctx, args, |repo| ok(repo.diff_staged().map_err(|e| e.to_string())?)),
        "git_diff" => git_diff(ctx, args),

        // ── staging / commit / discard ────────────────────────────────
        "git_stage" => git_stage(ctx, args),
        "git_unstage" => git_unstage(ctx, args),
        "git_commit" => git_commit(ctx, args),
        "git_discard" => git_discard(ctx, args),

        // ── init ──────────────────────────────────────────────────────
        "git_init" => git_init(ctx, args),

        // ── remote sync (token-backed) ────────────────────────────────
        "git_push" => git_push(ctx, args),
        "git_publish_branch" => git_publish_branch(ctx, args),
        "git_pull" => git_pull(ctx, args),
        "git_fetch" => git_fetch(ctx, args),
        "git_ahead_behind" => with_repo(ctx, args, |repo| ok(repo.ahead_behind().map_err(|e| e.to_string())?)),

        // ── branch ops ────────────────────────────────────────────────
        "git_checkout_branch" => git_checkout_branch(ctx, args),
        "git_create_branch" => git_create_branch(ctx, args),

        // ── rebase / conflicts / merge ────────────────────────────────
        "git_rebase" => git_rebase(ctx, args),
        "git_rebase_continue" => with_repo(ctx, args, |repo| {
            repo.rebase_continue().map_err(|e| e.to_string())?;
            ok(json!(null))
        }),
        "git_rebase_abort" => with_repo(ctx, args, |repo| {
            repo.rebase_abort().map_err(|e| e.to_string())?;
            ok(json!(null))
        }),
        "git_get_conflicts" => with_repo(ctx, args, |repo| ok(repo.get_conflicts().map_err(|e| e.to_string())?)),
        "git_resolve_conflict" => git_resolve_conflict(ctx, args),
        "git_merge_commit" => with_repo(ctx, args, |repo| ok(repo.merge_commit().map_err(|e| e.to_string())?)),

        // ── token / remotes ───────────────────────────────────────────
        "git_set_token" => git_set_token(ctx, args),
        "git_get_token" => git_get_token(ctx),
        "git_add_remote" => git_add_remote(ctx, args),
        "git_get_remote_url" => with_repo(ctx, args, |repo| ok(repo.get_remote_url().map_err(|e| e.to_string())?)),

        // ── gitignore ─────────────────────────────────────────────────
        "git_add_to_gitignore" => git_add_to_gitignore(ctx, args),

        // ── log / history ─────────────────────────────────────────────
        "git_log" => git_log(ctx, args),
        "git_commit_file_diff" => git_commit_file_diff(ctx, args),
        "git_commit_files" => git_commit_files(ctx, args),
        "git_unpushed_commits" => git_unpushed_commits(ctx, args),
        "git_undo_last_commit" => with_repo(ctx, args, |repo| {
            repo.undo_last_commit().map_err(|e| e.to_string())?;
            ok(json!(null))
        }),

        // ── projects dir / clone ──────────────────────────────────────
        "get_default_projects_dir" => get_default_projects_dir(ctx),
        "git_clone" => git_clone(ctx, args).await,

        // ── GitHub REST / OAuth device flow ───────────────────────────
        "github_create_repo" => github_create_repo(ctx, args).await,
        "github_device_code" => github_device_code().await,
        "github_poll_token" => github_poll_token(ctx, args).await,
        "github_get_user" => github_get_user(ctx).await,

        _ => return None,
    })
}

// ── staging / commit / discard ────────────────────────────────────────

fn git_stage(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: ProjectPathsArg = parse(args)?;
    let root = project_root(ctx, &a.project_id)?;
    let repo = GitRepo::open(Path::new(&root)).map_err(|e| e.to_string())?;
    ok(repo.stage(&a.paths).map_err(|e| e.to_string())?)
}

fn git_unstage(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: ProjectPathsArg = parse(args)?;
    let root = project_root(ctx, &a.project_id)?;
    let repo = GitRepo::open(Path::new(&root)).map_err(|e| e.to_string())?;
    repo.unstage(&a.paths).map_err(|e| e.to_string())?;
    ok(json!(null))
}

fn git_commit(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: ProjectMessageArg = parse(args)?;
    let root = project_root(ctx, &a.project_id)?;
    let repo = GitRepo::open(Path::new(&root)).map_err(|e| e.to_string())?;
    ok(repo.commit(&a.message).map_err(|e| e.to_string())?)
}

fn git_discard(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: ProjectPathsArg = parse(args)?;
    let root = project_root(ctx, &a.project_id)?;
    let repo = GitRepo::open(Path::new(&root)).map_err(|e| e.to_string())?;
    repo.discard_changes(&a.paths).map_err(|e| e.to_string())?;
    ok(json!(null))
}

// ── init ────────────────────────────────────────────────────────────────

fn git_init(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: ProjectArg = parse(args)?;
    let root = project_root(ctx, &a.project_id)?;
    GitRepo::init(Path::new(&root)).map_err(|e| e.to_string())?;
    ok(json!(null))
}

// ── remote sync (token-backed) ────────────────────────────────────────────

/// In-memory git token, identical access to the desktop `get_stored_token`.
fn stored_token(ctx: &ServerContext) -> Option<String> {
    ctx.state().git_token.lock_safe().clone()
}

fn git_push(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: ProjectArg = parse(args)?;
    let root = project_root(ctx, &a.project_id)?;
    let repo = GitRepo::open(Path::new(&root)).map_err(|e| e.to_string())?;
    let token = stored_token(ctx);
    repo.push(token.as_deref()).map_err(|e| e.to_string())?;
    ok(json!(null))
}

fn git_publish_branch(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: ProjectArg = parse(args)?;
    let root = project_root(ctx, &a.project_id)?;
    let repo = GitRepo::open(Path::new(&root)).map_err(|e| e.to_string())?;
    let token = stored_token(ctx);
    repo.publish_branch(token.as_deref()).map_err(|e| e.to_string())?;
    ok(json!(null))
}

fn git_pull(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: ProjectArg = parse(args)?;
    let root = project_root(ctx, &a.project_id)?;
    let repo = GitRepo::open(Path::new(&root)).map_err(|e| e.to_string())?;
    let token = stored_token(ctx);
    repo.pull(token.as_deref()).map_err(|e| e.to_string())?;
    ok(json!(null))
}

fn git_fetch(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: ProjectArg = parse(args)?;
    let root = project_root(ctx, &a.project_id)?;
    let repo = GitRepo::open(Path::new(&root)).map_err(|e| e.to_string())?;
    let token = stored_token(ctx);
    repo.fetch(token.as_deref()).map_err(|e| e.to_string())?;
    ok(json!(null))
}

// ── branch ops ──────────────────────────────────────────────────────────

fn git_checkout_branch(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: ProjectBranchArg = parse(args)?;
    let root = project_root(ctx, &a.project_id)?;
    let repo = GitRepo::open(Path::new(&root)).map_err(|e| e.to_string())?;
    repo.checkout_branch(&a.branch).map_err(|e| e.to_string())?;
    ok(json!(null))
}

fn git_create_branch(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: ProjectCreateBranchArg = parse(args)?;
    let root = project_root(ctx, &a.project_id)?;
    let repo = GitRepo::open(Path::new(&root)).map_err(|e| e.to_string())?;
    repo.create_branch(&a.branch, a.checkout).map_err(|e| e.to_string())?;
    ok(json!(null))
}

// ── rebase / conflicts / merge ────────────────────────────────────────────

fn git_rebase(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: ProjectRebaseArg = parse(args)?;
    let root = project_root(ctx, &a.project_id)?;
    let repo = GitRepo::open(Path::new(&root)).map_err(|e| e.to_string())?;
    repo.rebase(&a.onto_branch).map_err(|e| e.to_string())?;
    ok(json!(null))
}

fn git_resolve_conflict(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: ResolveConflictArg = parse(args)?;
    let root = project_root(ctx, &a.project_id)?;
    let repo = GitRepo::open(Path::new(&root)).map_err(|e| e.to_string())?;
    repo.resolve_conflict_side(&a.path, &a.side).map_err(|e| e.to_string())?;
    ok(json!(null))
}

// ── token / remotes ───────────────────────────────────────────────────────

/// Store (or clear) the GitHub token. The desktop body gates a non-empty set
/// behind a native confirmation dialog as an anti-XSS measure; the server is
/// headless with no UI to prompt from, so it performs the persistence directly.
/// The in-memory cache and the secret store are kept in sync exactly as desktop.
fn git_set_token(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: SetTokenArg = parse(args)?;
    if a.token.is_empty() {
        *ctx.state().git_token.lock_safe() = None;
        let _ = ctx.secrets().delete(GIT_TOKEN_ACCOUNT);
        return ok(json!(null));
    }
    {
        let mut stored = ctx.state().git_token.lock_safe();
        *stored = Some(a.token.clone());
    }
    if let Err(e) = ctx.secrets().set(GIT_TOKEN_ACCOUNT, &a.token) {
        tracing::warn!(error = %e, "git_set_token: secret store set failed; token kept in memory only");
    }
    ok(json!(null))
}

fn git_get_token(ctx: &ServerContext) -> Result<Value, ApiError> {
    let stored = ctx.state().git_token.lock_safe();
    ok(stored.is_some())
}

/// Add a git remote. The desktop body confirms via a native dialog before
/// repointing an existing remote (anti-XSS); the server is headless, so it
/// applies the change directly via the same `rustic_git` method.
fn git_add_remote(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: AddRemoteArg = parse(args)?;
    let root = project_root(ctx, &a.project_id)?;
    let repo = GitRepo::open(Path::new(&root)).map_err(|e| e.to_string())?;
    repo.add_remote(&a.name, &a.url).map_err(|e| e.to_string())?;
    ok(json!(null))
}

// ── gitignore ──────────────────────────────────────────────────────────────

fn git_add_to_gitignore(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    use std::io::Write;

    let a: GitignoreArg = parse(args)?;
    let root = project_root(ctx, &a.project_id)?;
    let gitignore_path = Path::new(&root).join(".gitignore");

    let existing = std::fs::read_to_string(&gitignore_path).unwrap_or_default();

    if existing.lines().any(|line| line.trim() == a.pattern.trim()) {
        return ok(json!(null));
    }

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&gitignore_path)
        .map_err(|e| e.to_string())?;

    if !existing.is_empty() && !existing.ends_with('\n') {
        writeln!(file).map_err(|e| e.to_string())?;
    }
    writeln!(file, "{}", a.pattern).map_err(|e| e.to_string())?;

    ok(json!(null))
}

// ── log / history ────────────────────────────────────────────────────────

fn git_log(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: LogArg = parse(args)?;
    let root = project_root(ctx, &a.project_id)?;
    let repo = GitRepo::open(Path::new(&root)).map_err(|e| e.to_string())?;
    ok(repo.log(a.max_count.unwrap_or(50)).map_err(|e| e.to_string())?)
}

fn git_commit_file_diff(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: CommitFileDiffArg = parse(args)?;
    let root = project_root(ctx, &a.project_id)?;
    let repo = GitRepo::open(Path::new(&root)).map_err(|e| e.to_string())?;
    ok(repo.commit_file_diff(&a.oid, &a.path).map_err(|e| e.to_string())?)
}

fn git_commit_files(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: CommitFilesArg = parse(args)?;
    let root = project_root(ctx, &a.project_id)?;
    let repo = GitRepo::open(Path::new(&root)).map_err(|e| e.to_string())?;
    ok(repo.commit_files(&a.oid).map_err(|e| e.to_string())?)
}

fn git_unpushed_commits(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: LogArg = parse(args)?;
    let root = project_root(ctx, &a.project_id)?;
    let repo = GitRepo::open(Path::new(&root)).map_err(|e| e.to_string())?;
    ok(repo
        .unpushed_commits(a.max_count.unwrap_or(100))
        .map_err(|e| e.to_string())?)
}

// ── projects dir / clone ───────────────────────────────────────────────────

/// Returns the default projects directory (`<home>/projects`), creating it if
/// needed. Desktop resolves home via `AppHandle::path().home_dir()`; the server
/// uses `ctx.home_dir()`.
fn get_default_projects_dir(ctx: &ServerContext) -> Result<Value, ApiError> {
    let projects = ctx.home_dir().join("projects");
    std::fs::create_dir_all(&projects).map_err(|e| e.to_string())?;
    ok(projects.to_string_lossy().to_string())
}

/// Validate a git clone URL. Allows only `https://` and SCP-style
/// `user@host:path`. Mirrors the desktop `validate_git_url`.
fn validate_git_url(url: &str) -> Result<(), String> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return Err("Empty git URL".to_string());
    }
    if trimmed.contains('\0') || trimmed.contains('\n') || trimmed.contains('\r') {
        return Err("Git URL contains invalid characters".to_string());
    }

    let looks_scp = !trimmed.contains("://")
        && trimmed.contains('@')
        && trimmed.find(':').map_or(false, |colon| {
            trimmed[..colon].contains('@') && !trimmed[..colon].contains('/')
        });

    if looks_scp {
        return Ok(());
    }

    if !trimmed.starts_with("https://") {
        return Err(format!(
            "Only https:// and user@host:path git URLs are allowed (got: {})",
            trimmed
        ));
    }
    Ok(())
}

/// Validate that `target` is under `home`. Mirrors desktop `validate_clone_target`.
fn validate_clone_target(target: &Path, home: &Path) -> Result<(), String> {
    let target_str = target.to_string_lossy();
    if target_str.contains("..") {
        return Err("target_dir must not contain '..'".to_string());
    }
    let canon_home = home
        .canonicalize()
        .map_err(|e| format!("Cannot resolve home dir: {}", e))?;
    let mut probe: PathBuf = target.to_path_buf();
    let canon_target = loop {
        if probe.exists() {
            break probe.canonicalize().map_err(|e| e.to_string())?;
        }
        if !probe.pop() {
            return Err("target_dir has no existing ancestor".to_string());
        }
    };
    if !canon_target.starts_with(&canon_home) {
        return Err(format!(
            "target_dir {} must be inside your home directory",
            target.display()
        ));
    }
    Ok(())
}

/// Clone a git repository into `target_dir` (defaults to `<home>/projects/<repo>`).
/// Returns the path of the cloned directory. Mirrors the desktop body, resolving
/// home via `ctx.home_dir()` and the token via the in-memory cache.
async fn git_clone(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: CloneArg = parse(args)?;
    validate_git_url(&a.url)?;

    let home = ctx.home_dir();
    let dest = if let Some(dir) = a.target_dir {
        PathBuf::from(dir)
    } else {
        home.join("projects")
    };
    validate_clone_target(&dest, &home)?;

    let repo_name = a
        .url
        .trim_end_matches('/')
        .rsplit(&['/', ':'][..])
        .next()
        .unwrap_or("repo")
        .trim_end_matches(".git")
        .to_string();

    let clone_dir = dest.join(&repo_name);

    if clone_dir.exists() {
        return Err(ApiError::bad(format!(
            "Directory already exists: {}",
            clone_dir.display()
        )));
    }

    let token = stored_token(ctx);

    let clone_dir_clone = clone_dir.clone();
    let url_clone = a.url.clone();
    tokio::task::spawn_blocking(move || {
        rustic_git::clone_repo(&url_clone, &clone_dir_clone, token.as_deref())
    })
    .await
    .map_err(|e| ApiError::bad(format!("clone task failed: {e}")))?
    .map_err(|e| e.to_string())?;

    ok(clone_dir.to_string_lossy().to_string())
}

// ── GitHub REST / OAuth device flow ─────────────────────────────────────────

/// GitHub OAuth app client id for the device flow. Mirrors the desktop const.
const GITHUB_CLIENT_ID: &str = "Ov23lijXgTEVp8hmIRf3";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GithubCreateRepoArg {
    name: String,
    private: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GithubPollTokenArg {
    device_code: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct DeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    expires_in: u64,
    interval: u64,
}

#[derive(Debug, Serialize)]
struct OAuthUserInfo {
    login: String,
    avatar_url: String,
}

/// Create a new GitHub repository for the authenticated user via the REST API.
/// Returns the HTTPS clone URL of the newly-created repository. The token is
/// read from the in-memory cache exactly like the desktop body.
async fn github_create_repo(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: GithubCreateRepoArg = parse(args)?;
    let trimmed = a.name.trim().to_string();
    if trimmed.is_empty() {
        return Err(ApiError::from("Repository name cannot be empty".to_string()));
    }

    let token = {
        let stored = ctx.state().git_token.lock_safe();
        stored
            .clone()
            .ok_or_else(|| "Not authenticated with GitHub. Sign in first.".to_string())?
    };

    let client = reqwest::Client::new();
    let resp = client
        .post("https://api.github.com/user/repos")
        .header("Authorization", format!("Bearer {}", token))
        .header("User-Agent", "Rustic-IDE")
        .header("Accept", "application/vnd.github.v3+json")
        .json(&serde_json::json!({
            "name": trimmed,
            "private": a.private,
            "auto_init": false,
        }))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !resp.status().is_success() {
        let err_body: serde_json::Value = resp.json().await.unwrap_or_default();
        let msg = err_body["message"]
            .as_str()
            .unwrap_or("Failed to create repository")
            .to_string();
        return Err(ApiError::from(msg));
    }

    let repo: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    let clone_url = repo["clone_url"]
        .as_str()
        .ok_or_else(|| "GitHub response missing clone_url".to_string())?
        .to_string();
    ok(clone_url)
}

async fn github_device_code() -> Result<Value, ApiError> {
    let client = reqwest::Client::new();
    let resp = client
        .post("https://github.com/login/device/code")
        .header("Accept", "application/json")
        .header("User-Agent", "Rustic-IDE")
        .form(&[
            ("client_id", GITHUB_CLIENT_ID),
            ("scope", "repo user:email"),
        ])
        .send()
        .await
        .map_err(|e| format!("Request failed: {}", e))?;

    let status = resp.status();
    let body_text = resp
        .text()
        .await
        .map_err(|e| format!("Failed to read response: {}", e))?;

    if !status.is_success() {
        return Err(ApiError::from(format!(
            "GitHub returned {}: {}",
            status, body_text
        )));
    }

    let parsed: serde_json::Value = serde_json::from_str(&body_text)
        .map_err(|_| format!("Invalid JSON from GitHub: {}", body_text))?;

    if let Some(err) = parsed.get("error") {
        let desc = parsed
            .get("error_description")
            .and_then(|d| d.as_str())
            .unwrap_or("Unknown error");
        return Err(ApiError::from(format!(
            "{}: {}",
            err.as_str().unwrap_or("error"),
            desc
        )));
    }

    let parsed: DeviceCodeResponse = serde_json::from_value(parsed)
        .map_err(|e| format!("Failed to parse device code response: {}", e))?;
    ok(parsed)
}

async fn github_poll_token(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: GithubPollTokenArg = parse(args)?;
    let client = reqwest::Client::new();
    let resp = client
        .post("https://github.com/login/oauth/access_token")
        .header("Accept", "application/json")
        .header("User-Agent", "Rustic-IDE")
        .form(&[
            ("client_id", GITHUB_CLIENT_ID),
            ("device_code", a.device_code.as_str()),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ])
        .send()
        .await
        .map_err(|e| format!("Request failed: {}", e))?;

    let status = resp.status();
    let body_text = resp
        .text()
        .await
        .map_err(|e| format!("Failed to read response: {}", e))?;

    // Do not log status code or body — body contains the access_token on success.

    // Try JSON first
    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&body_text) {
        if let Some(token) = parsed.get("access_token").and_then(|t| t.as_str()) {
            if !token.is_empty() {
                {
                    let mut stored = ctx.state().git_token.lock_safe();
                    *stored = Some(token.to_string());
                }
                let _ = ctx.secrets().set(GIT_TOKEN_ACCOUNT, token);
                return ok(token.to_string());
            }
        }
        if let Some(err) = parsed.get("error").and_then(|e| e.as_str()) {
            return Err(ApiError::from(err.to_string()));
        }
    }

    // Fall back to form-encoded parsing (GitHub sometimes ignores Accept header)
    let params: std::collections::HashMap<String, String> =
        form_urlencoded::parse(body_text.as_bytes())
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();

    if let Some(token) = params.get("access_token") {
        if !token.is_empty() {
            let mut stored = ctx.state().git_token.lock_safe();
            *stored = Some(token.to_string());
            return ok(token.to_string());
        }
    }
    if let Some(err) = params.get("error") {
        return Err(ApiError::from(err.to_string()));
    }

    Err(ApiError::from(format!(
        "Unexpected response ({}): {}",
        status, body_text
    )))
}

async fn github_get_user(ctx: &ServerContext) -> Result<Value, ApiError> {
    let token = {
        let stored = ctx.state().git_token.lock_safe();
        stored.clone().ok_or("Not authenticated")?
    };

    let client = reqwest::Client::new();
    let resp = client
        .get("https://api.github.com/user")
        .header("Authorization", format!("Bearer {}", token))
        .header("User-Agent", "Rustic-IDE")
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !resp.status().is_success() {
        return Err(ApiError::from(format!("GitHub API error: {}", resp.status())));
    }

    let user: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    ok(OAuthUserInfo {
        login: user["login"].as_str().unwrap_or("unknown").to_string(),
        avatar_url: user["avatar_url"].as_str().unwrap_or("").to_string(),
    })
}

// ── helpers ────────────────────────────────────────────────────────────────

fn git_diff(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: ProjectPathArg = parse(args)?;
    let root = project_root(ctx, &a.project_id)?;
    let repo = GitRepo::open(Path::new(&root)).map_err(|e| e.to_string())?;
    ok(repo.diff_file(&a.path).map_err(|e| e.to_string())?)
}

/// Run `f` against the project's opened repo, resolving root + open errors to
/// `ApiError`. For the read commands that take just `{ projectId }`.
fn with_repo<F>(ctx: &ServerContext, args: &Value, f: F) -> Result<Value, ApiError>
where
    F: FnOnce(&GitRepo) -> Result<Value, ApiError>,
{
    let a: ProjectArg = parse(args)?;
    let root = project_root(ctx, &a.project_id)?;
    let repo = GitRepo::open(Path::new(&root)).map_err(|e| e.to_string())?;
    f(&repo)
}

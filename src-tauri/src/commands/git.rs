use crate::secrets;
use crate::state::AppState;
use crate::sync_ext::MutexExt;
use rustic_git::{
    is_git_available, AheadBehind, BranchInfo, CommitFileChange, CommitInfo, ConflictFile,
    FileDiff, GitRepo, GitStatus, GIT_NOT_FOUND_MESSAGE,
};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

/// Keychain account name for the GitHub OAuth / PAT token.
pub const GIT_TOKEN_ACCOUNT: &str = "github_token";

/// Helper to get a project's root path by ID.
fn get_project_path(state: &AppState, project_id: &str) -> Result<String, String> {
    let workspace = state.workspace.lock_safe();
    let project = workspace
        .list_projects()
        .into_iter()
        .find(|p| p.id.to_string() == project_id)
        .ok_or_else(|| format!("Project not found: {}", project_id))?;
    Ok(project.root_path.to_string_lossy().to_string())
}

fn get_stored_token(state: &AppState) -> Option<String> {
    let token = state.git_token.lock_safe();
    token.clone()
}

/// Open the repo and run `f` on a blocking worker thread.
///
/// Every git operation here shells out to the git CLI and/or walks the repo —
/// blocking work that can take SECONDS-to-MINUTES on a large change set
/// (90k-file initial commits are a real use case). Tauri runs non-async
/// commands on the MAIN thread, so the old sync variants froze the entire
/// window ("not responding") for the duration. `spawn_blocking` moves the
/// work to the thread pool and keeps the UI alive.
async fn with_repo_blocking<T, F>(
    state: &State<'_, AppState>,
    project_id: &str,
    f: F,
) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(GitRepo) -> Result<T, String> + Send + 'static,
{
    let root = get_project_path(state, project_id)?;
    tokio::task::spawn_blocking(move || {
        let repo = GitRepo::open(Path::new(&root)).map_err(|e| e.to_string())?;
        f(repo)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Payload for the `git-progress` window event. The SCM panel listens and
/// renders "Staging… N files" / "Committing…" / git's own sideband line
/// ("Receiving objects: 42% (12000/90000)") during long operations.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct GitProgress {
    project_id: String,
    /// "staging" | "committing" | "pushing" | "pulling" | "fetching" |
    /// "publishing" | "done"
    phase: &'static str,
    /// Files processed so far (staging only).
    done: Option<u64>,
    /// Raw git progress line (network ops only).
    text: Option<String>,
}

fn emit_git_progress(app: &AppHandle, project_id: &str, phase: &'static str, done: Option<u64>) {
    emit_git_progress_text(app, project_id, phase, done, None);
}

fn emit_git_progress_text(
    app: &AppHandle,
    project_id: &str,
    phase: &'static str,
    done: Option<u64>,
    text: Option<String>,
) {
    let _ = app.emit(
        "git-progress",
        GitProgress {
            project_id: project_id.to_string(),
            phase,
            done,
            text,
        },
    );
}

/// Build a throttled `on_progress` callback that forwards git's sideband
/// lines as `git-progress` events (~6/sec keeps the IPC bridge calm while a
/// big transfer spams hundreds of updates per second).
fn throttled_text_progress(
    app: AppHandle,
    project_id: String,
    phase: &'static str,
) -> impl FnMut(&str) {
    let mut last_emit = std::time::Instant::now() - std::time::Duration::from_secs(1);
    move |line: &str| {
        if last_emit.elapsed() >= std::time::Duration::from_millis(150) {
            last_emit = std::time::Instant::now();
            emit_git_progress_text(&app, &project_id, phase, None, Some(line.to_string()));
        }
    }
}

/// Returned by `git_check_available`. `available = false` carries the
/// install-guidance message in `message` so the UI can render it directly.
#[derive(Debug, Clone, Serialize)]
pub struct GitAvailability {
    pub available: bool,
    pub message: Option<String>,
}

/// One-shot probe the frontend should call on app startup to detect missing
/// git. When git isn't found, returns `available: false` with the
/// install-guidance message in `message`. Sub-50 ms on every supported
/// platform; safe to call eagerly.
#[tauri::command]
pub fn git_check_available() -> GitAvailability {
    if is_git_available() {
        GitAvailability {
            available: true,
            message: None,
        }
    } else {
        GitAvailability {
            available: false,
            message: Some(GIT_NOT_FOUND_MESSAGE.to_string()),
        }
    }
}

#[tauri::command]
pub async fn git_status(
    state: State<'_, AppState>,
    project_id: String,
    limit: Option<usize>,
) -> Result<GitStatus, String> {
    with_repo_blocking(&state, &project_id, move |repo| {
        repo.status_limited(limit).map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command]
pub async fn git_stage(
    state: State<'_, AppState>,
    project_id: String,
    paths: Vec<String>,
) -> Result<Vec<String>, String> {
    with_repo_blocking(&state, &project_id, move |repo| {
        repo.stage(&paths).map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command]
pub async fn git_unstage(
    state: State<'_, AppState>,
    project_id: String,
    paths: Vec<String>,
) -> Result<(), String> {
    with_repo_blocking(&state, &project_id, move |repo| {
        repo.unstage(&paths).map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command]
pub async fn git_commit(
    app: AppHandle,
    state: State<'_, AppState>,
    project_id: String,
    message: String,
) -> Result<String, String> {
    // `git commit` over a freshly-staged 90k-file tree takes a while (tree
    // writing); tell the panel which phase it's in.
    emit_git_progress(&app, &project_id, "committing", None);
    let pid = project_id.clone();
    let result = with_repo_blocking(&state, &project_id, move |repo| {
        repo.commit(&message).map_err(|e| e.to_string())
    })
    .await;
    emit_git_progress(&app, &pid, "done", None);
    result
}

#[tauri::command]
pub async fn git_discard(
    state: State<'_, AppState>,
    project_id: String,
    paths: Vec<String>,
) -> Result<(), String> {
    with_repo_blocking(&state, &project_id, move |repo| {
        repo.discard_changes(&paths).map_err(|e| e.to_string())
    })
    .await
}

/// Stage the whole working tree (`git add -A`). Repo-wide "Stage all" — works
/// without the frontend sending every path, so it's safe on huge change lists.
/// Streams `git-progress` events (throttled) so the SCM panel can show
/// "Staging… N files" while git hashes a big tree.
#[tauri::command]
pub async fn git_stage_all(
    app: AppHandle,
    state: State<'_, AppState>,
    project_id: String,
) -> Result<(), String> {
    let pid = project_id.clone();
    let app_for_task = app.clone();
    let result = with_repo_blocking(&state, &project_id, move |repo| {
        let mut last_emit = std::time::Instant::now();
        let mut on_progress = |done: u64| {
            // ~6 events/sec keeps the UI live without flooding the IPC bridge.
            if last_emit.elapsed() >= std::time::Duration::from_millis(150) {
                last_emit = std::time::Instant::now();
                emit_git_progress(&app_for_task, &pid, "staging", Some(done));
            }
        };
        repo.stage_all_with_progress(&mut on_progress)
            .map_err(|e| e.to_string())
    })
    .await;
    emit_git_progress(&app, &project_id, "done", None);
    result
}

/// Unstage the entire index. Repo-wide "Unstage all".
#[tauri::command]
pub async fn git_unstage_all(state: State<'_, AppState>, project_id: String) -> Result<(), String> {
    with_repo_blocking(&state, &project_id, |repo| {
        repo.unstage_all().map_err(|e| e.to_string())
    })
    .await
}

/// Discard all unstaged worktree changes + delete all untracked files. Repo-wide
/// "Discard all".
#[tauri::command]
pub async fn git_discard_all(state: State<'_, AppState>, project_id: String) -> Result<(), String> {
    with_repo_blocking(&state, &project_id, |repo| {
        repo.discard_all().map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command]
pub async fn git_diff(
    state: State<'_, AppState>,
    project_id: String,
    path: String,
) -> Result<FileDiff, String> {
    with_repo_blocking(&state, &project_id, move |repo| {
        repo.diff_file(&path).map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command]
pub async fn git_diff_staged(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<Vec<FileDiff>, String> {
    with_repo_blocking(&state, &project_id, |repo| {
        repo.diff_staged().map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command]
pub async fn git_branches(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<Vec<BranchInfo>, String> {
    with_repo_blocking(&state, &project_id, |repo| {
        repo.branches().map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command]
pub async fn git_init(state: State<'_, AppState>, project_id: String) -> Result<(), String> {
    let root = get_project_path(&state, &project_id)?;
    tokio::task::spawn_blocking(move || {
        GitRepo::init(Path::new(&root)).map_err(|e| e.to_string())?;
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn git_push(
    app: AppHandle,
    state: State<'_, AppState>,
    project_id: String,
) -> Result<(), String> {
    let token = get_stored_token(&state);
    let mut on_progress = throttled_text_progress(app.clone(), project_id.clone(), "pushing");
    let result = with_repo_blocking(&state, &project_id, move |repo| {
        repo.push_with_progress(token.as_deref(), &mut on_progress)
            .map_err(|e| e.to_string())
    })
    .await;
    emit_git_progress(&app, &project_id, "done", None);
    result
}

#[tauri::command]
pub async fn git_publish_branch(
    app: AppHandle,
    state: State<'_, AppState>,
    project_id: String,
) -> Result<(), String> {
    let token = get_stored_token(&state);
    let mut on_progress = throttled_text_progress(app.clone(), project_id.clone(), "publishing");
    let result = with_repo_blocking(&state, &project_id, move |repo| {
        repo.publish_branch_with_progress(token.as_deref(), &mut on_progress)
            .map_err(|e| e.to_string())
    })
    .await;
    emit_git_progress(&app, &project_id, "done", None);
    result
}

#[tauri::command]
pub async fn git_pull(
    app: AppHandle,
    state: State<'_, AppState>,
    project_id: String,
) -> Result<(), String> {
    let token = get_stored_token(&state);
    let mut on_progress = throttled_text_progress(app.clone(), project_id.clone(), "pulling");
    let result = with_repo_blocking(&state, &project_id, move |repo| {
        repo.pull_with_progress(token.as_deref(), &mut on_progress)
            .map_err(|e| e.to_string())
    })
    .await;
    emit_git_progress(&app, &project_id, "done", None);
    result
}

#[tauri::command]
pub async fn git_fetch(
    app: AppHandle,
    state: State<'_, AppState>,
    project_id: String,
) -> Result<(), String> {
    let token = get_stored_token(&state);
    let mut on_progress = throttled_text_progress(app.clone(), project_id.clone(), "fetching");
    let result = with_repo_blocking(&state, &project_id, move |repo| {
        repo.fetch_with_progress(token.as_deref(), &mut on_progress)
            .map_err(|e| e.to_string())
    })
    .await;
    emit_git_progress(&app, &project_id, "done", None);
    result
}

#[tauri::command]
pub async fn git_ahead_behind(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<AheadBehind, String> {
    with_repo_blocking(&state, &project_id, |repo| {
        repo.ahead_behind().map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command]
pub async fn git_checkout_branch(
    state: State<'_, AppState>,
    project_id: String,
    branch: String,
) -> Result<(), String> {
    with_repo_blocking(&state, &project_id, move |repo| {
        repo.checkout_branch(&branch).map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command]
pub async fn git_create_branch(
    state: State<'_, AppState>,
    project_id: String,
    branch: String,
    checkout: bool,
) -> Result<(), String> {
    with_repo_blocking(&state, &project_id, move |repo| {
        repo.create_branch(&branch, checkout)
            .map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command]
pub async fn git_rebase(
    state: State<'_, AppState>,
    project_id: String,
    onto_branch: String,
) -> Result<(), String> {
    with_repo_blocking(&state, &project_id, move |repo| {
        repo.rebase(&onto_branch).map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command]
pub async fn git_rebase_continue(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<(), String> {
    with_repo_blocking(&state, &project_id, |repo| {
        repo.rebase_continue().map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command]
pub async fn git_rebase_abort(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<(), String> {
    with_repo_blocking(&state, &project_id, |repo| {
        repo.rebase_abort().map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command]
pub async fn git_get_conflicts(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<Vec<ConflictFile>, String> {
    with_repo_blocking(&state, &project_id, |repo| {
        repo.get_conflicts().map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command]
pub async fn git_resolve_conflict(
    state: State<'_, AppState>,
    project_id: String,
    path: String,
    side: String,
) -> Result<(), String> {
    with_repo_blocking(&state, &project_id, move |repo| {
        repo.resolve_conflict_side(&path, &side)
            .map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command]
pub async fn git_merge_commit(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<String, String> {
    with_repo_blocking(&state, &project_id, |repo| {
        repo.merge_commit().map_err(|e| e.to_string())
    })
    .await
}

/// Store a GitHub token. Clearing (empty string) is allowed without prompting;
/// setting a new token requires the user to confirm via a native dialog so an
/// XSS / malicious-renderer cannot silently swap the token for one belonging
/// to an attacker (which would let `git_push` exfiltrate commits to a remote
/// the attacker controls).
#[tauri::command]
pub async fn git_set_token(
    app: AppHandle,
    state: State<'_, AppState>,
    token: String,
) -> Result<(), String> {
    if token.is_empty() {
        // Clearing the token doesn't need a prompt — and we want the
        // account-panel "Sign out" flow to remain a single click.
        let mut stored = state.git_token.lock_safe();
        *stored = None;
        let _ = secrets::delete(GIT_TOKEN_ACCOUNT);
        return Ok(());
    }

    // Confirm with the user via a native modal. blocking_show() is a sync
    // call; offload to a blocking thread so we don't stall the async runtime
    // (and so a misbehaving webview cannot synthesize an instant "OK").
    let app_clone = app.clone();
    let confirmed = tokio::task::spawn_blocking(move || {
        app_clone
            .dialog()
            .message(
                "Rustic is about to save a GitHub access token. Confirm only if you initiated this action — accepting will allow Rustic to push commits using this token.",
            )
            .title("Save GitHub token?")
            .kind(MessageDialogKind::Warning)
            .buttons(MessageDialogButtons::OkCancelCustom(
                "Save token".into(),
                "Cancel".into(),
            ))
            .blocking_show()
    })
    .await
    .map_err(|e| e.to_string())?;

    if !confirmed {
        return Err("User cancelled token save".to_string());
    }

    {
        let mut stored = state.git_token.lock_safe();
        *stored = Some(token.clone());
    }
    if let Err(e) = secrets::set(GIT_TOKEN_ACCOUNT, &token) {
        // Keychain failure is non-fatal — the in-memory token still works for
        // this session. Surface as a warning rather than swallowing silently.
        tracing::warn!(error = %e, "git_set_token: keychain set failed; token kept in memory only");
    }
    Ok(())
}

#[tauri::command]
pub fn git_get_token(state: State<'_, AppState>) -> Result<bool, String> {
    let stored = state.git_token.lock_safe();
    Ok(stored.is_some())
}

/// Add a git remote. Confirms via a native dialog before changing an existing
/// remote URL — XSS could otherwise repoint `origin` to an attacker server and
/// the next user-initiated push would exfiltrate the working tree.
#[tauri::command]
pub async fn git_add_remote(
    app: AppHandle,
    state: State<'_, AppState>,
    project_id: String,
    name: String,
    url: String,
) -> Result<(), String> {
    let root = get_project_path(&state, &project_id)?;
    // If a remote with the same name already exists with a different URL,
    // prompt before overwriting.
    let existing = GitRepo::open(Path::new(&root))
        .ok()
        .and_then(|r| r.get_remote_url().ok())
        .flatten();
    if let Some(existing_url) = existing.clone() {
        if existing_url != url {
            let url_for_msg = url.clone();
            let existing_for_msg = existing_url.clone();
            let app_clone = app.clone();
            let confirmed = tokio::task::spawn_blocking(move || {
                app_clone
                    .dialog()
                    .message(format!(
                        "Change git remote '{}' from\n\n{}\n\nto\n\n{}?",
                        "origin", existing_for_msg, url_for_msg
                    ))
                    .title("Change git remote?")
                    .kind(MessageDialogKind::Warning)
                    .buttons(MessageDialogButtons::OkCancelCustom(
                        "Change remote".into(),
                        "Cancel".into(),
                    ))
                    .blocking_show()
            })
            .await
            .map_err(|e| e.to_string())?;
            if !confirmed {
                return Err("User cancelled remote change".to_string());
            }
        }
    }

    let repo = GitRepo::open(Path::new(&root)).map_err(|e| e.to_string())?;
    repo.add_remote(&name, &url).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn git_get_remote_url(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<Option<String>, String> {
    with_repo_blocking(&state, &project_id, |repo| {
        repo.get_remote_url().map_err(|e| e.to_string())
    })
    .await
}

// ── Gitignore ────────────────────────────────────────────────────────

#[tauri::command]
pub fn git_add_to_gitignore(
    state: State<'_, AppState>,
    project_id: String,
    pattern: String,
) -> Result<(), String> {
    let root = get_project_path(&state, &project_id)?;
    let gitignore_path = Path::new(&root).join(".gitignore");

    use std::io::Write;

    // Read existing content
    let existing = std::fs::read_to_string(&gitignore_path).unwrap_or_default();

    // Check if pattern already exists
    if existing.lines().any(|line| line.trim() == pattern.trim()) {
        return Ok(());
    }

    // Append pattern
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&gitignore_path)
        .map_err(|e| e.to_string())?;

    // Add newline before pattern if file doesn't end with one
    if !existing.is_empty() && !existing.ends_with('\n') {
        writeln!(file).map_err(|e| e.to_string())?;
    }
    writeln!(file, "{}", pattern).map_err(|e| e.to_string())?;

    Ok(())
}

// ── Git Log / History ────────────────────────────────────────────────

#[tauri::command]
pub async fn git_log(
    state: State<'_, AppState>,
    project_id: String,
    max_count: Option<usize>,
) -> Result<Vec<CommitInfo>, String> {
    with_repo_blocking(&state, &project_id, move |repo| {
        repo.log(max_count.unwrap_or(50)).map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command]
pub async fn git_commit_file_diff(
    state: State<'_, AppState>,
    project_id: String,
    oid: String,
    path: String,
) -> Result<FileDiff, String> {
    with_repo_blocking(&state, &project_id, move |repo| {
        repo.commit_file_diff(&oid, &path)
            .map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command]
pub async fn git_commit_files(
    state: State<'_, AppState>,
    project_id: String,
    oid: String,
) -> Result<Vec<CommitFileChange>, String> {
    with_repo_blocking(&state, &project_id, move |repo| {
        repo.commit_files(&oid).map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command]
pub async fn git_unpushed_commits(
    state: State<'_, AppState>,
    project_id: String,
    max_count: Option<usize>,
) -> Result<Vec<CommitInfo>, String> {
    with_repo_blocking(&state, &project_id, move |repo| {
        repo.unpushed_commits(max_count.unwrap_or(100))
            .map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command]
pub async fn git_undo_last_commit(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<(), String> {
    with_repo_blocking(&state, &project_id, |repo| {
        repo.undo_last_commit().map_err(|e| e.to_string())
    })
    .await
}

/// Check whether the project directory is inside a git repository.
/// Uses `GitRepo::open` (which calls `Repository::discover`) so nested
/// projects that live inside a parent repo are still detected correctly.
#[tauri::command]
pub async fn git_is_repo(state: State<'_, AppState>, project_id: String) -> Result<bool, String> {
    let root = get_project_path(&state, &project_id)?;
    tokio::task::spawn_blocking(move || Ok(GitRepo::open(Path::new(&root)).is_ok()))
        .await
        .map_err(|e| e.to_string())?
}

/// Create a new GitHub repository for the authenticated user via the REST API.
/// Returns the HTTPS clone URL of the newly-created repository.
#[tauri::command]
pub async fn github_create_repo(
    state: State<'_, AppState>,
    name: String,
    private: bool,
) -> Result<String, String> {
    let trimmed = name.trim().to_string();
    if trimmed.is_empty() {
        return Err("Repository name cannot be empty".to_string());
    }

    let token = {
        let stored = state.git_token.lock_safe();
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
            "private": private,
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
        let name_exists = err_body["errors"]
            .as_array()
            .map(|errs| {
                errs.iter().any(|e| {
                    e["message"]
                        .as_str()
                        .is_some_and(|m| m.contains("already exists"))
                })
            })
            .unwrap_or(false)
            || msg.contains("already exists");
        // Recovery path for a previously-failed publish: the repo was created
        // but the push never landed. Reuse the existing repo instead of
        // failing the whole publish on the retry.
        if name_exists {
            if let Some(url) = existing_repo_clone_url(&client, &token, &trimmed).await {
                return Ok(url);
            }
        }
        return Err(msg);
    }

    let repo: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    let clone_url = repo["clone_url"]
        .as_str()
        .ok_or_else(|| "GitHub response missing clone_url".to_string())?
        .to_string();
    Ok(clone_url)
}

/// Look up the authenticated user's existing repo `name` and return its
/// clone URL, or None when it can't be resolved.
async fn existing_repo_clone_url(
    client: &reqwest::Client,
    token: &str,
    name: &str,
) -> Option<String> {
    let user: serde_json::Value = client
        .get("https://api.github.com/user")
        .header("Authorization", format!("Bearer {}", token))
        .header("User-Agent", "Rustic-IDE")
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;
    let login = user["login"].as_str()?;
    let repo: serde_json::Value = client
        .get(format!("https://api.github.com/repos/{}/{}", login, name))
        .header("Authorization", format!("Bearer {}", token))
        .header("User-Agent", "Rustic-IDE")
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;
    repo["clone_url"].as_str().map(|s| s.to_string())
}

// ── GitHub OAuth Device Flow ─────────────────────────────────────────

const GITHUB_CLIENT_ID: &str = "Ov23lijXgTEVp8hmIRf3";

#[derive(Debug, Serialize, Deserialize)]
pub struct DeviceCodeResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
}

#[derive(Debug, Serialize)]
pub struct OAuthUserInfo {
    pub login: String,
    pub avatar_url: String,
}

#[tauri::command]
pub async fn github_device_code() -> Result<DeviceCodeResponse, String> {
    let client = reqwest::Client::new();
    let resp = client
        .post("https://github.com/login/device/code")
        .header("Accept", "application/json")
        .header("User-Agent", "Rustic-IDE")
        .form(&[
            ("client_id", GITHUB_CLIENT_ID),
            ("scope", "repo workflow user:email"),
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
        return Err(format!("GitHub returned {}: {}", status, body_text));
    }

    // Try to parse as our expected response
    let parsed: serde_json::Value = serde_json::from_str(&body_text)
        .map_err(|_| format!("Invalid JSON from GitHub: {}", body_text))?;

    // Check for error in response
    if let Some(err) = parsed.get("error") {
        let desc = parsed
            .get("error_description")
            .and_then(|d| d.as_str())
            .unwrap_or("Unknown error");
        return Err(format!("{}: {}", err.as_str().unwrap_or("error"), desc));
    }

    serde_json::from_value(parsed)
        .map_err(|e| format!("Failed to parse device code response: {}", e))
}

#[tauri::command]
pub async fn github_poll_token(
    state: State<'_, AppState>,
    device_code: String,
) -> Result<String, String> {
    let client = reqwest::Client::new();
    let resp = client
        .post("https://github.com/login/oauth/access_token")
        .header("Accept", "application/json")
        .header("User-Agent", "Rustic-IDE")
        .form(&[
            ("client_id", GITHUB_CLIENT_ID),
            ("device_code", device_code.as_str()),
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
                    let mut stored = state.git_token.lock_safe();
                    *stored = Some(token.to_string());
                }
                let _ = secrets::set(GIT_TOKEN_ACCOUNT, token);
                return Ok(token.to_string());
            }
        }
        if let Some(err) = parsed.get("error").and_then(|e| e.as_str()) {
            return Err(err.to_string());
        }
    }

    // Fall back to form-encoded parsing (GitHub sometimes ignores Accept header)
    let params: std::collections::HashMap<String, String> =
        form_urlencoded::parse(body_text.as_bytes())
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();

    if let Some(token) = params.get("access_token") {
        if !token.is_empty() {
            let mut stored = state.git_token.lock_safe();
            *stored = Some(token.to_string());
            return Ok(token.to_string());
        }
    }
    if let Some(err) = params.get("error") {
        return Err(err.to_string());
    }

    Err(format!("Unexpected response ({}): {}", status, body_text))
}

#[tauri::command]
pub async fn github_get_user(state: State<'_, AppState>) -> Result<OAuthUserInfo, String> {
    let token = {
        let stored = state.git_token.lock_safe();
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
        return Err(format!("GitHub API error: {}", resp.status()));
    }

    let user: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    Ok(OAuthUserInfo {
        login: user["login"].as_str().unwrap_or("unknown").to_string(),
        avatar_url: user["avatar_url"].as_str().unwrap_or("").to_string(),
    })
}

// ── Clone ─────────────────────────────────────────────────────────────

/// Returns the default projects directory (`~/projects`), creating it if needed.
#[tauri::command]
pub fn get_default_projects_dir(app: AppHandle) -> Result<String, String> {
    let home = app.path().home_dir().map_err(|e| e.to_string())?;
    let projects = home.join("projects");
    std::fs::create_dir_all(&projects).map_err(|e| e.to_string())?;
    Ok(projects.to_string_lossy().to_string())
}

/// Validate a git clone URL. Allows only `https://` and SCP-style `user@host:path`.
/// Rejects `file://`, `ext::`, `git://`, raw paths, and anything containing
/// shell metacharacters that could be coerced through libgit2 transports.
fn validate_git_url(url: &str) -> Result<(), String> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return Err("Empty git URL".to_string());
    }
    if trimmed.contains('\0') || trimmed.contains('\n') || trimmed.contains('\r') {
        return Err("Git URL contains invalid characters".to_string());
    }

    // SCP-style: user@host:path  (no scheme, must contain ':' before any '/')
    let looks_scp = !trimmed.contains("://")
        && trimmed.contains('@')
        && trimmed.find(':').map_or(false, |colon| {
            trimmed[..colon].contains('@') && !trimmed[..colon].contains('/')
        });

    if looks_scp {
        return Ok(());
    }

    // Otherwise must be https://
    if !trimmed.starts_with("https://") {
        return Err(format!(
            "Only https:// and user@host:path git URLs are allowed (got: {})",
            trimmed
        ));
    }
    Ok(())
}

/// Validate a user-picked clone destination: it must be an EXISTING directory
/// (the folder picker only returns existing dirs) with no traversal patterns.
/// The old home-directory confinement is gone — projects legitimately live on
/// other drives (e.g. `D:\Programming`), and the destination now always comes
/// from an explicit native folder pick rather than free webview text.
fn validate_clone_target(target: &std::path::Path) -> Result<(), String> {
    if target.to_string_lossy().contains("..") {
        return Err("target_dir must not contain '..'".to_string());
    }
    if !target.is_dir() {
        return Err(format!(
            "Destination is not an existing folder: {}",
            target.display()
        ));
    }
    Ok(())
}

/// Clone a git repository into `target_dir` (defaults to `~/projects/<repo-name>`).
/// Returns the path of the cloned directory. Streams `git-progress` events
/// under the synthetic project id `__clone__` ("Receiving objects: …",
/// "Updating files: …") for the clone dialog to render.
#[tauri::command]
pub async fn git_clone(
    app: AppHandle,
    state: State<'_, AppState>,
    url: String,
    target_dir: Option<String>,
) -> Result<String, String> {
    validate_git_url(&url)?;

    let dest = if let Some(dir) = target_dir {
        std::path::PathBuf::from(dir)
    } else {
        let home = app.path().home_dir().map_err(|e| e.to_string())?;
        let projects = home.join("projects");
        std::fs::create_dir_all(&projects).map_err(|e| e.to_string())?;
        projects
    };
    validate_clone_target(&dest)?;

    // Derive repo name from URL (strip trailing slash + .git suffix)
    let repo_name = url
        .trim_end_matches('/')
        .rsplit(&['/', ':'][..])
        .next()
        .unwrap_or("repo")
        .trim_end_matches(".git")
        .to_string();

    let clone_dir = dest.join(&repo_name);

    if clone_dir.exists() {
        return Err(format!("Directory already exists: {}", clone_dir.display()));
    }

    let token = get_stored_token(&state);

    // Clone is blocking I/O — run it on the thread pool so we don't stall the async runtime.
    let clone_dir_clone = clone_dir.clone();
    let url_clone = url.clone();
    let mut on_progress = throttled_text_progress(app.clone(), "__clone__".to_string(), "cloning");
    let result = tokio::task::spawn_blocking(move || {
        rustic_git::clone_repo_with_progress(
            &url_clone,
            &clone_dir_clone,
            token.as_deref(),
            &mut on_progress,
        )
    })
    .await
    .map_err(|e| e.to_string())?;
    emit_git_progress(&app, "__clone__", "done", None);
    result.map_err(|e| e.to_string())?;

    Ok(clone_dir.to_string_lossy().to_string())
}

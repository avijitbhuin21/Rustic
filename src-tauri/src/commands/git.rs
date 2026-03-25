use crate::state::AppState;
use rustic_git::{AheadBehind, BranchInfo, ConflictFile, FileDiff, GitRepo, GitStatus};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tauri::State;

/// Helper to get a project's root path by ID.
fn get_project_path(state: &AppState, project_id: &str) -> Result<String, String> {
    let workspace = state.workspace.lock().unwrap();
    let project = workspace
        .list_projects()
        .into_iter()
        .find(|p| p.id.to_string() == project_id)
        .ok_or_else(|| format!("Project not found: {}", project_id))?;
    Ok(project.root_path.to_string_lossy().to_string())
}

fn get_stored_token(state: &AppState) -> Option<String> {
    let token = state.git_token.lock().unwrap();
    token.clone()
}

#[tauri::command]
pub fn git_status(state: State<'_, AppState>, project_id: String) -> Result<GitStatus, String> {
    let root = get_project_path(&state, &project_id)?;
    let repo = GitRepo::open(Path::new(&root)).map_err(|e| e.to_string())?;
    repo.status().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn git_stage(
    state: State<'_, AppState>,
    project_id: String,
    paths: Vec<String>,
) -> Result<(), String> {
    let root = get_project_path(&state, &project_id)?;
    let repo = GitRepo::open(Path::new(&root)).map_err(|e| e.to_string())?;
    repo.stage(&paths).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn git_unstage(
    state: State<'_, AppState>,
    project_id: String,
    paths: Vec<String>,
) -> Result<(), String> {
    let root = get_project_path(&state, &project_id)?;
    let repo = GitRepo::open(Path::new(&root)).map_err(|e| e.to_string())?;
    repo.unstage(&paths).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn git_commit(
    state: State<'_, AppState>,
    project_id: String,
    message: String,
) -> Result<String, String> {
    let root = get_project_path(&state, &project_id)?;
    let repo = GitRepo::open(Path::new(&root)).map_err(|e| e.to_string())?;
    repo.commit(&message).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn git_discard(
    state: State<'_, AppState>,
    project_id: String,
    paths: Vec<String>,
) -> Result<(), String> {
    let root = get_project_path(&state, &project_id)?;
    let repo = GitRepo::open(Path::new(&root)).map_err(|e| e.to_string())?;
    repo.discard_changes(&paths).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn git_diff(
    state: State<'_, AppState>,
    project_id: String,
    path: String,
) -> Result<FileDiff, String> {
    let root = get_project_path(&state, &project_id)?;
    let repo = GitRepo::open(Path::new(&root)).map_err(|e| e.to_string())?;
    repo.diff_file(&path).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn git_diff_staged(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<Vec<FileDiff>, String> {
    let root = get_project_path(&state, &project_id)?;
    let repo = GitRepo::open(Path::new(&root)).map_err(|e| e.to_string())?;
    repo.diff_staged().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn git_branches(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<Vec<BranchInfo>, String> {
    let root = get_project_path(&state, &project_id)?;
    let repo = GitRepo::open(Path::new(&root)).map_err(|e| e.to_string())?;
    repo.branches().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn git_init(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<(), String> {
    let root = get_project_path(&state, &project_id)?;
    GitRepo::init(Path::new(&root)).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn git_push(state: State<'_, AppState>, project_id: String) -> Result<(), String> {
    let root = get_project_path(&state, &project_id)?;
    let repo = GitRepo::open(Path::new(&root)).map_err(|e| e.to_string())?;
    let token = get_stored_token(&state);
    repo.push(token.as_deref()).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn git_pull(state: State<'_, AppState>, project_id: String) -> Result<(), String> {
    let root = get_project_path(&state, &project_id)?;
    let repo = GitRepo::open(Path::new(&root)).map_err(|e| e.to_string())?;
    let token = get_stored_token(&state);
    repo.pull(token.as_deref()).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn git_fetch(state: State<'_, AppState>, project_id: String) -> Result<(), String> {
    let root = get_project_path(&state, &project_id)?;
    let repo = GitRepo::open(Path::new(&root)).map_err(|e| e.to_string())?;
    let token = get_stored_token(&state);
    repo.fetch(token.as_deref()).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn git_ahead_behind(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<AheadBehind, String> {
    let root = get_project_path(&state, &project_id)?;
    let repo = GitRepo::open(Path::new(&root)).map_err(|e| e.to_string())?;
    repo.ahead_behind().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn git_checkout_branch(
    state: State<'_, AppState>,
    project_id: String,
    branch: String,
) -> Result<(), String> {
    let root = get_project_path(&state, &project_id)?;
    let repo = GitRepo::open(Path::new(&root)).map_err(|e| e.to_string())?;
    repo.checkout_branch(&branch).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn git_create_branch(
    state: State<'_, AppState>,
    project_id: String,
    branch: String,
    checkout: bool,
) -> Result<(), String> {
    let root = get_project_path(&state, &project_id)?;
    let repo = GitRepo::open(Path::new(&root)).map_err(|e| e.to_string())?;
    repo.create_branch(&branch, checkout).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn git_rebase(
    state: State<'_, AppState>,
    project_id: String,
    onto_branch: String,
) -> Result<(), String> {
    let root = get_project_path(&state, &project_id)?;
    let repo = GitRepo::open(Path::new(&root)).map_err(|e| e.to_string())?;
    repo.rebase(&onto_branch).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn git_rebase_continue(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<(), String> {
    let root = get_project_path(&state, &project_id)?;
    let repo = GitRepo::open(Path::new(&root)).map_err(|e| e.to_string())?;
    repo.rebase_continue().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn git_rebase_abort(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<(), String> {
    let root = get_project_path(&state, &project_id)?;
    let repo = GitRepo::open(Path::new(&root)).map_err(|e| e.to_string())?;
    repo.rebase_abort().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn git_get_conflicts(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<Vec<ConflictFile>, String> {
    let root = get_project_path(&state, &project_id)?;
    let repo = GitRepo::open(Path::new(&root)).map_err(|e| e.to_string())?;
    repo.get_conflicts().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn git_resolve_conflict(
    state: State<'_, AppState>,
    project_id: String,
    path: String,
    side: String,
) -> Result<(), String> {
    let root = get_project_path(&state, &project_id)?;
    let repo = GitRepo::open(Path::new(&root)).map_err(|e| e.to_string())?;
    repo.resolve_conflict_side(&path, &side).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn git_merge_commit(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<String, String> {
    let root = get_project_path(&state, &project_id)?;
    let repo = GitRepo::open(Path::new(&root)).map_err(|e| e.to_string())?;
    repo.merge_commit().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn git_set_token(
    state: State<'_, AppState>,
    token: String,
) -> Result<(), String> {
    let mut stored = state.git_token.lock().unwrap();
    *stored = if token.is_empty() { None } else { Some(token) };
    Ok(())
}

#[tauri::command]
pub fn git_get_token(state: State<'_, AppState>) -> Result<bool, String> {
    let stored = state.git_token.lock().unwrap();
    Ok(stored.is_some())
}

#[tauri::command]
pub fn git_add_remote(
    state: State<'_, AppState>,
    project_id: String,
    name: String,
    url: String,
) -> Result<(), String> {
    let root = get_project_path(&state, &project_id)?;
    let repo = GitRepo::open(Path::new(&root)).map_err(|e| e.to_string())?;
    repo.add_remote(&name, &url).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn git_get_remote_url(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<Option<String>, String> {
    let root = get_project_path(&state, &project_id)?;
    let repo = GitRepo::open(Path::new(&root)).map_err(|e| e.to_string())?;
    repo.get_remote_url().map_err(|e| e.to_string())
}

// ── GitHub OAuth Device Flow ─────────────────────────────────────────

const GITHUB_CLIENT_ID: &str = "Ov23liYrt8i3vY4NIJtG";

#[derive(Debug, Serialize, Deserialize)]
pub struct DeviceCodeResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
}

#[derive(Debug, Deserialize)]
struct OAuthTokenResponse {
    access_token: Option<String>,
    error: Option<String>,
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
        .form(&[
            ("client_id", GITHUB_CLIENT_ID),
            ("scope", "repo user:email"),
        ])
        .send()
        .await
        .map_err(|e| e.to_string())?;

    resp.json::<DeviceCodeResponse>()
        .await
        .map_err(|e| e.to_string())
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
        .form(&[
            ("client_id", GITHUB_CLIENT_ID),
            ("device_code", device_code.as_str()),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ])
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let body: OAuthTokenResponse = resp.json().await.map_err(|e| e.to_string())?;

    if let Some(token) = body.access_token {
        // Store the token
        let mut stored = state.git_token.lock().unwrap();
        *stored = Some(token.clone());
        Ok(token)
    } else if let Some(err) = body.error {
        Err(err)
    } else {
        Err("Unknown error".to_string())
    }
}

#[tauri::command]
pub async fn github_get_user(state: State<'_, AppState>) -> Result<OAuthUserInfo, String> {
    let token = {
        let stored = state.git_token.lock().unwrap();
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

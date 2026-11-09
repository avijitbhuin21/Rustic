//! Global skills — desktop adapter over `rustic_app::host_ops::skills`.

use rustic_app::host_ops::skills as ops;

pub use ops::{RepoSkillInfo, SkillInfo};

#[tauri::command(async)]
pub fn list_skills() -> Result<Vec<SkillInfo>, String> {
    Ok(ops::list_skills())
}

#[tauri::command(async)]
pub fn get_skill_body(name: String) -> Result<String, String> {
    ops::get_skill_body(&name)
}

#[tauri::command(async)]
pub fn create_skill(name: String, body: String) -> Result<SkillInfo, String> {
    ops::create_skill(&name, &body)
}

#[tauri::command(async)]
pub fn update_skill(
    original_name: String,
    name: String,
    body: String,
) -> Result<SkillInfo, String> {
    ops::update_skill(&original_name, &name, &body)
}

#[tauri::command(async)]
pub fn delete_skill(name: String) -> Result<(), String> {
    ops::delete_skill(&name)
}

#[tauri::command]
pub async fn list_repo_skills(source: String) -> Result<Vec<RepoSkillInfo>, String> {
    ops::list_repo_skills(&source).await
}

#[tauri::command]
pub async fn preview_repo_skill(source: String, path: String) -> Result<String, String> {
    ops::preview_repo_skill(&source, &path).await
}

#[tauri::command]
pub async fn install_repo_skills(
    source: String,
    paths: Vec<String>,
    names: Option<Vec<String>>,
) -> Result<Vec<SkillInfo>, String> {
    ops::install_repo_skills(&source, &paths, names.as_deref()).await
}

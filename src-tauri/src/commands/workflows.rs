//! Global workflows — desktop adapter over `rustic_app::host_ops::workflows`.

use rustic_app::host_ops::workflows as ops;

pub use ops::{RepoWorkflowInfo, WorkflowInfo};

#[tauri::command(async)]
pub fn list_workflows() -> Result<Vec<WorkflowInfo>, String> {
    Ok(ops::list_workflows())
}

#[tauri::command(async)]
pub fn get_workflow_body(name: String) -> Result<String, String> {
    ops::get_workflow_body(&name)
}

#[tauri::command(async)]
pub fn create_workflow(name: String, body: String) -> Result<WorkflowInfo, String> {
    ops::create_workflow(&name, &body)
}

#[tauri::command(async)]
pub fn update_workflow(original_name: String, name: String, body: String) -> Result<WorkflowInfo, String> {
    ops::update_workflow(&original_name, &name, &body)
}

#[tauri::command(async)]
pub fn delete_workflow(name: String) -> Result<(), String> {
    ops::delete_workflow(&name)
}

#[tauri::command]
pub async fn list_repo_workflows(source: String) -> Result<Vec<RepoWorkflowInfo>, String> {
    ops::list_repo_workflows(&source).await
}

#[tauri::command]
pub async fn preview_repo_workflow(source: String, path: String) -> Result<String, String> {
    ops::preview_repo_workflow(&source, &path).await
}

#[tauri::command]
pub async fn install_repo_workflows(
    source: String,
    paths: Vec<String>,
    names: Option<Vec<String>>,
) -> Result<Vec<WorkflowInfo>, String> {
    ops::install_repo_workflows(&source, &paths, names.as_deref()).await
}

use crate::state::AppState;
use rustic_core::workspace::project::Project;
use std::path::PathBuf;
use tauri::State;

#[tauri::command]
pub async fn add_project(
    state: State<'_, AppState>,
    path: String,
) -> Result<Project, String> {
    let path = PathBuf::from(&path);
    if !path.exists() || !path.is_dir() {
        return Err(format!("Directory does not exist: {}", path.display()));
    }

    let mut workspace = state.workspace.lock().map_err(|e| e.to_string())?;
    let project = workspace.add_project(path);
    Ok(project)
}

#[tauri::command]
pub async fn remove_project(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<(), String> {
    let mut workspace = state.workspace.lock().map_err(|e| e.to_string())?;
    workspace.remove_project(&project_id);
    Ok(())
}

#[tauri::command]
pub async fn list_projects(
    state: State<'_, AppState>,
) -> Result<Vec<Project>, String> {
    let workspace = state.workspace.lock().map_err(|e| e.to_string())?;
    Ok(workspace.list_projects())
}

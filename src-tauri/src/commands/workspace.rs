use crate::state::AppState;
use rustic_core::workspace::project::Project;
use rustic_db::models::ProjectRow;
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

    // Return early if already in workspace memory
    {
        let workspace = state.workspace.lock().map_err(|e| e.to_string())?;
        if let Some(existing) = workspace.projects.iter().find(|p| p.root_path == path) {
            return Ok(existing.clone());
        }
    }

    // Reuse the stable project ID from DB (keyed by root_path) so the FK
    // constraint never breaks after an app restart that re-generates UUIDs.
    let existing_id = {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        db.get_project_by_path(&path.to_string_lossy())
            .ok()
            .flatten()
            .map(|p| p.id)
    };

    let mut project = Project::new(path);
    if let Some(id) = existing_id {
        project.id = id;
    }

    {
        let mut workspace = state.workspace.lock().map_err(|e| e.to_string())?;
        // Double-check in case another thread added it concurrently
        if let Some(existing) = workspace.projects.iter().find(|p| p.root_path == project.root_path) {
            return Ok(existing.clone());
        }
        workspace.projects.push(project.clone());
    }

    // Persist to DB so tasks can reference project_id via foreign key
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let _ = db.insert_project(&ProjectRow {
        id: project.id.clone(),
        name: project.name.clone(),
        root_path: project.root_path.to_string_lossy().to_string(),
        created_at: now,
        settings_json: None,
    });

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

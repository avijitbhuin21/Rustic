use crate::state::AppState;
use rustic_agent::{WorkflowDef, discover_workflows, workflow_body};
use serde::Serialize;
use std::path::PathBuf;
use tauri::State;

/// Serializable workflow info returned to the frontend.
#[derive(Clone, Serialize)]
pub struct WorkflowInfo {
    pub name: String,
    pub description: String,
}

fn to_workflow_info(w: &WorkflowDef) -> WorkflowInfo {
    WorkflowInfo {
        name: w.name.clone(),
        description: w.description.clone(),
    }
}

fn project_root(state: &AppState, project_id: &str) -> Result<PathBuf, String> {
    let workspace = state.workspace.lock().unwrap();
    workspace
        .list_projects()
        .into_iter()
        .find(|p| p.id.to_string() == project_id)
        .map(|p| p.root_path.clone())
        .ok_or_else(|| "Project not found".to_string())
}

#[tauri::command]
pub fn list_workflows(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<Vec<WorkflowInfo>, String> {
    let root = project_root(&state, &project_id)?;
    let workflows = discover_workflows(&root);
    Ok(workflows.iter().map(to_workflow_info).collect())
}

#[tauri::command]
pub fn get_workflow_body(
    state: State<'_, AppState>,
    project_id: String,
    name: String,
) -> Result<String, String> {
    let root = project_root(&state, &project_id)?;
    let workflows = discover_workflows(&root);
    let workflow = workflows
        .iter()
        .find(|w| w.name == name)
        .ok_or_else(|| format!("Workflow not found: {}", name))?;
    let content = std::fs::read_to_string(&workflow.path).map_err(|e| e.to_string())?;
    Ok(workflow_body(&content).to_string())
}

#[tauri::command]
pub fn create_workflow(
    state: State<'_, AppState>,
    project_id: String,
    name: String,
    description: String,
    body: String,
) -> Result<WorkflowInfo, String> {
    let root = project_root(&state, &project_id)?;

    // Sanitize name: lowercase, alphanumeric + hyphens
    let safe_name: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    if safe_name.is_empty() {
        return Err("Invalid workflow name".to_string());
    }

    let workflows_dir = root.join(".rustic/workflows");
    std::fs::create_dir_all(&workflows_dir).map_err(|e| e.to_string())?;

    let workflow_path = workflows_dir.join(format!("{}.md", safe_name));
    if workflow_path.exists() {
        return Err(format!("Workflow already exists: {}", safe_name));
    }

    let content = format!(
        "---\nname: {}\ndescription: {}\n---\n\n{}",
        safe_name, description, body
    );
    std::fs::write(&workflow_path, &content).map_err(|e| e.to_string())?;

    Ok(WorkflowInfo {
        name: safe_name,
        description,
    })
}

#[tauri::command]
pub fn delete_workflow(
    state: State<'_, AppState>,
    project_id: String,
    name: String,
) -> Result<(), String> {
    let root = project_root(&state, &project_id)?;
    let workflows = discover_workflows(&root);
    let workflow = workflows
        .iter()
        .find(|w| w.name == name)
        .ok_or_else(|| format!("Workflow not found: {}", name))?;

    std::fs::remove_file(&workflow.path).map_err(|e| e.to_string())?;
    Ok(())
}

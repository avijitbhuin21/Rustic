//! Per-project user preferences (default model, provider, permission
//! level, thinking effort). Stored in projects.settings_json.

use crate::state::AppState;
use crate::sync_ext::MutexExt;
use serde::{Deserialize, Serialize};
use tauri::State;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProjectDefaults {
    pub model: Option<String>,
    pub provider_type: Option<String>,
    pub permission_level: Option<String>,
    pub thinking_effort: Option<String>,
}

#[tauri::command]
pub fn get_project_defaults(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<ProjectDefaults, String> {
    let db = state.db.lock_safe();
    if let Ok(Some(project)) = db.get_project(&project_id) {
        if let Some(json) = &project.settings_json {
            if let Ok(defaults) = serde_json::from_str::<ProjectDefaults>(json) {
                return Ok(defaults);
            }
        }
    }
    Ok(ProjectDefaults::default())
}

#[tauri::command]
pub fn save_project_defaults(
    state: State<'_, AppState>,
    project_id: String,
    defaults: ProjectDefaults,
) -> Result<(), String> {
    let json = serde_json::to_string(&defaults).map_err(|e| e.to_string())?;
    let db = state.db.lock_safe();
    db.update_project_settings(&project_id, Some(&json))
        .map_err(|e| e.to_string())
}

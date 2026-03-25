use crate::state::AppState;
use rustic_core::search::{SearchEngine, SearchQuery, SearchResult};
use std::path::PathBuf;
use serde::Serialize;
use tauri::State;

#[tauri::command]
pub fn search_in_project(
    state: State<'_, AppState>,
    project_id: String,
    pattern: String,
    is_regex: bool,
    case_sensitive: bool,
    whole_word: bool,
    include_glob: Option<String>,
    exclude_glob: Option<String>,
) -> Result<Vec<SearchResult>, String> {
    let workspace = state.workspace.lock().unwrap();
    let project = workspace
        .list_projects()
        .into_iter()
        .find(|p| p.id.to_string() == project_id)
        .ok_or_else(|| format!("Project not found: {}", project_id))?;

    let query = SearchQuery {
        pattern,
        is_regex,
        case_sensitive,
        whole_word,
        paths: vec![PathBuf::from(&project.root_path)],
        include_glob,
        exclude_glob,
    };

    SearchEngine::search(&query).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn search_global(
    state: State<'_, AppState>,
    pattern: String,
    is_regex: bool,
    case_sensitive: bool,
    whole_word: bool,
    include_glob: Option<String>,
    exclude_glob: Option<String>,
) -> Result<Vec<SearchResult>, String> {
    let workspace = state.workspace.lock().unwrap();
    let paths: Vec<PathBuf> = workspace
        .list_projects()
        .iter()
        .map(|p| PathBuf::from(&p.root_path))
        .collect();

    if paths.is_empty() {
        return Ok(Vec::new());
    }

    let query = SearchQuery {
        pattern,
        is_regex,
        case_sensitive,
        whole_word,
        paths,
        include_glob,
        exclude_glob,
    };

    SearchEngine::search(&query).map_err(|e| e.to_string())
}

#[derive(Serialize)]
pub struct ReplaceResult {
    pub replacements: u32,
}

#[tauri::command]
pub fn replace_in_file(
    path: String,
    pattern: String,
    replacement: String,
    is_regex: bool,
    case_sensitive: bool,
    whole_word: bool,
) -> Result<ReplaceResult, String> {
    let count = SearchEngine::replace_in_file(&path, &pattern, &replacement, is_regex, case_sensitive, whole_word)
        .map_err(|e| e.to_string())?;
    Ok(ReplaceResult { replacements: count })
}

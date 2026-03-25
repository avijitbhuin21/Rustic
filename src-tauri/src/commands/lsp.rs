use crate::state::AppState;
use rustic_core::lsp::manager::path_to_uri;
use serde::Serialize;
use tauri::State;

#[derive(Clone, Serialize)]
pub struct CompletionEntry {
    pub label: String,
    pub kind: String,
    pub detail: Option<String>,
    pub insert_text: Option<String>,
}

#[derive(Clone, Serialize)]
pub struct HoverInfo {
    pub contents: String,
}

#[derive(Clone, Serialize)]
pub struct LocationInfo {
    pub file_path: String,
    pub line: u32,
    pub col: u32,
}

#[derive(Clone, Serialize)]
pub struct FormatEdit {
    pub start_line: u32,
    pub start_col: u32,
    pub end_line: u32,
    pub end_col: u32,
    pub new_text: String,
}

/// Helper: get file path and text from buffer
fn get_buffer_info(state: &AppState, buffer_id: u64) -> Result<(String, String, String), String> {
    let buffers = state.buffers.lock().unwrap();
    let buffer = buffers
        .get(&buffer_id)
        .ok_or_else(|| format!("Buffer not found: {}", buffer_id))?;
    let file_path = buffer
        .file_path
        .as_ref()
        .ok_or_else(|| "Buffer has no file path".to_string())?
        .to_string_lossy()
        .to_string();
    let text = buffer.rope.to_string();
    let ext = buffer
        .file_path
        .as_ref()
        .and_then(|p| p.extension())
        .map(|e| e.to_string_lossy().to_string())
        .unwrap_or_default();
    Ok((file_path, text, ext))
}

/// Helper: find the project root for a given file path
fn find_project_root(state: &AppState, file_path: &str) -> Result<String, String> {
    let workspace = state.workspace.lock().unwrap();
    for project in workspace.list_projects() {
        if file_path.starts_with(&project.root_path.to_string_lossy().to_string()) {
            return Ok(project.root_path.to_string_lossy().to_string());
        }
    }
    // Fallback: use the file's parent directory
    let path = std::path::Path::new(file_path);
    Ok(path
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default())
}

#[tauri::command]
pub fn lsp_notify_open(
    state: State<'_, AppState>,
    buffer_id: u64,
) -> Result<(), String> {
    let (file_path, text, ext) = get_buffer_info(&state, buffer_id)?;
    let project_root = find_project_root(&state, &file_path)?;
    let uri = path_to_uri(&file_path);

    let mut lsp = state.lsp_manager.lock().unwrap();
    if let Some(client) = lsp.get_or_start(&project_root, &ext).map_err(|e| e.to_string())? {
        let lang = client.language_id().to_string();
        let _ = client.did_open(&uri, &lang, 1, &text);
    }
    Ok(())
}

#[tauri::command]
pub fn lsp_notify_change(
    state: State<'_, AppState>,
    buffer_id: u64,
    version: i32,
) -> Result<(), String> {
    let (file_path, text, ext) = get_buffer_info(&state, buffer_id)?;
    let project_root = find_project_root(&state, &file_path)?;
    let uri = path_to_uri(&file_path);

    let lsp = state.lsp_manager.lock().unwrap();
    let config = lsp.config_for_extension(&ext);
    if let Some(config) = config {
        if let Some(client) = lsp.get_client(&project_root, &config.language_id) {
            let _ = client.did_change(&uri, version, &text);
        }
    }
    Ok(())
}

#[tauri::command]
pub fn lsp_notify_save(
    state: State<'_, AppState>,
    buffer_id: u64,
) -> Result<(), String> {
    let (file_path, text, ext) = get_buffer_info(&state, buffer_id)?;
    let project_root = find_project_root(&state, &file_path)?;
    let uri = path_to_uri(&file_path);

    let lsp = state.lsp_manager.lock().unwrap();
    let config = lsp.config_for_extension(&ext);
    if let Some(config) = config {
        if let Some(client) = lsp.get_client(&project_root, &config.language_id) {
            let _ = client.did_save(&uri, Some(&text));
        }
    }
    Ok(())
}

#[tauri::command]
pub fn lsp_notify_close(
    state: State<'_, AppState>,
    buffer_id: u64,
) -> Result<(), String> {
    let (file_path, _text, ext) = get_buffer_info(&state, buffer_id)?;
    let project_root = find_project_root(&state, &file_path)?;
    let uri = path_to_uri(&file_path);

    let lsp = state.lsp_manager.lock().unwrap();
    let config = lsp.config_for_extension(&ext);
    if let Some(config) = config {
        if let Some(client) = lsp.get_client(&project_root, &config.language_id) {
            let _ = client.did_close(&uri);
        }
    }
    Ok(())
}

#[tauri::command]
pub fn get_completions(
    state: State<'_, AppState>,
    buffer_id: u64,
    line: u32,
    col: u32,
) -> Result<Vec<CompletionEntry>, String> {
    let (file_path, _text, ext) = get_buffer_info(&state, buffer_id)?;
    let project_root = find_project_root(&state, &file_path)?;
    let uri = path_to_uri(&file_path);

    let lsp = state.lsp_manager.lock().unwrap();
    let config = match lsp.config_for_extension(&ext) {
        Some(c) => c.clone(),
        None => return Ok(Vec::new()),
    };
    let client = match lsp.get_client(&project_root, &config.language_id) {
        Some(c) => c,
        None => return Ok(Vec::new()),
    };

    let items = client.completion_simple(&uri, line, col).map_err(|e| e.to_string())?;

    Ok(items
        .into_iter()
        .map(|(label, kind, detail, insert_text)| CompletionEntry {
            label, kind, detail, insert_text,
        })
        .collect())
}

#[tauri::command]
pub fn get_hover(
    state: State<'_, AppState>,
    buffer_id: u64,
    line: u32,
    col: u32,
) -> Result<Option<HoverInfo>, String> {
    let (file_path, _text, ext) = get_buffer_info(&state, buffer_id)?;
    let project_root = find_project_root(&state, &file_path)?;
    let uri = path_to_uri(&file_path);

    let lsp = state.lsp_manager.lock().unwrap();
    let config = match lsp.config_for_extension(&ext) {
        Some(c) => c.clone(),
        None => return Ok(None),
    };
    let client = match lsp.get_client(&project_root, &config.language_id) {
        Some(c) => c,
        None => return Ok(None),
    };

    let contents = client.hover_string(&uri, line, col).map_err(|e| e.to_string())?;
    Ok(contents.map(|c| HoverInfo { contents: c }))
}

#[tauri::command]
pub fn goto_definition(
    state: State<'_, AppState>,
    buffer_id: u64,
    line: u32,
    col: u32,
) -> Result<Vec<LocationInfo>, String> {
    let (file_path, _text, ext) = get_buffer_info(&state, buffer_id)?;
    let project_root = find_project_root(&state, &file_path)?;
    let uri = path_to_uri(&file_path);

    let lsp = state.lsp_manager.lock().unwrap();
    let config = match lsp.config_for_extension(&ext) {
        Some(c) => c.clone(),
        None => return Ok(Vec::new()),
    };
    let client = match lsp.get_client(&project_root, &config.language_id) {
        Some(c) => c,
        None => return Ok(Vec::new()),
    };

    let locations = client
        .goto_definition_simple(&uri, line, col)
        .map_err(|e| e.to_string())?;

    use rustic_core::lsp::manager::uri_to_path;
    Ok(locations
        .into_iter()
        .map(|(uri_str, l, c)| LocationInfo {
            file_path: uri_to_path(&uri_str),
            line: l,
            col: c,
        })
        .collect())
}

#[tauri::command]
pub fn format_document(
    state: State<'_, AppState>,
    buffer_id: u64,
) -> Result<Vec<FormatEdit>, String> {
    let (file_path, _text, ext) = get_buffer_info(&state, buffer_id)?;
    let project_root = find_project_root(&state, &file_path)?;
    let uri = path_to_uri(&file_path);

    let lsp = state.lsp_manager.lock().unwrap();
    let config = match lsp.config_for_extension(&ext) {
        Some(c) => c.clone(),
        None => return Ok(Vec::new()),
    };
    let client = match lsp.get_client(&project_root, &config.language_id) {
        Some(c) => c,
        None => return Ok(Vec::new()),
    };

    let edits = client
        .format_simple(&uri, 4, true)
        .map_err(|e| e.to_string())?;

    Ok(edits
        .into_iter()
        .map(|(sl, sc, el, ec, text)| FormatEdit {
            start_line: sl, start_col: sc, end_line: el, end_col: ec, new_text: text,
        })
        .collect())
}

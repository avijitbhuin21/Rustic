//! Search commands — desktop adapters. The bodies live in
//! `rustic_app::search_ops`; these wrappers only do Tauri parameter
//! extraction and wrap the `AppHandle` in a `TauriEmitter` so the shared
//! streaming search can emit `search-event` payloads unchanged.

use std::sync::Arc;

use rustic_app::search_ops::{
    self, FileReplacePlan, ReplaceAllResult, ReplaceResult, SearchParams,
};
use tauri::{AppHandle, State};

use crate::state::AppState;
use crate::transport::TauriEmitter;

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn start_search(
    state: State<'_, AppState>,
    app: AppHandle,
    scopes: Vec<String>,
    pattern: String,
    is_regex: bool,
    case_sensitive: bool,
    whole_word: bool,
    include_glob: Option<String>,
    exclude_glob: Option<String>,
) -> Result<u64, String> {
    let emitter: Arc<dyn rustic_app::EventEmitter> = Arc::new(TauriEmitter::new(app));
    Ok(search_ops::start_search(
        &state,
        emitter,
        SearchParams {
            scopes,
            pattern,
            is_regex,
            case_sensitive,
            whole_word,
            include_glob,
            exclude_glob,
        },
    ))
}

#[tauri::command]
pub fn cancel_search(state: State<'_, AppState>) {
    search_ops::cancel_search(&state);
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
    search_ops::replace_in_file(
        &path,
        &pattern,
        &replacement,
        is_regex,
        case_sensitive,
        whole_word,
    )
}

#[tauri::command]
pub fn replace_all_in_files(
    plans: Vec<FileReplacePlan>,
    pattern: String,
    replacement: String,
    is_regex: bool,
    case_sensitive: bool,
    whole_word: bool,
) -> Result<ReplaceAllResult, String> {
    Ok(search_ops::replace_all_in_files(
        plans,
        &pattern,
        &replacement,
        is_regex,
        case_sensitive,
        whole_word,
    ))
}

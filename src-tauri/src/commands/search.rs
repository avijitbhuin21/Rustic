use crate::state::AppState;
use rustic_core::search::{SearchEngine, SearchQuery, SearchResult, SearchSummary};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tauri::{AppHandle, Emitter, State};

const GLOBAL_MAX_TOTAL_MATCHES: u32 = 5000;
const GLOBAL_MAX_FILES_MATCHED: u32 = 1500;

// Time-based batching: emit at most ~10 FileMatch IPC events/second.
// Each emit() in Tauri calls WebView2 ExecuteScript on the UI thread.
// Flooding it at file-walker speed (thousands of files/second) saturates
// the Windows message queue and freezes the UI.
const EMIT_INTERVAL_MS: u128 = 100;
const BATCH_SIZE_CAP: usize = 100;

#[derive(Serialize, Clone)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum SearchEvent {
    FileMatch {
        search_id: u64,
        results: Vec<SearchResult>,
    },
    Completed {
        search_id: u64,
        files_scanned: u32,
        files_matched: u32,
        total_matches: u32,
        truncated: bool,
        cancelled: bool,
    },
}

struct ScopeRoot {
    path: PathBuf,
}

fn resolve_scope(state: &AppState, scopes: &[String]) -> Result<Vec<ScopeRoot>, String> {
    let workspace = state.workspace.lock().unwrap();
    let all_projects = workspace.list_projects();
    Ok(scopes
        .iter()
        .filter_map(|scope_id| {
            all_projects
                .iter()
                .find(|p| p.id.to_string() == *scope_id)
                .map(|p| ScopeRoot { path: PathBuf::from(&p.root_path) })
        })
        .collect())
}

struct BatchState {
    pending: Vec<SearchResult>,
    last_emit: Instant,
}

#[tauri::command]
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
    let roots = resolve_scope(&state, &scopes)?;
    let id = state.active_search_id.fetch_add(1, Ordering::SeqCst) + 1;

    if pattern.is_empty() || roots.is_empty() {
        return Ok(id);
    }

    let active = state.active_search_id.clone();
    let app_for_task = app.clone();

    tokio::task::spawn_blocking(move || {
        let root_total = roots.len() as u32;
        let mut accumulated_files_scanned: u32 = 0;
        let mut accumulated_files_matched: u32 = 0;
        let mut accumulated_total_matches: u32 = 0;
        let mut truncated_global = false;

        for (i, root) in roots.iter().enumerate() {
            if active.load(Ordering::Relaxed) != id {
                break;
            }

            let projects_left = (root_total - i as u32).max(1);
            let match_budget = GLOBAL_MAX_TOTAL_MATCHES
                .saturating_sub(accumulated_total_matches)
                / projects_left;
            let files_budget = GLOBAL_MAX_FILES_MATCHED
                .saturating_sub(accumulated_files_matched)
                / projects_left;

            if match_budget == 0 || files_budget == 0 {
                truncated_global = true;
                break;
            }

            let query = SearchQuery {
                pattern: pattern.clone(),
                is_regex,
                case_sensitive,
                whole_word,
                paths: vec![root.path.clone()],
                include_glob: include_glob.clone(),
                exclude_glob: exclude_glob.clone(),
            };

            let batch_state: Arc<Mutex<BatchState>> = Arc::new(Mutex::new(BatchState {
                pending: Vec::new(),
                last_emit: Instant::now(),
            }));
            let state_ref = Arc::clone(&batch_state);
            let app_match = app_for_task.clone();

            let on_file = move |result: SearchResult| {
                let mut s = state_ref.lock().unwrap();
                s.pending.push(result);
                let should_flush = s.last_emit.elapsed().as_millis() >= EMIT_INTERVAL_MS
                    || s.pending.len() >= BATCH_SIZE_CAP;
                if should_flush {
                    let to_emit = std::mem::take(&mut s.pending);
                    s.last_emit = Instant::now();
                    drop(s);
                    let _ = app_match.emit(
                        "search-event",
                        SearchEvent::FileMatch { search_id: id, results: to_emit },
                    );
                }
            };

            // Yield the search thread to the UI thread every 16 files.
            // The Windows message pump (which drives WebView2 event dispatch)
            // runs in this same process. Without periodic yields the search
            // thread monopolises the CPU and the UI freezes — even though it
            // runs on a separate thread — because the OS gives it no
            // opportunity to process pending window messages.
            let mut yield_counter: u32 = 0;
            let active_for_check = active.clone();
            let should_continue = move |summary: SearchSummary| -> bool {
                if active_for_check.load(Ordering::Relaxed) != id {
                    return false;
                }
                if summary.total_matches >= match_budget
                    || summary.files_matched >= files_budget
                {
                    return false;
                }
                yield_counter = yield_counter.wrapping_add(1);
                if yield_counter % 16 == 0 {
                    std::thread::yield_now();
                }
                true
            };

            let summary = SearchEngine::search_streaming(&query, on_file, should_continue)
                .unwrap_or_default();

            {
                let mut s = batch_state.lock().unwrap();
                if !s.pending.is_empty() {
                    let to_emit = std::mem::take(&mut s.pending);
                    drop(s);
                    let _ = app_for_task.emit(
                        "search-event",
                        SearchEvent::FileMatch { search_id: id, results: to_emit },
                    );
                }
            }

            accumulated_files_scanned =
                accumulated_files_scanned.saturating_add(summary.files_scanned);
            accumulated_files_matched =
                accumulated_files_matched.saturating_add(summary.files_matched);
            accumulated_total_matches =
                accumulated_total_matches.saturating_add(summary.total_matches);
            if summary.truncated {
                truncated_global = true;
            }
        }

        let cancelled = active.load(Ordering::Relaxed) != id;
        let _ = app_for_task.emit(
            "search-event",
            SearchEvent::Completed {
                search_id: id,
                files_scanned: accumulated_files_scanned,
                files_matched: accumulated_files_matched,
                total_matches: accumulated_total_matches,
                truncated: truncated_global,
                cancelled,
            },
        );
    });

    Ok(id)
}

#[tauri::command]
pub fn cancel_search(state: State<'_, AppState>) {
    state.active_search_id.fetch_add(1, Ordering::SeqCst);
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
    let count = SearchEngine::replace_in_file(
        &path,
        &pattern,
        &replacement,
        is_regex,
        case_sensitive,
        whole_word,
    )
    .map_err(|e| e.to_string())?;
    Ok(ReplaceResult { replacements: count })
}

//! Shared search command bodies — the transport-agnostic core behind the
//! desktop `#[tauri::command]`s in `src-tauri/src/commands/search.rs` and the
//! server dispatch in `rustic-server/src/commands/search.rs`.
//!
//! Streaming `FileMatch` / `Completed` events go out through the injected
//! [`EventEmitter`] (desktop: `AppHandle::emit`; server: WS broadcast hub), so
//! the payload shape — event name `"search-event"`, camelCase-tagged
//! [`SearchEvent`] — is identical on both transports.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::{Deserialize, Serialize};

use rustic_core::search::{SearchEngine, SearchQuery, SearchResult, SearchSummary};

use crate::context::{EventEmitter, EventEmitterExt};
use crate::state::AppState;
use crate::sync_ext::MutexExt;

const GLOBAL_MAX_TOTAL_MATCHES: u32 = 5000;
const GLOBAL_MAX_FILES_MATCHED: u32 = 1500;

// Time-based batching: emit at most ~10 FileMatch IPC events/second.
// Each emit() in Tauri calls WebView2 ExecuteScript on the UI thread.
// Flooding it at file-walker speed (thousands of files/second) saturates
// the Windows message queue and freezes the UI. (The server's WS hub is far
// more tolerant, but the same cadence keeps both transports identical.)
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

struct BatchState {
    pending: Vec<SearchResult>,
    last_emit: Instant,
}

/// The search parameters both transports receive from the frontend.
/// `camelCase` so the server can deserialize the wire args directly; the
/// desktop builds it from Tauri's already-converted snake_case params.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchParams {
    pub scopes: Vec<String>,
    pub pattern: String,
    pub is_regex: bool,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub include_glob: Option<String>,
    pub exclude_glob: Option<String>,
}

/// Map scope ids (project ids) to on-disk roots via the in-memory workspace.
/// Unknown ids are silently skipped, matching historical behavior.
fn resolve_scope(state: &AppState, scopes: &[String]) -> Vec<PathBuf> {
    let workspace = state.workspace.lock_safe();
    let all_projects = workspace.list_projects();
    scopes
        .iter()
        .filter_map(|scope_id| {
            all_projects
                .iter()
                .find(|p| p.id.to_string() == *scope_id)
                .map(|p| PathBuf::from(&p.root_path))
        })
        .collect()
}

/// Start a streaming search across the given project scopes. Returns the new
/// search id immediately; results arrive as `"search-event"` events on the
/// emitter. Must be called from within a tokio runtime (both hosts are).
pub fn start_search(state: &AppState, emitter: Arc<dyn EventEmitter>, params: SearchParams) -> u64 {
    let roots = resolve_scope(state, &params.scopes);
    let id = state.active_search_id.fetch_add(1, Ordering::SeqCst) + 1;

    if params.pattern.is_empty() || roots.is_empty() {
        return id;
    }

    let active = state.active_search_id.clone();
    let SearchParams {
        pattern,
        is_regex,
        case_sensitive,
        whole_word,
        include_glob,
        exclude_glob,
        ..
    } = params;

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
            let match_budget =
                GLOBAL_MAX_TOTAL_MATCHES.saturating_sub(accumulated_total_matches) / projects_left;
            let files_budget =
                GLOBAL_MAX_FILES_MATCHED.saturating_sub(accumulated_files_matched) / projects_left;

            if match_budget == 0 || files_budget == 0 {
                truncated_global = true;
                break;
            }

            let query = SearchQuery {
                pattern: pattern.clone(),
                is_regex,
                case_sensitive,
                whole_word,
                paths: vec![root.clone()],
                include_glob: include_glob.clone(),
                exclude_glob: exclude_glob.clone(),
            };

            let batch_state: Arc<Mutex<BatchState>> = Arc::new(Mutex::new(BatchState {
                pending: Vec::new(),
                last_emit: Instant::now(),
            }));
            let state_ref = Arc::clone(&batch_state);
            let emit_match = Arc::clone(&emitter);

            let on_file = move |result: SearchResult| {
                let mut s = state_ref.lock_safe();
                s.pending.push(result);
                let should_flush = s.last_emit.elapsed().as_millis() >= EMIT_INTERVAL_MS
                    || s.pending.len() >= BATCH_SIZE_CAP;
                if should_flush {
                    let to_emit = std::mem::take(&mut s.pending);
                    s.last_emit = Instant::now();
                    drop(s);
                    emit_match.emit(
                        "search-event",
                        SearchEvent::FileMatch {
                            search_id: id,
                            results: to_emit,
                        },
                    );
                }
            };

            // Yield the search thread to the UI thread every 16 files.
            // On desktop the Windows message pump (which drives WebView2 event
            // dispatch) runs in this same process. Without periodic yields the
            // search thread monopolises the CPU and the UI freezes — even
            // though it runs on a separate thread — because the OS gives it no
            // opportunity to process pending window messages.
            let mut yield_counter: u32 = 0;
            let active_for_check = active.clone();
            let should_continue = move |summary: SearchSummary| -> bool {
                if active_for_check.load(Ordering::Relaxed) != id {
                    return false;
                }
                if summary.total_matches >= match_budget || summary.files_matched >= files_budget {
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
                let mut s = batch_state.lock_safe();
                if !s.pending.is_empty() {
                    let to_emit = std::mem::take(&mut s.pending);
                    drop(s);
                    emitter.emit(
                        "search-event",
                        SearchEvent::FileMatch {
                            search_id: id,
                            results: to_emit,
                        },
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
        emitter.emit(
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

    id
}

/// Cancel any in-flight search by bumping the active id past it.
pub fn cancel_search(state: &AppState) {
    state.active_search_id.fetch_add(1, Ordering::SeqCst);
}

#[derive(Serialize)]
pub struct ReplaceResult {
    pub replacements: u32,
}

pub fn replace_in_file(
    path: &str,
    pattern: &str,
    replacement: &str,
    is_regex: bool,
    case_sensitive: bool,
    whole_word: bool,
) -> Result<ReplaceResult, String> {
    let count = SearchEngine::replace_in_file(
        path,
        pattern,
        replacement,
        is_regex,
        case_sensitive,
        whole_word,
    )
    .map_err(|e| e.to_string())?;
    Ok(ReplaceResult {
        replacements: count,
    })
}

/// One file's slice of a "Replace All": the file path plus the ordinals of
/// matches the user dismissed (0-based index in the search's match list for
/// that file). A fully-dismissed file is simply omitted from the plan list by
/// the frontend, so an empty `excluded_ordinals` means "replace every match".
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileReplacePlan {
    pub path: String,
    pub excluded_ordinals: Vec<usize>,
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ReplaceAllResult {
    pub files_changed: u32,
    pub replacements: u32,
    /// `(path, error)` for files that failed — the rest still apply.
    pub errors: Vec<(String, String)>,
}

/// VS Code-style "Replace All": apply `replacement` across many files in one
/// call, honoring per-file and per-match exclusions. Each file is processed
/// independently — one failure (e.g. a file deleted since the search) is
/// collected in `errors` and never aborts the others.
pub fn replace_all_in_files(
    plans: Vec<FileReplacePlan>,
    pattern: &str,
    replacement: &str,
    is_regex: bool,
    case_sensitive: bool,
    whole_word: bool,
) -> ReplaceAllResult {
    let mut out = ReplaceAllResult::default();
    for plan in plans {
        let excluded: std::collections::HashSet<usize> =
            plan.excluded_ordinals.into_iter().collect();
        match SearchEngine::replace_in_file_excluding(
            &plan.path,
            pattern,
            replacement,
            is_regex,
            case_sensitive,
            whole_word,
            &excluded,
        ) {
            Ok(0) => {}
            Ok(n) => {
                out.files_changed += 1;
                out.replacements += n;
            }
            Err(e) => out.errors.push((plan.path, e.to_string())),
        }
    }
    out
}

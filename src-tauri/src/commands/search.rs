use crate::state::AppState;
use rustic_core::search::{SearchEngine, SearchQuery, SearchResult, SearchSummary};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::Instant;
use tauri::{AppHandle, Emitter, State};

/// Hard ceiling for the entire scope (all projects combined). Past this point
/// the UI gets noisy and the renderer slows down regardless of caps elsewhere.
const GLOBAL_MAX_TOTAL_MATCHES: u32 = 5000;
const GLOBAL_MAX_FILES_MATCHED: u32 = 1500;

/// Streamed payloads pushed to the frontend via the `search-event` Tauri
/// event channel. One channel multiplexed by `search_id` so concurrent or
/// rapidly-superseded searches don't tangle — the frontend filters out any
/// event whose id isn't its current search.
#[derive(Serialize, Clone)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum SearchEvent {
    /// File with at least one match — appended to the results list as it arrives.
    FileMatch {
        search_id: u64,
        result: SearchResult,
    },
    /// Periodic heartbeat. `accumulated_files_scanned` is the total scanned
    /// across all projects so far (current project's scanned count plus all
    /// previously-completed projects). Used by the UI to show "scanned N files".
    Progress {
        search_id: u64,
        root_index: u32,
        root_total: u32,
        accumulated_files_scanned: u32,
    },
    /// Walker moved on to a new project root. Frontend uses this to render
    /// "Searching [project name] (i of N)" in the summary line.
    RootStarted {
        search_id: u64,
        index: u32,
        total: u32,
        project_name: String,
        path: String,
    },
    /// Project finished (either ran to completion or hit its budget).
    RootCompleted {
        search_id: u64,
        index: u32,
        total: u32,
        project_name: String,
        files_scanned: u32,
        files_matched: u32,
        total_matches: u32,
    },
    /// Terminal event. `cancelled = true` means the search was preempted by
    /// a newer one (or by an explicit cancel) and the summary is partial.
    Completed {
        search_id: u64,
        files_scanned: u32,
        files_matched: u32,
        total_matches: u32,
        truncated: bool,
        cancelled: bool,
    },
}

/// One scope entry — the project's display name plus its root path. Resolved
/// under the workspace lock and then moved into the blocking task.
struct ScopeRoot {
    name: String,
    path: PathBuf,
}

/// Resolve the scope string ("global" or a project id) into the ordered list
/// of project roots to walk. Done under the workspace lock then released
/// before any awaiting so we don't hold a sync mutex across `spawn_blocking`.
fn resolve_scope(state: &AppState, scope: &str) -> Result<Vec<ScopeRoot>, String> {
    let workspace = state.workspace.lock().unwrap();
    if scope == "global" {
        Ok(workspace
            .list_projects()
            .iter()
            .map(|p| ScopeRoot {
                name: p.name.clone(),
                path: PathBuf::from(&p.root_path),
            })
            .collect())
    } else {
        workspace
            .list_projects()
            .into_iter()
            .find(|p| p.id.to_string() == scope)
            .map(|p| {
                vec![ScopeRoot {
                    name: p.name.clone(),
                    path: PathBuf::from(&p.root_path),
                }]
            })
            .ok_or_else(|| format!("Project not found: {}", scope))
    }
}

/// Kick off a streaming search. Returns the new `search_id` immediately
/// (does not wait for completion). Results stream to the frontend as
/// `search-event` Tauri events until a `Completed` event arrives.
///
/// Projects are walked **sequentially** on the tokio blocking pool — one
/// project finishes (or hits its dynamic budget) before the next starts.
/// Sequential keeps IO pressure low and result order predictable; parallel
/// would interleave matches and compete with the rest of the IDE for disk.
///
/// Each project gets a *dynamic* slice of the remaining global budget:
/// `(global_cap − accumulated) / projects_left`. So if project 1 of 5
/// under-uses its slice, project 2 inherits the leftover; if a single
/// project hogs everything, the others still get a fair share of what
/// remains. With one project in scope, that project gets the full budget.
///
/// Cancellation is implicit: starting a new search bumps
/// `state.active_search_id`, and the background task checks that counter
/// between every file and between every project.
#[tauri::command]
pub async fn start_search(
    state: State<'_, AppState>,
    app: AppHandle,
    scope: String,
    pattern: String,
    is_regex: bool,
    case_sensitive: bool,
    whole_word: bool,
    include_glob: Option<String>,
    exclude_glob: Option<String>,
) -> Result<u64, String> {
    let roots = resolve_scope(&state, &scope)?;

    // Always bump the counter, even on an empty query — it cancels any
    // in-flight walk from a previous keystroke.
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
            // Bail between projects if a newer search has superseded us.
            if active.load(Ordering::Relaxed) != id {
                break;
            }

            // Dynamic per-project budget: split whatever's left of the global
            // cap evenly across the remaining projects. First project of one
            // gets the full budget; second of five gets 1/4 of the remaining,
            // and so on.
            let projects_left = (root_total - i as u32).max(1);
            let match_budget = GLOBAL_MAX_TOTAL_MATCHES
                .saturating_sub(accumulated_total_matches)
                / projects_left;
            let files_budget = GLOBAL_MAX_FILES_MATCHED
                .saturating_sub(accumulated_files_matched)
                / projects_left;

            // Global cap exhausted — flag and stop without entering this project.
            if match_budget == 0 || files_budget == 0 {
                truncated_global = true;
                break;
            }

            let _ = app_for_task.emit(
                "search-event",
                SearchEvent::RootStarted {
                    search_id: id,
                    index: i as u32,
                    total: root_total,
                    project_name: root.name.clone(),
                    path: root.path.to_string_lossy().to_string(),
                },
            );

            let query = SearchQuery {
                pattern: pattern.clone(),
                is_regex,
                case_sensitive,
                whole_word,
                paths: vec![root.path.clone()],
                include_glob: include_glob.clone(),
                exclude_glob: exclude_glob.clone(),
            };

            // Per-project progress throttle state. Reset each project so the
            // first scanned-files heartbeat fires quickly when a new project
            // starts (gives the user instant feedback that the walker moved on).
            let mut last_progress = Instant::now();
            let mut last_progress_scanned: u32 = 0;

            let app_match = app_for_task.clone();
            let on_file = move |result: SearchResult| {
                let _ = app_match.emit(
                    "search-event",
                    SearchEvent::FileMatch {
                        search_id: id,
                        result,
                    },
                );
            };

            let app_tick = app_for_task.clone();
            let active_for_check = active.clone();
            let acc_scanned_snapshot = accumulated_files_scanned;
            let should_continue = move |summary: SearchSummary| -> bool {
                // Hard checks: cancellation and per-project budget. Either
                // returns false to stop the walk for THIS project.
                if active_for_check.load(Ordering::Relaxed) != id {
                    return false;
                }
                if summary.total_matches >= match_budget
                    || summary.files_matched >= files_budget
                {
                    return false;
                }

                // Throttled progress heartbeat (50ms or 100 files, whichever first).
                let elapsed_ms = last_progress.elapsed().as_millis();
                let scanned_delta = summary
                    .files_scanned
                    .saturating_sub(last_progress_scanned);
                if elapsed_ms >= 50 || scanned_delta >= 100 {
                    let _ = app_tick.emit(
                        "search-event",
                        SearchEvent::Progress {
                            search_id: id,
                            root_index: i as u32,
                            root_total,
                            accumulated_files_scanned: acc_scanned_snapshot
                                + summary.files_scanned,
                        },
                    );
                    last_progress = Instant::now();
                    last_progress_scanned = summary.files_scanned;
                }
                true
            };

            let summary = SearchEngine::search_streaming(&query, on_file, should_continue)
                .unwrap_or_default();

            accumulated_files_scanned =
                accumulated_files_scanned.saturating_add(summary.files_scanned);
            accumulated_files_matched =
                accumulated_files_matched.saturating_add(summary.files_matched);
            accumulated_total_matches =
                accumulated_total_matches.saturating_add(summary.total_matches);
            if summary.truncated {
                // Engine-level truncation in any project bubbles up.
                truncated_global = true;
            }

            let _ = app_for_task.emit(
                "search-event",
                SearchEvent::RootCompleted {
                    search_id: id,
                    index: i as u32,
                    total: root_total,
                    project_name: root.name.clone(),
                    files_scanned: summary.files_scanned,
                    files_matched: summary.files_matched,
                    total_matches: summary.total_matches,
                },
            );
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

/// Cancel any in-flight search. Bumping the counter alone is enough — the
/// running task checks it between every file and between every project, and
/// bails out. Frontend should still ignore any late events that slip through
/// (last file emits before the cancel check runs).
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
    Ok(ReplaceResult {
        replacements: count,
    })
}

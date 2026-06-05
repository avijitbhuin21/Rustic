//! Search commands — server dispatch. Mirrors the desktop bodies in
//! `src-tauri/src/commands/search.rs`. Streaming `FileMatch`/`Completed`
//! events are published on the WS hub via the `ServerContext` emitter instead
//! of Tauri's `AppHandle::emit`, but the payload shape is identical.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use rustic_app::context::{AppContext, EventEmitterExt};
use rustic_app::state::AppState;
use rustic_app::sync_ext::MutexExt;
use rustic_core::search::{SearchEngine, SearchQuery, SearchResult, SearchSummary};

use crate::api::{ok, parse, ApiError};
use crate::context::ServerContext;

const GLOBAL_MAX_TOTAL_MATCHES: u32 = 5000;
const GLOBAL_MAX_FILES_MATCHED: u32 = 1500;
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

struct BatchState {
    pending: Vec<SearchResult>,
    last_emit: Instant,
}

pub async fn dispatch(
    ctx: &ServerContext,
    command: &str,
    args: &Value,
) -> Option<Result<Value, ApiError>> {
    Some(match command {
        "start_search" => start_search(ctx, args),
        "cancel_search" => {
            ctx.state().active_search_id.fetch_add(1, Ordering::SeqCst);
            ok(json!(null))
        }
        "replace_in_file" => replace_in_file(args),
        "replace_all_in_files" => replace_all_in_files(args),
        _ => return None,
    })
}

fn resolve_scope(state: &AppState, scopes: &[String]) -> Vec<ScopeRoot> {
    let workspace = state.workspace.lock_safe();
    let all_projects = workspace.list_projects();
    scopes
        .iter()
        .filter_map(|scope_id| {
            all_projects
                .iter()
                .find(|p| p.id.to_string() == *scope_id)
                .map(|p| ScopeRoot { path: PathBuf::from(&p.root_path) })
        })
        .collect()
}

fn start_search(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct A {
        scopes: Vec<String>,
        pattern: String,
        is_regex: bool,
        case_sensitive: bool,
        whole_word: bool,
        include_glob: Option<String>,
        exclude_glob: Option<String>,
    }
    let a: A = parse(args)?;
    let state = ctx.state();

    let roots = resolve_scope(state, &a.scopes);
    let id = state.active_search_id.fetch_add(1, Ordering::SeqCst) + 1;

    if a.pattern.is_empty() || roots.is_empty() {
        return ok(id);
    }

    let active = state.active_search_id.clone();
    // Clone the context so the spawned task can publish events on the hub.
    let emitter = ctx.clone();
    let pattern = a.pattern;
    let is_regex = a.is_regex;
    let case_sensitive = a.case_sensitive;
    let whole_word = a.whole_word;
    let include_glob = a.include_glob;
    let exclude_glob = a.exclude_glob;

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
            let emit_match = emitter.clone();

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
                        SearchEvent::FileMatch { search_id: id, results: to_emit },
                    );
                }
            };

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
                let mut s = batch_state.lock_safe();
                if !s.pending.is_empty() {
                    let to_emit = std::mem::take(&mut s.pending);
                    drop(s);
                    emitter.emit(
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

    ok(id)
}

#[derive(Serialize)]
struct ReplaceResult {
    replacements: u32,
}

fn replace_in_file(args: &Value) -> Result<Value, ApiError> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct A {
        path: String,
        pattern: String,
        replacement: String,
        is_regex: bool,
        case_sensitive: bool,
        whole_word: bool,
    }
    let a: A = parse(args)?;
    let count = SearchEngine::replace_in_file(
        &a.path,
        &a.pattern,
        &a.replacement,
        a.is_regex,
        a.case_sensitive,
        a.whole_word,
    )
    .map_err(|e| e.to_string())?;
    ok(ReplaceResult { replacements: count })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileReplacePlan {
    path: String,
    excluded_ordinals: Vec<usize>,
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
struct ReplaceAllResult {
    files_changed: u32,
    replacements: u32,
    errors: Vec<(String, String)>,
}

fn replace_all_in_files(args: &Value) -> Result<Value, ApiError> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct A {
        plans: Vec<FileReplacePlan>,
        pattern: String,
        replacement: String,
        is_regex: bool,
        case_sensitive: bool,
        whole_word: bool,
    }
    let a: A = parse(args)?;

    let mut out = ReplaceAllResult::default();
    for plan in a.plans {
        let excluded: std::collections::HashSet<usize> =
            plan.excluded_ordinals.into_iter().collect();
        match SearchEngine::replace_in_file_excluding(
            &plan.path,
            &a.pattern,
            &a.replacement,
            a.is_regex,
            a.case_sensitive,
            a.whole_word,
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
    ok(out)
}

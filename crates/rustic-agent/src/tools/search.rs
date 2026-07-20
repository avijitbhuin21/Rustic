use super::{ToolContext, ToolOutput};
use crate::provider::ToolDef;
use crate::task::permissions::Action;
use anyhow::Result;
use serde_json::{json, Value};

use super::reject_removed_batch;

pub fn definitions() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "grep_search".into(),
            description: "Search for a pattern in files under a REQUIRED `path`. Returns matching \
                          lines with file paths and line numbers. Runs on ripgrep's engine: \
                          parallel, streams files, and skips binary files instantly. \
                          \
                          SCOPE: `path` is mandatory — pass the narrowest subdirectory that \
                          covers what you need (e.g. 'src', 'crates/foo'); pass '.' only when \
                          you deliberately want the whole project. Files larger than 10 MB are \
                          skipped and listed in the result so you can read them directly. \
                          Searches longer than 60s stop and return partial results with a note. \
                          \
                          CONTEXT LINES: pass `context_before` and/or `context_after` (integers, \
                          capped at 10) to show N lines before/after each match, like `grep -B/-A`. \
                          `context` is a shorthand that sets both before and after (like `grep -C`). \
                          Context output uses `>` to mark matched lines and `:` for context lines, \
                          with `--` separators between match groups. The 100-result cap counts \
                          matches only, not context lines. Without context params, output is the \
                          same as before (file:line: content). Dot-folders (.git, .github, ...) \
                          and heavy build/dependency dirs (node_modules, target, dist, build, \
                          __pycache__, ...) are always skipped, regardless of .gitignore; \
                          `.rustic` stays in scope. Dot-files (.env, .gitignore, ...) are \
                          searched.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Search pattern (regex supported)" },
                    "path": { "type": "string", "description": "Subdirectory to search in, relative to project root (REQUIRED). Pass '.' to deliberately search the entire project." },
                    "include": { "type": "string", "description": "Glob pattern for files to include (e.g. '*.rs')" },
                    "exclude": { "type": "string", "description": "Glob pattern for files to exclude" },
                    "context_before": { "type": "integer", "description": "Number of lines to show before each match (like grep -B). Capped at 10." },
                    "context_after": { "type": "integer", "description": "Number of lines to show after each match (like grep -A). Capped at 10." },
                    "context": { "type": "integer", "description": "Shorthand: show N lines before AND after each match (like grep -C). Sets both context_before and context_after. Capped at 10." }
                },
                "required": ["query", "path"]
            }),
        },
        ToolDef {
            name: "glob".into(),
            description: "Find files by glob pattern. Returns matching file paths, newest first. \
                          Use this to LOCATE files before reading them — far cheaper than \
                          list_directory + read_file guessing. Respects .gitignore; dot-folders \
                          (.git, .github, ...) and heavy build/dependency dirs (node_modules, \
                          target, dist, build, ...) are always skipped, `.rustic` stays in scope, \
                          dot-files are matchable. \
                          Patterns support ** (recursive), * (any chars in one segment), \
                          ? (single char), and {a,b} alternatives. Results are capped at \
                          200 paths.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Glob pattern relative to project root. \
                                        Examples: 'src/**/*.rs', 'crates/*/Cargo.toml', \
                                        '**/README.md', 'tests/**/*.{js,ts}'."
                    },
                    "path": {
                        "type": "string",
                        "description": "Subdirectory to anchor the search under (relative to project root). \
                                        Omit to search the whole project."
                    }
                },
                "required": ["pattern"]
            }),
        },
    ]
}

pub async fn execute(
    name: &str,
    tool_use_id: &str,
    params: Value,
    context: &ToolContext,
) -> Result<ToolOutput> {
    if !context.check_permission(&Action::Read) {
        return Ok(ToolOutput {
            content: "Permission denied: read not allowed".into(),
            is_error: true,
            attachments: Vec::new(),
        });
    }

    if name == "glob" {
        return execute_glob_dispatch(params, context).await;
    }
    execute_grep_dispatch(tool_use_id, params, context).await
}

/// Walker filter: skip noise directories regardless of `.gitignore`.
///
/// Skips (a) any directory whose name starts with '.' (e.g. .git, .github, .venv)
/// and (b) any directory named in `file_tree::EXCLUDED_DIRS` (node_modules, target,
/// dist, build, ...). `.rustic` is carved out — it stays in scope so project memory
/// and rules are searchable. Dot-files (.env, .gitignore) are always searchable.
/// The walk root (depth 0) is always allowed so an explicitly-passed noise/dot path
/// still works.
fn skip_dot_dirs(entry: &ignore::DirEntry) -> bool {
    if entry.depth() == 0 {
        return true;
    }
    let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
    if !is_dir {
        return true;
    }
    !is_noise_dir_name(&entry.file_name().to_string_lossy())
}

/// True if a directory with this name should be excluded from grep/glob walks.
/// `.rustic` is always kept; other dot-dirs and `file_tree::EXCLUDED_DIRS` names
/// are excluded.
fn is_noise_dir_name(name: &str) -> bool {
    if name == ".rustic" {
        return false;
    }
    if name.starts_with('.') {
        return true;
    }
    crate::file_tree::EXCLUDED_DIRS.contains(&name)
}

// ── grep_search ──────────────────────────────────────────────────────────────

async fn execute_grep_dispatch(
    tool_use_id: &str,
    params: Value,
    context: &ToolContext,
) -> Result<ToolOutput> {
    if let Some(rejection) = reject_removed_batch(&params, "queries", "grep_search") {
        return Ok(rejection);
    }
    execute_grep_one(tool_use_id, params, context).await
}

/// Maximum context lines before/after a match (hard cap).
const MAX_CONTEXT_LINES: usize = 10;

/// Wall-clock budget for a single grep_search call; partial results are
/// returned with a note when exceeded.
const GREP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Files larger than this are skipped and reported instead of searched.
const MAX_GREP_FILE_SIZE: u64 = 10 * 1024 * 1024;

/// Parse context window params from a JSON object.
/// `context` is the shorthand that sets both before and after (like grep -C).
/// Returns (context_before, context_after).
fn parse_context_params(params: &Value) -> (usize, usize) {
    let shorthand = params["context"]
        .as_u64()
        .map(|v| (v as usize).min(MAX_CONTEXT_LINES))
        .unwrap_or(0);
    let before = params["context_before"]
        .as_u64()
        .map(|v| (v as usize).min(MAX_CONTEXT_LINES))
        .unwrap_or(shorthand);
    let after = params["context_after"]
        .as_u64()
        .map(|v| (v as usize).min(MAX_CONTEXT_LINES))
        .unwrap_or(shorthand);
    (before, after)
}

/// One contiguous stretch of output from a single file: either a lone match
/// line (no-context mode) or a match group with its context lines.
struct GrepGroup {
    match_count: usize,
    rendered: String,
}

/// All match groups found in one file, in file order.
struct FileResult {
    rel_path: String,
    groups: Vec<GrepGroup>,
}

/// Message sent from walker threads to the aggregating thread.
enum GrepMsg {
    File(FileResult),
    Oversized(String),
}

/// grep-searcher Sink that renders matches into the tool's output format:
/// `path:line: text` for plain matches, `path>line: text` + `path:line: text`
/// groups in context mode.
struct CollectSink<'a> {
    rel_path: &'a str,
    use_context: bool,
    groups: Vec<GrepGroup>,
    cur: String,
    cur_matches: usize,
}

impl CollectSink<'_> {
    /// Close the current context group, if any, and push it onto `groups`.
    fn flush_group(&mut self) {
        if !self.cur.is_empty() {
            self.groups.push(GrepGroup {
                match_count: self.cur_matches,
                rendered: std::mem::take(&mut self.cur),
            });
            self.cur_matches = 0;
        }
    }
}

impl grep_searcher::Sink for CollectSink<'_> {
    type Error = std::io::Error;

    fn matched(
        &mut self,
        _searcher: &grep_searcher::Searcher,
        mat: &grep_searcher::SinkMatch<'_>,
    ) -> Result<bool, Self::Error> {
        let line_no = mat.line_number().unwrap_or(0);
        let text = String::from_utf8_lossy(mat.bytes());
        let text = text.trim_end_matches(['\r', '\n']);
        if self.use_context {
            self.cur
                .push_str(&format!("{}>{}: {}\n", self.rel_path, line_no, text));
            self.cur_matches += 1;
        } else {
            self.groups.push(GrepGroup {
                match_count: 1,
                rendered: format!("{}:{}: {}", self.rel_path, line_no, text.trim()),
            });
        }
        Ok(true)
    }

    fn context(
        &mut self,
        _searcher: &grep_searcher::Searcher,
        ctx: &grep_searcher::SinkContext<'_>,
    ) -> Result<bool, Self::Error> {
        let line_no = ctx.line_number().unwrap_or(0);
        let text = String::from_utf8_lossy(ctx.bytes());
        let text = text.trim_end_matches(['\r', '\n']);
        self.cur
            .push_str(&format!("{}:{}: {}\n", self.rel_path, line_no, text));
        Ok(true)
    }

    fn context_break(
        &mut self,
        _searcher: &grep_searcher::Searcher,
    ) -> Result<bool, Self::Error> {
        self.flush_group();
        Ok(true)
    }

    fn finish(
        &mut self,
        _searcher: &grep_searcher::Searcher,
        _finish: &grep_searcher::SinkFinish,
    ) -> Result<(), Self::Error> {
        self.flush_group();
        Ok(())
    }
}

async fn execute_grep_one(
    tool_use_id: &str,
    params: Value,
    context: &ToolContext,
) -> Result<ToolOutput> {
    let query = params["query"].as_str().unwrap_or("");
    if query.is_empty() {
        return Ok(ToolOutput {
            content: "No search query provided".into(),
            is_error: true,
            attachments: Vec::new(),
        });
    }

    let Some(path_param) = params["path"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return Ok(ToolOutput {
            content: "GREP_ERROR: `path` is required. Pass the narrowest subdirectory that \
                      covers what you need (e.g. 'src' or 'crates/rustic-agent'), or '.' to \
                      deliberately search the entire project."
                .into(),
            is_error: true,
            attachments: Vec::new(),
        });
    };

    let search_path = context.project_root.join(path_param);
    if !search_path.exists() {
        return Ok(ToolOutput {
            content: format!(
                "GREP_ERROR: path '{}' does not exist under the project root. \
                 Use `glob` or `list_directory` to find the right directory.",
                path_param
            ),
            is_error: true,
            attachments: Vec::new(),
        });
    }

    let include_glob = params["include"]
        .as_str()
        .and_then(|s| glob::Pattern::new(s).ok());
    let exclude_glob = params["exclude"]
        .as_str()
        .and_then(|s| glob::Pattern::new(s).ok());

    let (ctx_before, ctx_after) = parse_context_params(&params);
    let use_context = ctx_before > 0 || ctx_after > 0;

    let matcher = match grep_regex::RegexMatcherBuilder::new()
        .case_insensitive(true)
        .line_terminator(Some(b'\n'))
        .build(query)
    {
        Ok(m) => m,
        Err(e) => {
            return Ok(ToolOutput {
                content: format!("Invalid regex: {}", e),
                is_error: true,
                attachments: Vec::new(),
            })
        }
    };

    let show_all = context.sensitive_files_allowed();
    context.emit_progress(tool_use_id, &format!("Searching for \"{}\"...", query));

    let run = GrepRun {
        matcher,
        search_path,
        project_root: context.project_root.clone(),
        include_glob,
        exclude_glob,
        ctx_before,
        ctx_after,
        respect_gitignore: !show_all,
        max_file_size: MAX_GREP_FILE_SIZE,
        timeout: GREP_TIMEOUT,
    };
    let content = run_grep(run, &|matches, files| {
        context.emit_progress(
            tool_use_id,
            &format!("{} matches in {} files...", matches, files),
        );
    });

    Ok(ToolOutput {
        content,
        is_error: false,
        attachments: Vec::new(),
    })
}

/// Inputs for one grep run; separated from ToolContext so the engine is testable.
struct GrepRun {
    matcher: grep_regex::RegexMatcher,
    search_path: std::path::PathBuf,
    project_root: std::path::PathBuf,
    include_glob: Option<glob::Pattern>,
    exclude_glob: Option<glob::Pattern>,
    ctx_before: usize,
    ctx_after: usize,
    respect_gitignore: bool,
    max_file_size: u64,
    timeout: std::time::Duration,
}

/// Run a parallel walk + ripgrep-engine search and render the tool output text.
fn run_grep(run: GrepRun, progress: &dyn Fn(usize, u32)) -> String {
    let use_context = run.ctx_before > 0 || run.ctx_after > 0;
    let ctx_before = run.ctx_before;
    let ctx_after = run.ctx_after;
    let timeout = run.timeout;
    let max_file_size = run.max_file_size;
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(12);
    let walker = ignore::WalkBuilder::new(&run.search_path)
        .hidden(false)
        .git_ignore(run.respect_gitignore)
        .filter_entry(skip_dot_dirs)
        .threads(threads)
        .build_parallel();

    let max_results: usize = 100;
    let stop = std::sync::atomic::AtomicBool::new(false);
    let timed_out = std::sync::atomic::AtomicBool::new(false);
    let files_searched = std::sync::atomic::AtomicU32::new(0);
    let started = std::time::Instant::now();
    let (tx, rx) = std::sync::mpsc::channel::<GrepMsg>();

    let mut kept: Vec<FileResult> = Vec::new();
    let mut oversized: Vec<String> = Vec::new();
    let mut match_count = 0usize;
    let mut truncated = false;

    std::thread::scope(|scope| {
        let stop = &stop;
        let timed_out = &timed_out;
        let files_searched = &files_searched;
        let include_glob = &run.include_glob;
        let exclude_glob = &run.exclude_glob;
        let matcher = &run.matcher;
        let project_root = &run.project_root;

        scope.spawn(move || {
            walker.run(|| {
                let tx = tx.clone();
                let mut searcher = grep_searcher::SearcherBuilder::new()
                    .binary_detection(grep_searcher::BinaryDetection::quit(0))
                    .line_number(true)
                    .before_context(ctx_before)
                    .after_context(ctx_after)
                    .build();
                Box::new(move |entry| {
                    if stop.load(std::sync::atomic::Ordering::Relaxed) {
                        return ignore::WalkState::Quit;
                    }
                    if started.elapsed() >= timeout {
                        timed_out.store(true, std::sync::atomic::Ordering::Relaxed);
                        return ignore::WalkState::Quit;
                    }
                    let Ok(entry) = entry else {
                        return ignore::WalkState::Continue;
                    };
                    if !entry.file_type().is_some_and(|t| t.is_file()) {
                        return ignore::WalkState::Continue;
                    }
                    let path = entry.path();

                    if let Some(include) = include_glob {
                        if !include.matches_path(path) {
                            return ignore::WalkState::Continue;
                        }
                    }
                    if let Some(exclude) = exclude_glob {
                        if exclude.matches_path(path) {
                            return ignore::WalkState::Continue;
                        }
                    }

                    let rel_path = path
                        .strip_prefix(project_root)
                        .unwrap_or(path)
                        .to_string_lossy()
                        .into_owned();

                    if let Ok(md) = entry.metadata() {
                        if md.len() > max_file_size {
                            let _ = tx.send(GrepMsg::Oversized(rel_path));
                            return ignore::WalkState::Continue;
                        }
                    }

                    let groups = {
                        let mut sink = CollectSink {
                            rel_path: &rel_path,
                            use_context,
                            groups: Vec::new(),
                            cur: String::new(),
                            cur_matches: 0,
                        };
                        if searcher.search_path(matcher, path, &mut sink).is_err() {
                            return ignore::WalkState::Continue;
                        }
                        sink.groups
                    };
                    files_searched.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if !groups.is_empty() {
                        let _ = tx.send(GrepMsg::File(FileResult { rel_path, groups }));
                    }
                    ignore::WalkState::Continue
                })
            });
        });

        // Aggregate on this thread; the loop ends when all walker threads
        // (and thus all tx clones) are done.
        for msg in rx {
            match msg {
                GrepMsg::Oversized(p) => oversized.push(p),
                GrepMsg::File(fr) => {
                    if truncated {
                        continue;
                    }
                    let mut kept_groups: Vec<GrepGroup> = Vec::new();
                    for g in fr.groups {
                        if match_count + g.match_count > max_results {
                            truncated = true;
                            stop.store(true, std::sync::atomic::Ordering::Relaxed);
                            break;
                        }
                        match_count += g.match_count;
                        if match_count % 20 == 0 {
                            progress(
                                match_count,
                                files_searched.load(std::sync::atomic::Ordering::Relaxed),
                            );
                        }
                        kept_groups.push(g);
                    }
                    if !kept_groups.is_empty() {
                        kept.push(FileResult {
                            rel_path: fr.rel_path,
                            groups: kept_groups,
                        });
                    }
                }
            }
        }
    });

    kept.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    oversized.sort();

    let mut content = if kept.is_empty() {
        String::from("No matches found")
    } else if !use_context {
        let mut lines: Vec<String> = kept
            .into_iter()
            .flat_map(|f| f.groups)
            .map(|g| g.rendered)
            .collect();
        if truncated {
            lines.push(format!("... (truncated at {} results)", max_results));
        }
        lines.join("\n")
    } else {
        let blocks: Vec<String> = kept
            .into_iter()
            .flat_map(|f| f.groups)
            .map(|g| g.rendered)
            .collect();
        let mut out = blocks.join("--\n");
        if truncated {
            out.push_str(&format!("... (truncated at {} results)\n", max_results));
        }
        out.trim_end_matches('\n').to_string()
    };

    if timed_out.load(std::sync::atomic::Ordering::Relaxed) {
        content.push_str(&format!(
            "\n\nNote: search timed out after {}s — partial results shown. \
             Narrow the `path` or make the pattern more specific.",
            timeout.as_secs()
        ));
    }
    if !oversized.is_empty() {
        let shown: Vec<&str> = oversized.iter().take(10).map(String::as_str).collect();
        let more = oversized.len().saturating_sub(shown.len());
        content.push_str(&format!(
            "\n\nNote: skipped {} file(s) larger than {} MB (not searched): {}{}",
            oversized.len(),
            max_file_size / (1024 * 1024),
            shown.join(", "),
            if more > 0 {
                format!(" ... and {} more", more)
            } else {
                String::new()
            }
        ));
    }

    content
}

// ── glob ─────────────────────────────────────────────────────────────────────

async fn execute_glob_dispatch(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    if let Some(rejection) = reject_removed_batch(&params, "patterns", "glob") {
        return Ok(rejection);
    }
    execute_glob_one(params, context).await
}

/// Find files by glob pattern, newest-modified first.
async fn execute_glob_one(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    let pattern = params["pattern"].as_str().unwrap_or("").trim();
    if pattern.is_empty() {
        return Ok(ToolOutput {
            content: "GLOB_ERROR: `pattern` is required (e.g. 'src/**/*.rs').".into(),
            is_error: true,
            attachments: Vec::new(),
        });
    }

    let search_root = params["path"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(|p| context.project_root.join(p))
        .unwrap_or_else(|| context.project_root.clone());

    let compiled = match glob::Pattern::new(pattern) {
        Ok(p) => p,
        Err(e) => {
            return Ok(ToolOutput {
                content: format!("GLOB_ERROR: invalid pattern '{}': {}", pattern, e),
                is_error: true,
                attachments: Vec::new(),
            })
        }
    };

    let show_all = context.sensitive_files_allowed();
    let walker = ignore::WalkBuilder::new(&search_root)
        .hidden(false)
        .git_ignore(!show_all)
        .filter_entry(skip_dot_dirs)
        .build();

    let mut hits: Vec<(std::path::PathBuf, std::time::SystemTime)> = Vec::new();
    const MAX_MATCHES: usize = 200;

    for entry in walker.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let rel = match path.strip_prefix(&context.project_root) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        if !compiled.matches(&rel_str) {
            continue;
        }
        let mtime = std::fs::metadata(path)
            .and_then(|m| m.modified())
            .unwrap_or(std::time::UNIX_EPOCH);
        hits.push((rel.to_path_buf(), mtime));
    }

    hits.sort_by(|a, b| b.1.cmp(&a.1));

    if hits.is_empty() {
        return Ok(ToolOutput {
            content: format!("No files match pattern '{}'.", pattern),
            is_error: false,
            attachments: Vec::new(),
        });
    }

    let truncated = hits.len() > MAX_MATCHES;
    let take = hits.len().min(MAX_MATCHES);
    let mut out: Vec<String> = hits
        .into_iter()
        .take(take)
        .map(|(p, _)| p.to_string_lossy().replace('\\', "/"))
        .collect();
    if truncated {
        out.push(format!(
            "... (truncated at {} results — narrow the pattern or pass `path` to shrink the search scope)",
            MAX_MATCHES
        ));
    }

    Ok(ToolOutput {
        content: out.join("\n"),
        is_error: false,
        attachments: Vec::new(),
    })
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── parse_context_params — pure function, needs no ToolContext ────────────

    #[test]
    fn test_parse_context_none() {
        let (b, a) = parse_context_params(&json!({}));
        assert_eq!((b, a), (0, 0));
    }

    #[test]
    fn test_parse_context_shorthand() {
        let (b, a) = parse_context_params(&json!({ "context": 3 }));
        assert_eq!((b, a), (3, 3));
    }

    #[test]
    fn test_parse_context_explicit_overrides_shorthand() {
        // Explicit context_before/context_after take priority over `context`.
        let (b, a) =
            parse_context_params(&json!({ "context": 3, "context_before": 1, "context_after": 5 }));
        assert_eq!((b, a), (1, 5));
    }

    #[test]
    fn test_parse_context_capped_at_10() {
        let (b, a) = parse_context_params(&json!({ "context": 99 }));
        assert_eq!((b, a), (MAX_CONTEXT_LINES, MAX_CONTEXT_LINES));
    }

    #[test]
    fn test_parse_context_individual_cap() {
        let (b, a) = parse_context_params(&json!({ "context_before": 50, "context_after": 0 }));
        assert_eq!((b, a), (MAX_CONTEXT_LINES, 0));
    }

    #[test]
    fn test_parse_context_zero_is_no_context() {
        let (b, a) = parse_context_params(&json!({ "context": 0 }));
        assert_eq!((b, a), (0, 0));
        // use_context should be false
        assert!(b == 0 && a == 0);
    }

    // ── run_grep — end-to-end engine tests against a temp directory ───────────

    fn mk_run(root: &std::path::Path, query: &str, ctx: (usize, usize)) -> GrepRun {
        GrepRun {
            matcher: grep_regex::RegexMatcherBuilder::new()
                .case_insensitive(true)
                .line_terminator(Some(b'\n'))
                .build(query)
                .unwrap(),
            search_path: root.to_path_buf(),
            project_root: root.to_path_buf(),
            include_glob: None,
            exclude_glob: None,
            ctx_before: ctx.0,
            ctx_after: ctx.1,
            respect_gitignore: true,
            max_file_size: MAX_GREP_FILE_SIZE,
            timeout: GREP_TIMEOUT,
        }
    }

    #[test]
    fn test_run_grep_basic_match_format() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "hello\nworld needle\n").unwrap();
        let out = run_grep(mk_run(dir.path(), "needle", (0, 0)), &|_, _| {});
        assert_eq!(out, "a.txt:2: world needle");
    }

    #[test]
    fn test_run_grep_no_matches() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
        let out = run_grep(mk_run(dir.path(), "zzz_absent", (0, 0)), &|_, _| {});
        assert_eq!(out, "No matches found");
    }

    #[test]
    fn test_run_grep_results_sorted_by_path() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("b.txt"), "needle\n").unwrap();
        std::fs::write(dir.path().join("a.txt"), "needle\n").unwrap();
        let out = run_grep(mk_run(dir.path(), "needle", (0, 0)), &|_, _| {});
        let lines: Vec<&str> = out.lines().collect();
        assert!(lines[0].starts_with("a.txt:1:"), "{}", out);
        assert!(lines[1].starts_with("b.txt:1:"), "{}", out);
    }

    #[test]
    fn test_run_grep_context_mode_markers() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("f.txt"), "before\nMATCH\nafter\n").unwrap();
        let out = run_grep(mk_run(dir.path(), "MATCH", (1, 1)), &|_, _| {});
        assert!(out.contains("f.txt>2: MATCH"), "{}", out);
        assert!(out.contains("f.txt:1: before"), "{}", out);
        assert!(out.contains("f.txt:3: after"), "{}", out);
    }

    #[test]
    fn test_run_grep_context_groups_separated() {
        let dir = tempfile::tempdir().unwrap();
        let body = "MATCH\nx\nx\nx\nx\nx\nMATCH\n";
        std::fs::write(dir.path().join("g.txt"), body).unwrap();
        let out = run_grep(mk_run(dir.path(), "MATCH", (1, 1)), &|_, _| {});
        assert!(out.contains("--"), "groups must be separated: {}", out);
        assert!(out.contains("g.txt>1: MATCH"), "{}", out);
        assert!(out.contains("g.txt>7: MATCH"), "{}", out);
    }

    #[test]
    fn test_run_grep_oversized_skipped_and_reported() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("big.txt"), "needle needle needle\n").unwrap();
        let mut run = mk_run(dir.path(), "needle", (0, 0));
        run.max_file_size = 5;
        let out = run_grep(run, &|_, _| {});
        assert!(out.starts_with("No matches found"), "{}", out);
        assert!(out.contains("skipped 1 file(s)"), "{}", out);
        assert!(out.contains("big.txt"), "{}", out);
    }

    #[test]
    fn test_run_grep_binary_file_skipped() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("bin.dat"), b"\x00\x01\x02needle\n").unwrap();
        let out = run_grep(mk_run(dir.path(), "needle", (0, 0)), &|_, _| {});
        assert_eq!(out, "No matches found");
    }

    #[test]
    fn test_run_grep_truncates_at_100_matches() {
        let dir = tempfile::tempdir().unwrap();
        let body = "needle\n".repeat(150);
        std::fs::write(dir.path().join("many.txt"), body).unwrap();
        let out = run_grep(mk_run(dir.path(), "needle", (0, 0)), &|_, _| {});
        let match_lines = out.lines().filter(|l| l.contains("needle")).count();
        assert_eq!(match_lines, 100, "{}", out);
        assert!(out.contains("truncated at 100 results"), "{}", out);
    }

    #[test]
    fn test_run_grep_include_glob_filters() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "needle\n").unwrap();
        std::fs::write(dir.path().join("a.txt"), "needle\n").unwrap();
        let mut run = mk_run(dir.path(), "needle", (0, 0));
        run.include_glob = Some(glob::Pattern::new("*.rs").unwrap());
        let out = run_grep(run, &|_, _| {});
        assert!(out.contains("a.rs:1:"), "{}", out);
        assert!(!out.contains("a.txt"), "{}", out);
    }

    #[test]
    fn test_run_grep_case_insensitive() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("c.txt"), "NeEdLe here\n").unwrap();
        let out = run_grep(mk_run(dir.path(), "needle", (0, 0)), &|_, _| {});
        assert!(out.contains("c.txt:1: NeEdLe here"), "{}", out);
    }

    // ── MAX_CONTEXT_LINES constant ────────────────────────────────────────────

    #[test]
    fn test_max_context_lines_is_10() {
        assert_eq!(MAX_CONTEXT_LINES, 10);
    }

    // ── directory skip filter ─────────────────────────────────────────────────

    #[test]
    fn test_noise_dirs_dotfolders_skipped() {
        for d in [".git", ".github", ".venv", ".idea", ".vscode", ".next"] {
            assert!(is_noise_dir_name(d), "{} should be skipped", d);
        }
    }

    #[test]
    fn test_noise_dirs_heavy_dirs_skipped() {
        for d in ["node_modules", "target", "dist", "build", "__pycache__", "coverage"] {
            assert!(is_noise_dir_name(d), "{} should be skipped", d);
        }
    }

    #[test]
    fn test_noise_dirs_rustic_kept() {
        assert!(!is_noise_dir_name(".rustic"), ".rustic must stay in scope");
    }

    #[test]
    fn test_noise_dirs_normal_dirs_kept() {
        for d in ["src", "crates", "tests", "lib"] {
            assert!(!is_noise_dir_name(d), "{} should not be skipped", d);
        }
    }
}

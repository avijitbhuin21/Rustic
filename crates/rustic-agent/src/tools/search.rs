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
                    "include": { "type": "string", "description": "Glob pattern for files to include (e.g. '*.rs', '*.{js,jsx}')" },
                    "exclude": { "type": "string", "description": "Glob pattern for files to exclude ({a,b} alternatives supported)" },
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
    /// A key or secrets file that was left unsearched.
    Sensitive(String),
    /// A file or directory that could not be opened, with the OS error.
    Unreadable(String),
}

/// Whether grep must leave `path` unsearched: key and credential files always,
/// secrets files (`.env` and the like) unless the user allowed them.
fn is_withheld_from_grep(
    path: &std::path::Path,
    project_rel: &str,
    skip_secret_files: bool,
    allowed_paths: &[String],
) -> bool {
    if crate::tools::file_ops::tier1_block_reason(project_rel, path).is_some() {
        return true;
    }
    if !skip_secret_files || project_rel.starts_with(".rustic/") {
        return false;
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    crate::tools::file_ops::is_tier2_secret_name(&name)
        && !allowed_paths.iter().any(|p| p.trim() == project_rel)
}

/// Append a "Note: <count> <what>: a, b ... and N more" paragraph listing up to ten `items`.
fn push_list_note(content: &mut String, items: &[String], what: &str) {
    if items.is_empty() {
        return;
    }
    let shown: Vec<&str> = items.iter().take(10).map(String::as_str).collect();
    let more = items.len() - shown.len();
    content.push_str(&format!(
        "\n\nNote: {} {}: {}{}",
        items.len(),
        what,
        shown.join(", "),
        if more > 0 {
            format!(" ... and {} more", more)
        } else {
            String::new()
        }
    ));
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

    fn context_break(&mut self, _searcher: &grep_searcher::Searcher) -> Result<bool, Self::Error> {
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

    // Same rule as the other file tools: inside the project, unless the user
    // has granted the task access to all files.
    let search_path = match crate::tools::file_ops::resolve_with_scope(context, path_param) {
        Ok(p) => p,
        Err(out) => return Ok(out),
    };
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

    let mut filters: [Option<BraceGlob>; 2] = [None, None];
    for (slot, key) in filters.iter_mut().zip(["include", "exclude"]) {
        let Some(pattern) = params[key].as_str().filter(|s| !s.is_empty()) else {
            continue;
        };
        match BraceGlob::new(pattern) {
            Ok(g) => *slot = Some(g),
            Err(e) => {
                return Ok(ToolOutput {
                    content: format!("GREP_ERROR: invalid `{key}` pattern '{pattern}': {e}"),
                    is_error: true,
                    attachments: Vec::new(),
                })
            }
        }
    }
    let [include_glob, exclude_glob] = filters;

    let (ctx_before, ctx_after) = parse_context_params(&params);

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
        skip_secret_files: !show_all,
        allowed_paths: context.allowed_paths.clone(),
        max_file_size: MAX_GREP_FILE_SIZE,
        timeout: GREP_TIMEOUT,
    };
    // The search is disk- and CPU-bound for up to a minute. On an async worker
    // it stalled every other tool call and event of the run for that long, so
    // it runs on a blocking thread and reports progress back over a channel.
    let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel::<(usize, u32)>();
    let mut search = tokio::task::spawn_blocking(move || {
        run_grep(run, &|matches, files| {
            let _ = progress_tx.send((matches, files));
        })
    });
    let content = loop {
        tokio::select! {
            Some((matches, files)) = progress_rx.recv() => context.emit_progress(
                tool_use_id,
                &format!("{} matches in {} files...", matches, files),
            ),
            done = &mut search => {
                break done.map_err(|e| anyhow::anyhow!("grep_search worker failed: {e}"))?;
            }
        }
    };

    Ok(ToolOutput {
        is_error: content.starts_with("GREP_ERROR"),
        content,
        attachments: Vec::new(),
    })
}

/// Upper bound on the patterns one `{a,b}` expression may expand to.
const MAX_BRACE_EXPANSIONS: usize = 128;

/// Expand shell-style `{a,b}` alternatives (nested and repeated groups included) into plain glob patterns.
fn expand_braces(pattern: &str) -> Vec<String> {
    let bytes = pattern.as_bytes();
    for open in 0..bytes.len() {
        if bytes[open] != b'{' {
            continue;
        }
        let mut depth = 0usize;
        let mut commas: Vec<usize> = Vec::new();
        let mut close = None;
        for (j, &b) in bytes.iter().enumerate().skip(open) {
            match b {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(j);
                        break;
                    }
                }
                b',' if depth == 1 => commas.push(j),
                _ => {}
            }
        }
        // A group needs a closing brace and at least one comma; `{a}` stays literal.
        let Some(close) = close else { continue };
        if commas.is_empty() {
            continue;
        }
        let (prefix, suffix) = (&pattern[..open], &pattern[close + 1..]);
        let mut out = Vec::new();
        let mut start = open + 1;
        for end in commas.into_iter().chain(std::iter::once(close)) {
            out.extend(expand_braces(&format!(
                "{prefix}{}{suffix}",
                &pattern[start..end]
            )));
            if out.len() > MAX_BRACE_EXPANSIONS {
                return out;
            }
            start = end + 1;
        }
        return out;
    }
    vec![pattern.to_string()]
}

/// A glob that also understands `{a,b}` alternatives, which the `glob` crate treats as literal text.
/// Each alternative remembers whether it names a directory (contains `/`).
struct BraceGlob(Vec<(glob::Pattern, bool)>);

impl BraceGlob {
    /// Compile `pattern`, expanding brace alternatives first.
    fn new(pattern: &str) -> std::result::Result<Self, String> {
        let expanded = expand_braces(pattern);
        if expanded.len() > MAX_BRACE_EXPANSIONS {
            return Err(format!(
                "too many `{{a,b}}` alternatives (more than {MAX_BRACE_EXPANSIONS} patterns)"
            ));
        }
        expanded
            .iter()
            .map(|p| {
                glob::Pattern::new(p)
                    .map(|compiled| (compiled, p.contains('/')))
                    .map_err(|e| e.to_string())
            })
            .collect::<std::result::Result<Vec<_>, _>>()
            .map(Self)
    }

    /// True when any alternative matches `text`.
    fn matches(&self, text: &str) -> bool {
        self.0.iter().any(|(p, _)| p.matches(text))
    }

    /// True when any alternative matches a searched file, given its
    /// forward-slash path relative to the project root and to the search
    /// directory. An alternative without `/` also matches the bare file name,
    /// so `foo.rs` and `{a.rs,b.rs}` find those files at any depth. The
    /// project-relative path is also tried with a leading `/`, so `*/dir/*`
    /// covers a top-level `dir` too.
    fn matches_file(&self, project_rel: &str, search_rel: &str) -> bool {
        let name = project_rel.rsplit('/').next().unwrap_or(project_rel);
        let rooted = format!("/{project_rel}");
        self.0.iter().any(|(p, has_dir)| {
            p.matches(project_rel)
                || p.matches(&rooted)
                || (!search_rel.is_empty() && p.matches(search_rel))
                || (!has_dir && p.matches(name))
        })
    }
}

/// Inputs for one grep run; separated from ToolContext so the engine is testable.
struct GrepRun {
    matcher: grep_regex::RegexMatcher,
    search_path: std::path::PathBuf,
    project_root: std::path::PathBuf,
    include_glob: Option<BraceGlob>,
    exclude_glob: Option<BraceGlob>,
    ctx_before: usize,
    ctx_after: usize,
    respect_gitignore: bool,
    /// Leave `.env`-style secrets files unsearched (key files always are).
    skip_secret_files: bool,
    /// Project-relative paths the user pre-approved in `.rustic/allowed-files.txt`.
    allowed_paths: Vec<String>,
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
    let mut withheld: Vec<String> = Vec::new();
    let mut unreadable: Vec<String> = Vec::new();
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
        let search_path = &run.search_path;
        let skip_secret_files = run.skip_secret_files;
        let allowed_paths = &run.allowed_paths;

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
                    let entry = match entry {
                        Ok(entry) => entry,
                        Err(e) => {
                            // A directory that could not be listed hides every
                            // file under it; say so instead of "No matches".
                            let _ = tx.send(GrepMsg::Unreadable(e.to_string()));
                            return ignore::WalkState::Continue;
                        }
                    };
                    if !entry.file_type().is_some_and(|t| t.is_file()) {
                        return ignore::WalkState::Continue;
                    }
                    let path = entry.path();

                    let rel_path = path
                        .strip_prefix(project_root)
                        .unwrap_or(path)
                        .to_string_lossy()
                        .into_owned();

                    // Filters see relative paths, never the absolute one: a
                    // pattern must not depend on (or be defeated by) the
                    // directories above the project.
                    if include_glob.is_some() || exclude_glob.is_some() {
                        let project_rel = rel_path.replace('\\', "/");
                        let search_rel = path
                            .strip_prefix(search_path)
                            .map(|p| p.to_string_lossy().replace('\\', "/"))
                            .unwrap_or_default();
                        if let Some(include) = include_glob {
                            if !include.matches_file(&project_rel, &search_rel) {
                                return ignore::WalkState::Continue;
                            }
                        }
                        if let Some(exclude) = exclude_glob {
                            if exclude.matches_file(&project_rel, &search_rel) {
                                return ignore::WalkState::Continue;
                            }
                        }
                    }

                    if is_withheld_from_grep(
                        path,
                        &rel_path.replace('\\', "/"),
                        skip_secret_files,
                        allowed_paths,
                    ) {
                        let _ = tx.send(GrepMsg::Sensitive(rel_path));
                        return ignore::WalkState::Continue;
                    }

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
                        if let Err(e) = searcher.search_path(matcher, path, &mut sink) {
                            let _ = tx.send(GrepMsg::Unreadable(format!("{rel_path} ({e})")));
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
                GrepMsg::Sensitive(p) => withheld.push(p),
                GrepMsg::Unreadable(p) => unreadable.push(p),
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
                        if match_count.is_multiple_of(20) {
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
    withheld.sort();
    unreadable.sort();

    // Nothing could be opened at all: that is a failed search, not an empty one.
    if kept.is_empty()
        && !unreadable.is_empty()
        && files_searched.load(std::sync::atomic::Ordering::Relaxed) == 0
    {
        let mut content = String::from(
            "GREP_ERROR: the search could not read any file, so this is NOT a 'no matches' result.",
        );
        push_list_note(&mut content, &unreadable, "path(s) could not be read");
        return content;
    }

    let mut content = if kept.is_empty() && !unreadable.is_empty() {
        String::from("No matches found in the files that could be read (results are incomplete)")
    } else if kept.is_empty() {
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

    push_list_note(
        &mut content,
        &unreadable,
        "path(s) could not be read and were not searched, so results are incomplete",
    );
    push_list_note(
        &mut content,
        &withheld,
        "key or secrets file(s) were not searched (read_file asks the user for access)",
    );

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

    let search_root = match params["path"].as_str().filter(|s| !s.is_empty()) {
        Some(p) => match crate::tools::file_ops::resolve_with_scope(context, p) {
            Ok(root) => root,
            Err(out) => return Ok(out),
        },
        None => context.project_root.clone(),
    };

    let compiled = match BraceGlob::new(pattern) {
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
        // Outside the project (only reachable with access to all files
        // granted): match against the path under the search root and list
        // the absolute path.
        let (rel, shown) = match path.strip_prefix(&context.project_root) {
            Ok(r) => (r, r.to_path_buf()),
            Err(_) => match path.strip_prefix(&search_root) {
                Ok(r) => (r, path.to_path_buf()),
                Err(_) => continue,
            },
        };
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        if !compiled.matches(&rel_str) {
            continue;
        }
        let mtime = std::fs::metadata(path)
            .and_then(|m| m.modified())
            .unwrap_or(std::time::UNIX_EPOCH);
        hits.push((shown, mtime));
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
            skip_secret_files: true,
            allowed_paths: Vec::new(),
            max_file_size: MAX_GREP_FILE_SIZE,
            timeout: GREP_TIMEOUT,
        }
    }

    #[test]
    fn key_and_secrets_files_are_not_searched() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "needle public\n").unwrap();
        std::fs::write(dir.path().join(".env"), "TOKEN=needle\n").unwrap();
        std::fs::write(dir.path().join("server.pem"), "needle key\n").unwrap();
        std::fs::write(dir.path().join("id_rsa"), "needle key\n").unwrap();
        std::fs::create_dir_all(dir.path().join("conf")).unwrap();
        std::fs::write(dir.path().join("conf/prod.env"), "needle\n").unwrap();

        let out = run_grep(mk_run(dir.path(), "needle", (0, 0)), &|_, _| {}).replace('\\', "/");
        assert!(out.starts_with("a.txt:1: needle public"), "{out}");
        assert!(!out.contains("TOKEN="), "{out}");
        assert!(!out.contains("needle key"), "{out}");
        assert!(
            out.contains("4 key or secrets file(s) were not searched"),
            "{out}"
        );
        assert!(
            out.contains(".env") && out.contains("conf/prod.env"),
            "{out}"
        );

        // Granting access to all files opens secrets files, never key files.
        let mut run = mk_run(dir.path(), "needle", (0, 0));
        run.skip_secret_files = false;
        let out = run_grep(run, &|_, _| {}).replace('\\', "/");
        assert!(out.contains(".env:1: TOKEN=needle"), "{out}");
        assert!(!out.contains("needle key"), "{out}");
        assert!(
            out.contains("2 key or secrets file(s) were not searched"),
            "{out}"
        );

        // A path pre-approved by the user is searched.
        let mut run = mk_run(dir.path(), "needle", (0, 0));
        run.allowed_paths = vec!["conf/prod.env".into()];
        let out = run_grep(run, &|_, _| {}).replace('\\', "/");
        assert!(out.contains("conf/prod.env:1: needle"), "{out}");
        assert!(!out.contains("TOKEN="), "{out}");
    }

    #[tokio::test]
    async fn grep_and_glob_stay_inside_the_project() {
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("notes.txt"), "needle outside\n").unwrap();
        let project = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(project.path().join("src")).unwrap();
        std::fs::write(project.path().join("src/a.txt"), "needle inside\n").unwrap();
        let (ctx, rx) = ToolContext::new_test(project.path().to_path_buf());
        std::mem::forget(rx);
        // The test context starts with "access to all files" granted.
        ctx.shared_permissions.set_sensitive_files_allowed(false);
        let abs = outside.path().to_string_lossy().into_owned();

        for path in [abs.as_str(), "..", "src/../.."] {
            let out = execute(
                "grep_search",
                "t",
                json!({ "query": "needle", "path": path }),
                &ctx,
            )
            .await
            .unwrap();
            assert!(out.is_error, "grep {path}: {}", out.content);
            assert!(
                out.content.starts_with("PATH_SCOPE_VIOLATION"),
                "grep {path}: {}",
                out.content
            );
            let out = execute(
                "glob",
                "t",
                json!({ "pattern": "**/*.txt", "path": path }),
                &ctx,
            )
            .await
            .unwrap();
            assert!(out.is_error, "glob {path}: {}", out.content);
            assert!(
                out.content.starts_with("PATH_SCOPE_VIOLATION"),
                "glob {path}: {}",
                out.content
            );
        }

        let out = execute(
            "grep_search",
            "t",
            json!({ "query": "needle", "path": "src" }),
            &ctx,
        )
        .await
        .unwrap();
        assert!(!out.is_error);
        assert_eq!(out.content.replace('\\', "/"), "src/a.txt:1: needle inside");
        let out = execute(
            "glob",
            "t",
            json!({ "pattern": "**/*.txt", "path": "src" }),
            &ctx,
        )
        .await
        .unwrap();
        assert_eq!(out.content.replace('\\', "/"), "src/a.txt");

        // With access to all files granted, the same outside path is searchable.
        ctx.shared_permissions.set_sensitive_files_allowed(true);
        let out = execute(
            "grep_search",
            "t",
            json!({ "query": "needle", "path": abs }),
            &ctx,
        )
        .await
        .unwrap();
        assert!(
            out.content.ends_with("notes.txt:1: needle outside"),
            "{}",
            out.content
        );
        let out = execute(
            "glob",
            "t",
            json!({ "pattern": "*.txt", "path": abs }),
            &ctx,
        )
        .await
        .unwrap();
        assert!(out.content.ends_with("notes.txt"), "{}", out.content);
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_files_are_reported_not_hidden() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let locked = dir.path().join("locked.txt");
        std::fs::write(&locked, "needle\n").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::read(&locked).is_ok() {
            return; // running as root: permissions do not apply
        }

        // Nothing readable at all is an error, not an empty result.
        let out = run_grep(mk_run(dir.path(), "needle", (0, 0)), &|_, _| {});
        assert!(out.starts_with("GREP_ERROR"), "{out}");
        assert!(out.contains("locked.txt"), "{out}");

        // Some files readable: results are flagged as incomplete.
        std::fs::write(dir.path().join("open.txt"), "nothing here\n").unwrap();
        let out = run_grep(mk_run(dir.path(), "needle", (0, 0)), &|_, _| {});
        assert!(
            out.starts_with("No matches found in the files that could be read"),
            "{out}"
        );
        assert!(out.contains("1 path(s) could not be read"), "{out}");

        std::fs::write(dir.path().join("hit.txt"), "needle\n").unwrap();
        let out = run_grep(mk_run(dir.path(), "needle", (0, 0)), &|_, _| {});
        assert!(out.starts_with("hit.txt:1: needle"), "{out}");
        assert!(out.contains("results are incomplete"), "{out}");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o644)).unwrap();
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
        run.include_glob = Some(BraceGlob::new("*.rs").unwrap());
        let out = run_grep(run, &|_, _| {});
        assert!(out.contains("a.rs:1:"), "{}", out);
        assert!(!out.contains("a.txt"), "{}", out);
    }

    #[test]
    fn test_run_grep_brace_include_and_exclude() {
        // Tracker #7: `*.{js,jsx}` used to match nothing and report "No matches found".
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("sub")).unwrap();
        for name in ["a.js", "sub/b.jsx", "c.ts", "d.rs"] {
            std::fs::write(dir.path().join(name), "needle\n").unwrap();
        }
        let mut run = mk_run(dir.path(), "needle", (0, 0));
        run.include_glob = Some(BraceGlob::new("*.{js,jsx}").unwrap());
        let out = run_grep(run, &|_, _| {});
        // Windows prints the sub-directory match as `sub\b.jsx`.
        let listed = out.replace('\\', "/");
        assert!(
            listed.contains("a.js:1:") && listed.contains("sub/b.jsx:1:"),
            "{}",
            out
        );
        assert!(!out.contains("c.ts") && !out.contains("d.rs"), "{}", out);

        let mut run = mk_run(dir.path(), "needle", (0, 0));
        run.exclude_glob = Some(BraceGlob::new("*.{js,jsx,ts}").unwrap());
        let out = run_grep(run, &|_, _| {});
        assert_eq!(out, "d.rs:1: needle");
    }

    /// Tracker issue #15: filters were matched against the absolute path, so
    /// only patterns starting with a wildcard could match anything.
    #[test]
    fn test_run_grep_include_file_names_and_relative_paths() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/ui")).unwrap();
        for name in [
            "src/ui/prompt-box.jsx",
            "src/ui/tool-call-card.jsx",
            "src/ui/other.jsx",
            "src/main.rs",
        ] {
            std::fs::write(dir.path().join(name), "needle\n").unwrap();
        }
        let files = |include: &str, search: &str| -> Vec<String> {
            let mut run = mk_run(dir.path(), "needle", (0, 0));
            run.search_path = dir.path().join(search);
            run.include_glob = Some(BraceGlob::new(include).unwrap());
            let out = run_grep(run, &|_, _| {}).replace('\\', "/");
            let mut found: Vec<String> = out
                .lines()
                .filter_map(|l| l.split(':').next())
                .filter(|p| p.contains('.'))
                .map(str::to_string)
                .collect();
            found.sort();
            found
        };
        let both = vec![
            "src/ui/prompt-box.jsx".to_string(),
            "src/ui/tool-call-card.jsx".to_string(),
        ];

        // A brace list of whole file names, from a sub-directory and from the root.
        assert_eq!(files("{prompt-box.jsx,tool-call-card.jsx}", "src/ui"), both);
        assert_eq!(files("{prompt-box.jsx,tool-call-card.jsx}", ""), both);
        // A single plain file name.
        assert_eq!(files("prompt-box.jsx", ""), both[..1].to_vec());
        // A path relative to the project root, and one relative to the search directory.
        assert_eq!(files("src/ui/prompt-*.jsx", ""), both[..1].to_vec());
        assert_eq!(files("ui/tool-*.jsx", "src"), both[1..].to_vec());
        // A name that is only part of a file name must not match.
        assert!(files("box.jsx", "").is_empty());
    }

    #[test]
    fn test_run_grep_exclude_directories() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["vendor/a.rs", "references/deep/b.rs", "src/c.rs", "d.rs"] {
            let p = dir.path().join(name);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "needle\n").unwrap();
        }
        let kept = |exclude: &str| -> String {
            let mut run = mk_run(dir.path(), "needle", (0, 0));
            run.exclude_glob = Some(BraceGlob::new(exclude).unwrap());
            run_grep(run, &|_, _| {}).replace('\\', "/")
        };

        let out = kept("{vendor,references}/**");
        assert!(
            out.contains("src/c.rs:1:") && out.contains("d.rs:1:"),
            "{out}"
        );
        assert!(
            !out.contains("vendor") && !out.contains("references"),
            "{out}"
        );

        // `*/dir/*` also covers a top-level directory.
        let out = kept("*/vendor/*");
        assert!(!out.contains("vendor/a.rs"), "{out}");
        assert!(out.contains("references/deep/b.rs:1:"), "{out}");

        // A bare file name is excluded at any depth.
        let out = kept("c.rs");
        assert!(!out.contains("src/c.rs"), "{out}");
        assert!(out.contains("d.rs:1:"), "{out}");
    }

    /// The directories above the project must not take part in matching.
    #[test]
    fn test_run_grep_filters_ignore_directories_above_the_project() {
        let outer = tempfile::tempdir().unwrap();
        let root = outer.path().join("zzparentzz");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.rs"), "needle\n").unwrap();

        let mut run = mk_run(&root, "needle", (0, 0));
        run.exclude_glob = Some(BraceGlob::new("*zzparentzz*").unwrap());
        assert_eq!(run_grep(run, &|_, _| {}), "a.rs:1: needle");

        let mut run = mk_run(&root, "needle", (0, 0));
        run.include_glob = Some(BraceGlob::new("*zzparentzz*").unwrap());
        assert_eq!(run_grep(run, &|_, _| {}), "No matches found");
    }

    #[test]
    fn test_expand_braces() {
        assert_eq!(expand_braces("*.rs"), vec!["*.rs"]);
        assert_eq!(expand_braces("*.{js,jsx}"), vec!["*.js", "*.jsx"]);
        assert_eq!(
            expand_braces("{src,tests}/**/*.{js,ts}"),
            vec![
                "src/**/*.js",
                "src/**/*.ts",
                "tests/**/*.js",
                "tests/**/*.ts"
            ]
        );
        assert_eq!(expand_braces("a{b,c{d,e}}"), vec!["ab", "acd", "ace"]);
        assert_eq!(expand_braces("*.{js,}"), vec!["*.js", "*."]);
        // No comma or no closing brace: not an alternative group.
        assert_eq!(expand_braces("{a}.txt"), vec!["{a}.txt"]);
        assert_eq!(expand_braces("a{b,c"), vec!["a{b,c"]);
    }

    #[test]
    fn test_brace_glob_matches_and_rejects() {
        let g = BraceGlob::new("tests/**/*.{js,ts}").unwrap();
        assert!(g.matches("tests/unit/a.ts") && g.matches("tests/a.js"));
        assert!(!g.matches("tests/a.rs") && !g.matches("src/a.js"));
        assert!(BraceGlob::new("src/[a").is_err());
        let huge = "{a,b}".repeat(8);
        assert!(BraceGlob::new(&huge).is_err());
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
        for d in [
            "node_modules",
            "target",
            "dist",
            "build",
            "__pycache__",
            "coverage",
        ] {
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

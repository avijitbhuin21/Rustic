use anyhow::Result;
use ignore::WalkBuilder;
use regex::{Regex, RegexBuilder};
use serde::Serialize;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize)]
pub struct SearchQuery {
    pub pattern: String,
    pub is_regex: bool,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub paths: Vec<PathBuf>,
    pub include_glob: Option<String>,
    pub exclude_glob: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchMatch {
    pub line_number: usize,
    pub line_text: String,
    pub match_start: usize,
    pub match_end: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchResult {
    pub file_path: String,
    pub matches: Vec<SearchMatch>,
}

#[derive(Debug, Clone, Copy, Serialize, Default)]
pub struct SearchSummary {
    pub files_scanned: u32,
    pub files_matched: u32,
    pub total_matches: u32,
    pub truncated: bool,
}

/// Directory names skipped unconditionally during recursive search. These are
/// dependency caches, build outputs, and VCS metadata — almost always huge,
/// almost never useful to grep. `.gitignore` covers most of them in well-kept
/// repos, but many projects ship without one (e.g. a `vendor/` Go module
/// tree that wasn't gitignored), so we hard-skip them as a safety net.
pub const DEFAULT_IGNORED_DIRS: &[&str] = &[
    // JS / TS
    "node_modules", "dist", "build", "out", ".next", ".nuxt", ".svelte-kit",
    ".turbo", ".parcel-cache", ".cache",
    // Rust
    "target",
    // Go / PHP / Ruby
    "vendor",
    // Python
    "__pycache__", ".pytest_cache", ".mypy_cache", ".ruff_cache", ".tox",
    "venv", ".venv", "env",
    // JVM / Android
    ".gradle",
    // .NET
    "bin", "obj",
    // Apple
    "DerivedData", "Pods",
    // Misc
    "coverage", ".terraform", ".idea",
];

pub struct SearchEngine;

impl SearchEngine {
    /// Replace all occurrences of a pattern in a single file.
    /// Returns the number of replacements made.
    pub fn replace_in_file(
        file_path: &str,
        pattern: &str,
        replacement: &str,
        is_regex: bool,
        case_sensitive: bool,
        whole_word: bool,
    ) -> Result<u32> {
        let query = SearchQuery {
            pattern: pattern.to_string(),
            is_regex,
            case_sensitive,
            whole_word,
            paths: vec![],
            include_glob: None,
            exclude_glob: None,
        };
        let regex = Self::build_regex(&query)?;
        let content = fs::read_to_string(file_path)?;
        let uses_crlf = content.contains("\r\n");
        let line_ending = if uses_crlf { "\r\n" } else { "\n" };
        let mut count = 0u32;
        let new_content: String = content
            .lines()
            .map(|line| {
                let matches_in_line = regex.find_iter(line).count() as u32;
                count += matches_in_line;
                regex.replace_all(line, replacement).into_owned()
            })
            .collect::<Vec<_>>()
            .join(line_ending);

        // Preserve trailing newline if original had one
        let final_content = if content.ends_with('\n') || content.ends_with("\r\n") {
            format!("{}{}", new_content, line_ending)
        } else {
            new_content
        };

        if count > 0 {
            crate::io_util::atomic_write(std::path::Path::new(file_path), final_content.as_bytes())?;
        }
        Ok(count)
    }

    /// Streaming search across all given paths. `on_file` is invoked once per
    /// file that has at least one match — callers (the Tauri layer) emit it
    /// to the frontend immediately so the UI fills in as the walker progresses
    /// instead of waiting for the whole tree to finish.
    ///
    /// `should_continue` is invoked between every file scanned (whether or not
    /// it matched). Returning `false` stops the walk — used by the Tauri layer
    /// to honor cancellation (when the user kicks off a newer search) and to
    /// throttle progress events.
    ///
    /// Uses ripgrep's `grep-searcher`: (a) memory-maps files, (b) detects
    /// binaries via NUL byte and skips, (c) uses a tuned line-scanning state
    /// machine. Hard caps prevent unbounded memory + UI freeze on huge
    /// projects: walking stops once any cap is hit. Directories named in
    /// `DEFAULT_IGNORED_DIRS` are never recursed into.
    pub fn search_streaming<F1, F2>(
        query: &SearchQuery,
        mut on_file: F1,
        mut should_continue: F2,
    ) -> Result<SearchSummary>
    where
        F1: FnMut(SearchResult),
        F2: FnMut(SearchSummary) -> bool,
    {
        use grep_matcher::Matcher;
        use grep_regex::RegexMatcherBuilder;
        use grep_searcher::{Searcher, Sink, SinkMatch};

        const MAX_FILES: usize = 1000;
        const MAX_MATCHES_PER_FILE: usize = 500;
        const MAX_TOTAL_MATCHES: usize = 5000;
        const MAX_FILE_SIZE_BYTES: u64 = 10 * 1024 * 1024; // 10 MB

        // Build the matcher with the same case/word semantics as before.
        let pattern = if query.is_regex {
            query.pattern.clone()
        } else {
            regex::escape(&query.pattern)
        };
        let pattern = if query.whole_word {
            format!(r"\b{}\b", pattern)
        } else {
            pattern
        };
        let matcher = RegexMatcherBuilder::new()
            .case_insensitive(!query.case_sensitive)
            .build(&pattern)?;

        let mut summary = SearchSummary::default();

        // Sink collects matches per-file. grep_searcher invokes us once per
        // matched line; we pull the byte ranges from the matcher's captures
        // and convert to char columns to match the legacy SearchMatch shape.
        // Returning Ok(false) tells grep_searcher to stop searching this file
        // — we use it to enforce the per-file match cap cheaply.
        struct CollectSink<'a, M: Matcher> {
            matcher: &'a M,
            file_matches: Vec<SearchMatch>,
            per_file_cap: usize,
        }
        impl<'a, M: Matcher> Sink for CollectSink<'a, M> {
            type Error = std::io::Error;
            fn matched(
                &mut self,
                _searcher: &Searcher,
                m: &SinkMatch<'_>,
            ) -> std::result::Result<bool, std::io::Error> {
                if self.file_matches.len() >= self.per_file_cap {
                    return Ok(false);
                }
                let line_no = m.line_number().unwrap_or(0) as usize;
                let line_bytes = m.bytes();
                let line_text = match std::str::from_utf8(line_bytes) {
                    Ok(s) => s.trim_end_matches(['\n', '\r']).to_string(),
                    Err(_) => return Ok(true), // skip non-UTF-8 lines
                };

                // The matcher may match multiple times on the same line;
                // `find_iter`-style enumeration gives us each match.
                let mut start = 0usize;
                while start < line_bytes.len() {
                    let matched = self
                        .matcher
                        .find_at(line_bytes, start)
                        .map_err(|_| std::io::Error::other("matcher failed"))?;
                    let Some(mat) = matched else { break };
                    if mat.start() == mat.end() {
                        start += 1;
                        continue;
                    }
                    // Convert byte offsets to char offsets for the line.
                    let bs = mat.start().min(line_bytes.len());
                    let be = mat.end().min(line_bytes.len());
                    let prefix = &line_text.as_bytes()[..bs.min(line_text.len())];
                    let mid = &line_text.as_bytes()[bs.min(line_text.len())..be.min(line_text.len())];
                    let match_start = std::str::from_utf8(prefix).map(|s| s.chars().count()).unwrap_or(0);
                    let match_end = match_start
                        + std::str::from_utf8(mid).map(|s| s.chars().count()).unwrap_or(0);

                    self.file_matches.push(SearchMatch {
                        line_number: line_no,
                        line_text: line_text.clone(),
                        match_start,
                        match_end,
                    });
                    if self.file_matches.len() >= self.per_file_cap {
                        return Ok(false);
                    }
                    start = mat.end();
                }
                Ok(true)
            }
        }

        'outer: for search_path in &query.paths {
            let mut walker = WalkBuilder::new(search_path);
            walker
                .hidden(true)
                .git_ignore(true)
                .max_depth(None)
                // Hard-skip well-known dependency/build/cache directories. The
                // filter is called on each entry as it's discovered; returning
                // false for a directory means the walker never descends into
                // it. This is the cheapest possible way to skip these trees
                // (whole subtree never even stat'd).
                .filter_entry(|entry| {
                    if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                        if let Some(name) = entry.file_name().to_str() {
                            return !DEFAULT_IGNORED_DIRS.contains(&name);
                        }
                    }
                    true
                });

            for entry in walker.build().flatten() {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }

                // Cap checks each scanned file. `truncated` is sticky once set
                // so the frontend can show a "narrow your search" hint.
                if summary.files_matched as usize >= MAX_FILES
                    || summary.total_matches as usize >= MAX_TOTAL_MATCHES
                {
                    summary.truncated = true;
                    break 'outer;
                }

                summary.files_scanned = summary.files_scanned.saturating_add(1);

                // Skip huge files — they're almost always generated artifacts
                // (lockfiles, bundles, datasets) and dominate walk time.
                if let Ok(meta) = entry.metadata() {
                    if meta.len() > MAX_FILE_SIZE_BYTES {
                        if !should_continue(summary) {
                            break 'outer;
                        }
                        continue;
                    }
                }

                // Apply include/exclude glob filters (legacy semantics).
                if let Some(ref include) = query.include_glob {
                    if let Ok(glob) = glob::Pattern::new(include) {
                        if !glob.matches_path(path) {
                            if !should_continue(summary) {
                                break 'outer;
                            }
                            continue;
                        }
                    }
                }
                if let Some(ref exclude) = query.exclude_glob {
                    if let Ok(glob) = glob::Pattern::new(exclude) {
                        if glob.matches_path(path) {
                            if !should_continue(summary) {
                                break 'outer;
                            }
                            continue;
                        }
                    }
                }

                let remaining =
                    MAX_TOTAL_MATCHES.saturating_sub(summary.total_matches as usize);
                let per_file_cap = MAX_MATCHES_PER_FILE.min(remaining);
                if per_file_cap == 0 {
                    summary.truncated = true;
                    break 'outer;
                }

                let mut sink = CollectSink {
                    matcher: &matcher,
                    file_matches: Vec::new(),
                    per_file_cap,
                };
                let mut searcher = Searcher::new();
                if searcher.search_path(&matcher, path, &mut sink).is_err() {
                    // Binary or read error — skip silently to match prior behavior.
                    if !should_continue(summary) {
                        break 'outer;
                    }
                    continue;
                }

                if !sink.file_matches.is_empty() {
                    summary.files_matched = summary.files_matched.saturating_add(1);
                    summary.total_matches = summary
                        .total_matches
                        .saturating_add(sink.file_matches.len() as u32);
                    on_file(SearchResult {
                        file_path: path.to_string_lossy().to_string(),
                        matches: sink.file_matches,
                    });
                }

                if !should_continue(summary) {
                    break 'outer;
                }
            }
        }

        Ok(summary)
    }

    fn build_regex(query: &SearchQuery) -> Result<Regex> {
        let mut pattern = if query.is_regex {
            query.pattern.clone()
        } else {
            regex::escape(&query.pattern)
        };

        if query.whole_word {
            pattern = format!(r"\b{}\b", pattern);
        }

        let regex = RegexBuilder::new(&pattern)
            .case_insensitive(!query.case_sensitive)
            .build()?;

        Ok(regex)
    }
}

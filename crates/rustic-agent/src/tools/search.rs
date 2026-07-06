use super::{ToolContext, ToolOutput};
use crate::provider::ToolDef;
use crate::task::permissions::Action;
use anyhow::Result;
use serde_json::{json, Value};

use super::coerce_batch_array;

pub fn definitions() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "grep_search".into(),
            description: "Search for a pattern in files within the project. Returns matching \
                          lines with file paths and line numbers. \
                          \
                          CONTEXT LINES: pass `context_before` and/or `context_after` (integers, \
                          capped at 10) to show N lines before/after each match, like `grep -B/-A`. \
                          `context` is a shorthand that sets both before and after (like `grep -C`). \
                          Context output uses `>` to mark matched lines and `:` for context lines, \
                          with `--` separators between match groups. The 100-result cap counts \
                          matches only, not context lines. Without context params, output is the \
                          same as before (file:line: content). \
                          \
                          BATCH MODE: pass `queries: [{query, path?, include?, exclude?, \
                          context_before?, context_after?, context?}, ...]` \
                          to run several searches in one call. Mutually exclusive with the \
                          top-level fields. Each entry returns up to 100 results independently; \
                          empty array is an error.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Search pattern (regex supported). Required in single-search mode; omit when using `queries`." },
                    "path": { "type": "string", "description": "Subdirectory to search in (relative to project root, optional)" },
                    "include": { "type": "string", "description": "Glob pattern for files to include (e.g. '*.rs')" },
                    "exclude": { "type": "string", "description": "Glob pattern for files to exclude" },
                    "context_before": { "type": "integer", "description": "Number of lines to show before each match (like grep -B). Capped at 10." },
                    "context_after": { "type": "integer", "description": "Number of lines to show after each match (like grep -A). Capped at 10." },
                    "context": { "type": "integer", "description": "Shorthand: show N lines before AND after each match (like grep -C). Sets both context_before and context_after. Capped at 10." },
                    "queries": {
                        "type": "array",
                        "description": "Batch mode: run N searches in one call. Each entry uses the same shape as a single-search call (`{query, path?, include?, exclude?, context_before?, context_after?, context?}`). Mutually exclusive with the top-level `query`/`path`/`include`/`exclude` fields. Empty array is an error.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "query": { "type": "string" },
                                "path": { "type": "string" },
                                "include": { "type": "string" },
                                "exclude": { "type": "string" },
                                "context_before": { "type": "integer" },
                                "context_after": { "type": "integer" },
                                "context": { "type": "integer" }
                            },
                            "required": ["query"]
                        }
                    }
                }
            }),
        },
        ToolDef {
            name: "glob".into(),
            description: "Find files by glob pattern. Returns matching file paths, newest first. \
                          Use this to LOCATE files before reading them — far cheaper than \
                          list_directory + read_file guessing. Respects .gitignore. \
                          Patterns support ** (recursive), * (any chars in one segment), \
                          ? (single char), and {a,b} alternatives. Results are capped at \
                          200 paths. \
                          \
                          BATCH MODE: pass `patterns: [{pattern, path?}, ...]` to run several \
                          glob queries in one call. Mutually exclusive with the top-level \
                          `pattern`/`path` fields. Each entry returns up to 200 results \
                          independently; empty array is an error.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Glob pattern relative to project root. \
                                        Examples: 'src/**/*.rs', 'crates/*/Cargo.toml', \
                                        '**/README.md', 'tests/**/*.{js,ts}'. \
                                        Required in single-pattern mode; omit when using `patterns`."
                    },
                    "path": {
                        "type": "string",
                        "description": "Subdirectory to anchor the search under (relative to project root). \
                                        Omit to search the whole project."
                    },
                    "patterns": {
                        "type": "array",
                        "description": "Batch mode: run N glob queries in one call. Each entry uses the same shape as a single-glob call (`{pattern, path?}`). Mutually exclusive with the top-level `pattern`/`path` fields. Empty array is an error.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "pattern": { "type": "string" },
                                "path": { "type": "string" }
                            },
                            "required": ["pattern"]
                        }
                    }
                }
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

// ── grep_search ──────────────────────────────────────────────────────────────

async fn execute_grep_dispatch(
    tool_use_id: &str,
    params: Value,
    context: &ToolContext,
) -> Result<ToolOutput> {
    if let Some(queries) = coerce_batch_array(params.get("queries")) {
        let mixed = params.get("query").is_some()
            || params.get("path").is_some()
            || params.get("include").is_some()
            || params.get("exclude").is_some();
        if mixed {
            return Ok(ToolOutput {
                content: "BATCH_GREP_REJECTED: `queries` was provided alongside top-level \
                          `query`/`path`/`include`/`exclude` fields. Use one shape or the other, not both."
                    .into(),
                is_error: true, attachments: Vec::new() });
        }
        return execute_grep_batch(tool_use_id, queries, context).await;
    }
    execute_grep_one(tool_use_id, params, context).await
}

async fn execute_grep_batch(
    tool_use_id: &str,
    queries: Vec<Value>,
    context: &ToolContext,
) -> Result<ToolOutput> {
    if queries.is_empty() {
        return Ok(ToolOutput {
            content: "BATCH_GREP_REJECTED: `queries` array is empty. Pass at least one entry, \
                      or use the single-search shape `{ query, path?, include?, exclude? }`."
                .into(),
            is_error: true,
            attachments: Vec::new(),
        });
    }
    let mut shape_errors: Vec<String> = Vec::new();
    for (i, entry) in queries.iter().enumerate() {
        let q = entry
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if q.is_empty() {
            shape_errors.push(format!(
                "entry[{}]: `query` is required and must be non-empty",
                i
            ));
        }
    }
    if !shape_errors.is_empty() {
        return Ok(ToolOutput {
            content: format!(
                "BATCH_GREP_REJECTED: {} entry/entries failed validation.\n{}",
                shape_errors.len(),
                shape_errors.join("\n"),
            ),
            is_error: true,
            attachments: Vec::new(),
        });
    }

    let mut out = String::new();
    let mut all_errored = true;
    for (i, entry) in queries.iter().enumerate() {
        let query_preview = entry.get("query").and_then(|v| v.as_str()).unwrap_or("");
        out.push_str(&format!(
            "=== grep_search entry {}: \"{}\" ===\n",
            i + 1,
            query_preview
        ));
        let result = execute_grep_one(tool_use_id, entry.clone(), context).await?;
        if !result.is_error {
            all_errored = false;
        }
        out.push_str(&result.content);
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push('\n');
    }
    Ok(ToolOutput {
        content: out.trim_end().to_string(),
        is_error: all_errored,
        attachments: Vec::new(),
    })
}

/// Maximum context lines before/after a match (hard cap).
const MAX_CONTEXT_LINES: usize = 10;

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

    let search_path = params["path"]
        .as_str()
        .map(|p| context.project_root.join(p))
        .unwrap_or_else(|| context.project_root.clone());

    let include_glob = params["include"].as_str().map(|s| s.to_string());
    let exclude_glob = params["exclude"].as_str().map(|s| s.to_string());

    let (ctx_before, ctx_after) = parse_context_params(&params);
    let use_context = ctx_before > 0 || ctx_after > 0;

    let regex = match regex::RegexBuilder::new(query)
        .case_insensitive(true)
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            return Ok(ToolOutput {
                content: format!("Invalid regex: {}", e),
                is_error: true,
                attachments: Vec::new(),
            })
        }
    };

    let show_all = context.sensitive_files_allowed();
    let walker = ignore::WalkBuilder::new(&search_path)
        .hidden(!show_all)
        .git_ignore(!show_all)
        .filter_entry(|e| e.file_name() != std::ffi::OsStr::new(".git"))
        .build();

    // flat results for no-context mode (output byte-identical to original)
    let mut results: Vec<String> = Vec::new();
    // accumulated output for context mode
    let mut ctx_output = String::new();
    let max_results: usize = 100;
    let mut match_count = 0usize;
    let mut files_searched = 0u32;

    context.emit_progress(tool_use_id, &format!("Searching for \"{}\"...", query));

    'outer: for entry in walker.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        if let Some(ref include) = include_glob {
            if let Ok(glob) = glob::Pattern::new(include) {
                if !glob.matches_path(path) {
                    continue;
                }
            }
        }
        if let Some(ref exclude) = exclude_glob {
            if let Ok(glob) = glob::Pattern::new(exclude) {
                if glob.matches_path(path) {
                    continue;
                }
            }
        }

        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        files_searched += 1;

        let rel_path = path
            .strip_prefix(&context.project_root)
            .unwrap_or(path)
            .to_string_lossy()
            .into_owned();

        if !use_context {
            // ── original behaviour: no context lines ──────────────────────
            for (i, line) in content.lines().enumerate() {
                if regex.is_match(line) {
                    results.push(format!("{}:{}: {}", rel_path, i + 1, line.trim()));
                    match_count += 1;

                    if match_count % 20 == 0 {
                        context.emit_progress(
                            tool_use_id,
                            &format!("{} matches in {} files...", match_count, files_searched),
                        );
                    }

                    if match_count >= max_results {
                        results.push(format!("... (truncated at {} results)", max_results));
                        break 'outer;
                    }
                }
            }
        } else {
            // ── context mode ──────────────────────────────────────────────
            let file_lines: Vec<&str> = content.lines().collect();
            let num_lines = file_lines.len();

            // Collect 0-based indices of matching lines.
            let mut match_indices: Vec<usize> = Vec::new();
            for (i, line) in file_lines.iter().enumerate() {
                if regex.is_match(line) {
                    match_indices.push(i);
                }
            }

            if match_indices.is_empty() {
                continue;
            }

            // Build groups: merge overlapping/adjacent context windows so we
            // only emit `--` between truly separate stretches.
            // Each entry: (line_start, line_end, match_line_indices_in_group)
            let mut groups: Vec<(usize, usize, Vec<usize>)> = Vec::new();

            for &mi in &match_indices {
                let grp_start = mi.saturating_sub(ctx_before);
                let grp_end = (mi + ctx_after).min(num_lines.saturating_sub(1));

                if let Some(last) = groups.last_mut() {
                    if grp_start <= last.1 + 1 {
                        // Overlaps or is adjacent — extend the existing group.
                        last.1 = last.1.max(grp_end);
                        last.2.push(mi);
                        continue;
                    }
                }
                groups.push((grp_start, grp_end, vec![mi]));
            }

            // Emit groups; cap counts matches, not context lines.
            let mut file_header_written = false;
            for (grp_start, grp_end, grp_matches) in groups {
                let n_in_group = grp_matches.len();
                if match_count + n_in_group > max_results {
                    ctx_output.push_str(&format!("... (truncated at {} results)\n", max_results));
                    break 'outer;
                }
                match_count += n_in_group;

                if match_count % 20 == 0 {
                    context.emit_progress(
                        tool_use_id,
                        &format!("{} matches in {} files...", match_count, files_searched),
                    );
                }

                // `--` separator between groups / files.
                if !file_header_written {
                    if !ctx_output.is_empty() {
                        ctx_output.push_str("--\n");
                    }
                    file_header_written = true;
                } else {
                    ctx_output.push_str("--\n");
                }

                let match_set: std::collections::HashSet<usize> =
                    grp_matches.iter().copied().collect();

                for line_idx in grp_start..=grp_end {
                    let line_no = line_idx + 1; // 1-based
                    let raw_line = file_lines[line_idx];
                    if match_set.contains(&line_idx) {
                        ctx_output.push_str(&format!("{}>{}: {}\n", rel_path, line_no, raw_line));
                    } else {
                        ctx_output.push_str(&format!("{}:{}: {}\n", rel_path, line_no, raw_line));
                    }
                }
            }
        }
    }

    if !use_context {
        if results.is_empty() {
            Ok(ToolOutput {
                content: "No matches found".into(),
                is_error: false,
                attachments: Vec::new(),
            })
        } else {
            Ok(ToolOutput {
                content: results.join("\n"),
                is_error: false,
                attachments: Vec::new(),
            })
        }
    } else if ctx_output.is_empty() {
        Ok(ToolOutput {
            content: "No matches found".into(),
            is_error: false,
            attachments: Vec::new(),
        })
    } else {
        Ok(ToolOutput {
            content: ctx_output.trim_end_matches('\n').to_string(),
            is_error: false,
            attachments: Vec::new(),
        })
    }
}

// ── glob ─────────────────────────────────────────────────────────────────────

async fn execute_glob_dispatch(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    if let Some(patterns) = coerce_batch_array(params.get("patterns")) {
        let mixed = params.get("pattern").is_some() || params.get("path").is_some();
        if mixed {
            return Ok(ToolOutput {
                content: "BATCH_GLOB_REJECTED: `patterns` was provided alongside top-level \
                          `pattern`/`path` fields. Use one shape or the other, not both."
                    .into(),
                is_error: true,
                attachments: Vec::new(),
            });
        }
        return execute_glob_batch(patterns, context).await;
    }
    execute_glob_one(params, context).await
}

async fn execute_glob_batch(patterns: Vec<Value>, context: &ToolContext) -> Result<ToolOutput> {
    if patterns.is_empty() {
        return Ok(ToolOutput {
            content: "BATCH_GLOB_REJECTED: `patterns` array is empty. Pass at least one entry, \
                      or use the single-pattern shape `{ pattern, path? }`."
                .into(),
            is_error: true,
            attachments: Vec::new(),
        });
    }
    let mut shape_errors: Vec<String> = Vec::new();
    for (i, entry) in patterns.iter().enumerate() {
        let p = entry
            .get("pattern")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if p.is_empty() {
            shape_errors.push(format!(
                "entry[{}]: `pattern` is required and must be non-empty",
                i
            ));
        }
    }
    if !shape_errors.is_empty() {
        return Ok(ToolOutput {
            content: format!(
                "BATCH_GLOB_REJECTED: {} entry/entries failed validation.\n{}",
                shape_errors.len(),
                shape_errors.join("\n"),
            ),
            is_error: true,
            attachments: Vec::new(),
        });
    }

    let mut out = String::new();
    let mut all_errored = true;
    for (i, entry) in patterns.iter().enumerate() {
        let pat_preview = entry.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
        out.push_str(&format!(
            "=== glob entry {}: \"{}\" ===\n",
            i + 1,
            pat_preview
        ));
        let result = execute_glob_one(entry.clone(), context).await?;
        if !result.is_error {
            all_errored = false;
        }
        out.push_str(&result.content);
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push('\n');
    }
    Ok(ToolOutput {
        content: out.trim_end().to_string(),
        is_error: all_errored,
        attachments: Vec::new(),
    })
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

    let walker = ignore::WalkBuilder::new(&search_root)
        .hidden(true)
        .git_ignore(true)
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

    // ── grouping logic — test directly against the group-building algorithm ───
    //
    // We extract the group-building step inline so it can be tested without
    // a ToolContext.  The same code path runs inside execute_grep_one.

    /// Build groups from a sorted list of 0-based match indices and context window sizes.
    /// Returns Vec<(start, end, match_indices_in_group)>.
    fn build_groups(
        match_indices: &[usize],
        file_len: usize,
        ctx_before: usize,
        ctx_after: usize,
    ) -> Vec<(usize, usize, Vec<usize>)> {
        let mut groups: Vec<(usize, usize, Vec<usize>)> = Vec::new();
        for &mi in match_indices {
            let grp_start = mi.saturating_sub(ctx_before);
            let grp_end = (mi + ctx_after).min(file_len.saturating_sub(1));
            if let Some(last) = groups.last_mut() {
                if grp_start <= last.1 + 1 {
                    last.1 = last.1.max(grp_end);
                    last.2.push(mi);
                    continue;
                }
            }
            groups.push((grp_start, grp_end, vec![mi]));
        }
        groups
    }

    #[test]
    fn test_groups_single_match_no_context() {
        // A single match, no context: the group spans exactly that line.
        let groups = build_groups(&[4], 10, 0, 0);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0], (4, 4, vec![4]));
    }

    #[test]
    fn test_groups_single_match_with_context() {
        // Match at line 5 (0-based), context 2 before / 2 after, file has 10 lines.
        let groups = build_groups(&[5], 10, 2, 2);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].0, 3); // 5 - 2
        assert_eq!(groups[0].1, 7); // 5 + 2
        assert_eq!(groups[0].2, vec![5]);
    }

    #[test]
    fn test_groups_context_clipped_at_start() {
        // Match at line 1, context 5 before: start should clamp at 0.
        let groups = build_groups(&[1], 20, 5, 0);
        assert_eq!(groups[0].0, 0);
    }

    #[test]
    fn test_groups_context_clipped_at_end() {
        // Match at line 18, file has 20 lines (0-based 0..19), context 5 after: end clamps at 19.
        let groups = build_groups(&[18], 20, 0, 5);
        assert_eq!(groups[0].1, 19);
    }

    #[test]
    fn test_groups_two_matches_overlapping_merge() {
        // Matches at 3 and 5, context=2: windows [1-5] and [3-7] → merged [1-7].
        let groups = build_groups(&[3, 5], 10, 2, 2);
        assert_eq!(groups.len(), 1, "windows overlap → must merge");
        assert_eq!(groups[0].0, 1);
        assert_eq!(groups[0].1, 7);
        assert_eq!(groups[0].2.len(), 2);
    }

    #[test]
    fn test_groups_two_matches_adjacent_merge() {
        // Matches at 2 and 5, context=1: windows [1-3] and [4-6] → adjacent (+1 rule) → merged.
        let groups = build_groups(&[2, 5], 10, 1, 1);
        assert_eq!(groups.len(), 1, "adjacent windows must merge");
    }

    #[test]
    fn test_groups_two_matches_separated() {
        // Matches at 0 and 9, context=1, file=20 lines: no overlap.
        let groups = build_groups(&[0, 9], 20, 1, 1);
        assert_eq!(groups.len(), 2, "non-adjacent windows must stay separate");
        assert_eq!(groups[0].2, vec![0]);
        assert_eq!(groups[1].2, vec![9]);
    }

    #[test]
    fn test_groups_three_matches_first_two_merge_third_separate() {
        // Matches at 2, 4, 15; context=1 → [1-3], [3-5] merge → [1-5]; [14-16] separate.
        let groups = build_groups(&[2, 4, 15], 20, 1, 1);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].2.len(), 2); // matches 2 and 4
        assert_eq!(groups[1].2, vec![15]);
    }

    // ── context output format — test the rendering logic in isolation ──────────
    //
    // We call the rendering logic directly on known data without needing a
    // file-system walk or ToolContext.

    fn render_group(
        file_lines: &[&str],
        grp_start: usize,
        grp_end: usize,
        match_indices: &[usize],
        rel_path: &str,
    ) -> String {
        let match_set: std::collections::HashSet<usize> = match_indices.iter().copied().collect();
        let mut out = String::new();
        for line_idx in grp_start..=grp_end {
            let line_no = line_idx + 1;
            let raw_line = file_lines[line_idx];
            if match_set.contains(&line_idx) {
                out.push_str(&format!("{}>{}: {}\n", rel_path, line_no, raw_line));
            } else {
                out.push_str(&format!("{}:{}: {}\n", rel_path, line_no, raw_line));
            }
        }
        out
    }

    #[test]
    fn test_render_match_line_uses_gt_marker() {
        let lines = ["before", "MATCH", "after"];
        let out = render_group(&lines, 0, 2, &[1], "f.txt");
        // Line 1 (0-based) is the match: line_no=2
        assert!(out.contains("f.txt>2: MATCH"), "match marker: {}", out);
        assert!(out.contains("f.txt:1: before"), "ctx before: {}", out);
        assert!(out.contains("f.txt:3: after"), "ctx after: {}", out);
    }

    #[test]
    fn test_render_multiple_matches_in_one_group() {
        let lines = ["A", "MATCH1", "B", "MATCH2", "C"];
        let out = render_group(&lines, 0, 4, &[1, 3], "x.rs");
        assert!(out.contains("x.rs>2: MATCH1"), "{}", out);
        assert!(out.contains("x.rs>4: MATCH2"), "{}", out);
        assert!(out.contains("x.rs:1: A"), "{}", out);
        assert!(out.contains("x.rs:3: B"), "{}", out);
        assert!(out.contains("x.rs:5: C"), "{}", out);
    }

    #[test]
    fn test_render_context_line_uses_colon_marker() {
        let lines = ["ctx", "MATCH", "ctx"];
        let out = render_group(&lines, 0, 2, &[1], "p.txt");
        // Context lines use `:` not `>`
        let ctx_lines: Vec<&str> = out
            .lines()
            .filter(|l| l.contains(":1:") || l.contains(":3:"))
            .collect();
        assert_eq!(
            ctx_lines.len(),
            2,
            "both context lines should use colon: {}",
            out
        );
        for l in ctx_lines {
            assert!(!l.contains(">"), "context line must not use `>`: {}", l);
        }
    }

    // ── MAX_CONTEXT_LINES constant ────────────────────────────────────────────

    #[test]
    fn test_max_context_lines_is_10() {
        assert_eq!(MAX_CONTEXT_LINES, 10);
    }
}

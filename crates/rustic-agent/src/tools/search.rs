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
                          BATCH MODE: pass `queries: [{query, path?, include?, exclude?}, ...]` \
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
                    "queries": {
                        "type": "array",
                        "description": "Batch mode: run N searches in one call. Each entry uses the same shape as a single-search call (`{query, path?, include?, exclude?}`). Mutually exclusive with the top-level `query`/`path`/`include`/`exclude` fields. Empty array is an error.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "query": { "type": "string" },
                                "path": { "type": "string" },
                                "include": { "type": "string" },
                                "exclude": { "type": "string" }
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

pub async fn execute(name: &str, tool_use_id: &str, params: Value, context: &ToolContext) -> Result<ToolOutput> {
    if !context.check_permission(&Action::Read) {
        return Ok(ToolOutput {
            content: "Permission denied: read not allowed".into(),
            is_error: true, attachments: Vec::new() });
    }

    if name == "glob" {
        return execute_glob_dispatch(params, context).await;
    }
    execute_grep_dispatch(tool_use_id, params, context).await
}

// ── grep_search ──────────────────────────────────────────────────────────────

async fn execute_grep_dispatch(tool_use_id: &str, params: Value, context: &ToolContext) -> Result<ToolOutput> {
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

async fn execute_grep_batch(tool_use_id: &str, queries: Vec<Value>, context: &ToolContext) -> Result<ToolOutput> {
    if queries.is_empty() {
        return Ok(ToolOutput {
            content: "BATCH_GREP_REJECTED: `queries` array is empty. Pass at least one entry, \
                      or use the single-search shape `{ query, path?, include?, exclude? }`.".into(),
            is_error: true, attachments: Vec::new() });
    }
    let mut shape_errors: Vec<String> = Vec::new();
    for (i, entry) in queries.iter().enumerate() {
        let q = entry.get("query").and_then(|v| v.as_str()).unwrap_or("").trim();
        if q.is_empty() {
            shape_errors.push(format!("entry[{}]: `query` is required and must be non-empty", i));
        }
    }
    if !shape_errors.is_empty() {
        return Ok(ToolOutput {
            content: format!(
                "BATCH_GREP_REJECTED: {} entry/entries failed validation.\n{}",
                shape_errors.len(), shape_errors.join("\n"),
            ),
            is_error: true, attachments: Vec::new() });
    }

    let mut out = String::new();
    let mut all_errored = true;
    for (i, entry) in queries.iter().enumerate() {
        let query_preview = entry.get("query").and_then(|v| v.as_str()).unwrap_or("");
        out.push_str(&format!("=== grep_search entry {}: \"{}\" ===\n", i + 1, query_preview));
        let result = execute_grep_one(tool_use_id, entry.clone(), context).await?;
        if !result.is_error { all_errored = false; }
        out.push_str(&result.content);
        if !out.ends_with('\n') { out.push('\n'); }
        out.push('\n');
    }
    Ok(ToolOutput {
        content: out.trim_end().to_string(),
        is_error: all_errored, attachments: Vec::new() })
}

async fn execute_grep_one(tool_use_id: &str, params: Value, context: &ToolContext) -> Result<ToolOutput> {
    let query = params["query"].as_str().unwrap_or("");
    if query.is_empty() {
        return Ok(ToolOutput {
            content: "No search query provided".into(),
            is_error: true, attachments: Vec::new() });
    }

    let search_path = params["path"]
        .as_str()
        .map(|p| context.project_root.join(p))
        .unwrap_or_else(|| context.project_root.clone());

    let include_glob = params["include"].as_str().map(|s| s.to_string());
    let exclude_glob = params["exclude"].as_str().map(|s| s.to_string());

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

    let walker = ignore::WalkBuilder::new(&search_path)
        .hidden(true)
        .git_ignore(true)
        .build();

    let mut results = Vec::new();
    let max_results = 100;
    let mut files_searched = 0u32;

    context.emit_progress(tool_use_id, &format!("Searching for \"{}\"...", query));

    for entry in walker.flatten() {
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
            .to_string_lossy();

        for (i, line) in content.lines().enumerate() {
            if regex.is_match(line) {
                results.push(format!("{}:{}: {}", rel_path, i + 1, line.trim()));

                if results.len() % 20 == 0 {
                    context.emit_progress(
                        tool_use_id,
                        &format!("{} matches in {} files...", results.len(), files_searched),
                    );
                }

                if results.len() >= max_results {
                    results.push(format!("... (truncated at {} results)", max_results));
                    return Ok(ToolOutput {
                        content: results.join("\n"),
                        is_error: false, attachments: Vec::new() });
                }
            }
        }
    }

    if results.is_empty() {
        Ok(ToolOutput {
            content: "No matches found".into(),
            is_error: false, attachments: Vec::new() })
    } else {
        Ok(ToolOutput {
            content: results.join("\n"),
            is_error: false, attachments: Vec::new() })
    }
}

// ── glob ─────────────────────────────────────────────────────────────────────

async fn execute_glob_dispatch(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    if let Some(patterns) = coerce_batch_array(params.get("patterns")) {
        let mixed = params.get("pattern").is_some() || params.get("path").is_some();
        if mixed {
            return Ok(ToolOutput {
                content: "BATCH_GLOB_REJECTED: `patterns` was provided alongside top-level \
                          `pattern`/`path` fields. Use one shape or the other, not both.".into(),
                is_error: true, attachments: Vec::new() });
        }
        return execute_glob_batch(patterns, context).await;
    }
    execute_glob_one(params, context).await
}

async fn execute_glob_batch(patterns: Vec<Value>, context: &ToolContext) -> Result<ToolOutput> {
    if patterns.is_empty() {
        return Ok(ToolOutput {
            content: "BATCH_GLOB_REJECTED: `patterns` array is empty. Pass at least one entry, \
                      or use the single-pattern shape `{ pattern, path? }`.".into(),
            is_error: true, attachments: Vec::new() });
    }
    let mut shape_errors: Vec<String> = Vec::new();
    for (i, entry) in patterns.iter().enumerate() {
        let p = entry.get("pattern").and_then(|v| v.as_str()).unwrap_or("").trim();
        if p.is_empty() {
            shape_errors.push(format!("entry[{}]: `pattern` is required and must be non-empty", i));
        }
    }
    if !shape_errors.is_empty() {
        return Ok(ToolOutput {
            content: format!(
                "BATCH_GLOB_REJECTED: {} entry/entries failed validation.\n{}",
                shape_errors.len(), shape_errors.join("\n"),
            ),
            is_error: true, attachments: Vec::new() });
    }

    let mut out = String::new();
    let mut all_errored = true;
    for (i, entry) in patterns.iter().enumerate() {
        let pat_preview = entry.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
        out.push_str(&format!("=== glob entry {}: \"{}\" ===\n", i + 1, pat_preview));
        let result = execute_glob_one(entry.clone(), context).await?;
        if !result.is_error { all_errored = false; }
        out.push_str(&result.content);
        if !out.ends_with('\n') { out.push('\n'); }
        out.push('\n');
    }
    Ok(ToolOutput {
        content: out.trim_end().to_string(),
        is_error: all_errored, attachments: Vec::new() })
}

/// Find files by glob pattern, newest-modified first.
async fn execute_glob_one(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    let pattern = params["pattern"].as_str().unwrap_or("").trim();
    if pattern.is_empty() {
        return Ok(ToolOutput {
            content: "GLOB_ERROR: `pattern` is required (e.g. 'src/**/*.rs').".into(),
            is_error: true, attachments: Vec::new() });
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
        is_error: false, attachments: Vec::new() })
}

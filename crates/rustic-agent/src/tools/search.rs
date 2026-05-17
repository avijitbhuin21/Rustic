use super::{ToolContext, ToolOutput};
use crate::provider::ToolDef;
use crate::task::permissions::Action;
use anyhow::Result;
use serde_json::{json, Value};

pub fn definitions() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "grep_search".into(),
            description: "Search for a pattern in files within the project. Returns matching lines with file paths and line numbers.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Search pattern (regex supported)" },
                    "path": { "type": "string", "description": "Subdirectory to search in (relative to project root, optional)" },
                    "include": { "type": "string", "description": "Glob pattern for files to include (e.g. '*.rs')" },
                    "exclude": { "type": "string", "description": "Glob pattern for files to exclude" }
                },
                "required": ["query"]
            }),
        },
        ToolDef {
            name: "glob".into(),
            description: "Find files by glob pattern. Returns matching file paths, newest first. \
                          Use this to LOCATE files before reading them — far cheaper than \
                          list_directory + read_file guessing. Respects .gitignore. \
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

pub async fn execute(name: &str, tool_use_id: &str, params: Value, context: &ToolContext) -> Result<ToolOutput> {
    if !context.check_permission(&Action::Read) {
        return Ok(ToolOutput {
            content: "Permission denied: read not allowed".into(),
            is_error: true, attachments: Vec::new() });
    }

    if name == "glob" {
        return execute_glob(params, context).await;
    }

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

/// Find files by glob pattern, newest-modified first.
async fn execute_glob(params: Value, context: &ToolContext) -> Result<ToolOutput> {
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

use super::{ToolContext, ToolOutput};
use crate::provider::ToolDef;
use crate::task::permissions::Action;
use anyhow::Result;
use serde_json::{json, Value};

pub fn definitions() -> Vec<ToolDef> {
    vec![ToolDef {
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
    }]
}

pub async fn execute(_name: &str, tool_use_id: &str, params: Value, context: &ToolContext) -> Result<ToolOutput> {
    if !context.check_permission(&Action::Read) {
        return Ok(ToolOutput {
            content: "Permission denied: read not allowed".into(),
            is_error: true,
        });
    }

    let query = params["query"].as_str().unwrap_or("");
    if query.is_empty() {
        return Ok(ToolOutput {
            content: "No search query provided".into(),
            is_error: true,
        });
    }

    let search_path = params["path"]
        .as_str()
        .map(|p| context.project_root.join(p))
        .unwrap_or_else(|| context.project_root.clone());

    let include_glob = params["include"].as_str().map(|s| s.to_string());
    let exclude_glob = params["exclude"].as_str().map(|s| s.to_string());

    // Use the regex + ignore walker approach directly here
    let regex = match regex::RegexBuilder::new(query)
        .case_insensitive(true)
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            return Ok(ToolOutput {
                content: format!("Invalid regex: {}", e),
                is_error: true,
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

                // Emit progress every 20 matches
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
                        is_error: false,
                    });
                }
            }
        }
    }

    if results.is_empty() {
        Ok(ToolOutput {
            content: "No matches found".into(),
            is_error: false,
        })
    } else {
        Ok(ToolOutput {
            content: results.join("\n"),
            is_error: false,
        })
    }
}

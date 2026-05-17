//! Deferred tool schemas via the `tool_search` meta-tool.
//!
//! The system prompt advertises only always-on tools; all others are browsable
//! via `tool_search`. Matched tools are appended to `ToolContext.loaded_deferred_tools`
//! and included in subsequent turn requests. Sub-agents inherit the parent's set.

use crate::provider::ToolDef;
use crate::tools::{ToolContext, ToolOutput};
use anyhow::Result;
use serde_json::Value;

/// Core tools always included in every request; everything else is deferred and surfaced via `tool_search`.
pub const ALWAYS_ON: &[&str] = &[
    "read_file",
    "edit_file",
    "grep_search",
    "glob",
    "run_command",
    "web_search",
    "todo_write",
    "ask_user",
    "tool_search",
    "goal_complete",
];

pub fn is_always_on(name: &str) -> bool {
    ALWAYS_ON.contains(&name)
}

pub fn directory_line(def: &ToolDef) -> String {
    const PREVIEW_CHARS: usize = 30;
    let mut short = def.description.replace('\n', " ");
    if short.len() > PREVIEW_CHARS {
        let cutoff = short
            .char_indices()
            .nth(PREVIEW_CHARS)
            .map(|(i, _)| i)
            .unwrap_or(short.len());
        short.truncate(cutoff);
        short.push('…');
    }
    format!("- `{}` — {}", def.name, short.trim())
}

/// Resolves a `query` to deferred tool schemas and marks them loaded on the task context.
/// Returns `{ loaded: [...], tools: [...] }` — executor reads `loaded` to include schemas next turn.
pub async fn execute(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    let query = params
        .get("query")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let max_results = params
        .get("max_results")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(5)
        .clamp(1, 20);

    if query.is_empty() {
        return Ok(ToolOutput {
            content: "tool_search requires a `query` parameter. Use either \
                      `select:NAME[,NAME2]` for exact lookup or a free-text query \
                      like \"worktree\" / \"+slack send\"."
                .into(),
            is_error: true, attachments: Vec::new() });
    }

    let table = deferred_table();
    if table.is_empty() {
        return Ok(ToolOutput {
            content: "tool_search: no deferred tools are currently registered. The \
                      always-on set already includes everything available — call those \
                      tools directly."
                .into(),
            is_error: false, attachments: Vec::new() });
    }

    let mut selected: Vec<ToolDef> = Vec::new();
    if let Some(rest) = query.strip_prefix("select:") {
        for raw in rest.split(',') {
            let want = raw.trim();
            if want.is_empty() {
                continue;
            }
            if let Some(def) = table.iter().find(|t| t.name == want) {
                selected.push(def.clone());
            }
        }
        if selected.is_empty() {
            return Ok(ToolOutput {
                content: format!(
                    "tool_search: no deferred tool matched `{}`. Use a free-text query \
                     (e.g. `tool_search({{ \"query\": \"worktree\" }})`) to discover \
                     names first.",
                    query
                ),
                is_error: true,
                attachments: Vec::new(),
            });
        }
    } else {
        let terms: Vec<String> = query
            .split_whitespace()
            .map(|s| s.to_lowercase())
            .collect();
        let mut scored: Vec<(usize, &ToolDef)> = table
            .iter()
            .filter_map(|t| {
                let blob = format!("{} {}", t.name, t.description).to_lowercase();
                let hits: usize = terms.iter().map(|term| blob.matches(term).count()).sum();
                if hits == 0 { None } else { Some((hits, t)) }
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0));
        for (_, t) in scored.into_iter().take(max_results) {
            selected.push(t.clone());
        }
        if selected.is_empty() {
            return Ok(ToolOutput {
                content: format!(
                    "tool_search: nothing matched `{}`. Try a different keyword or read \
                     the deferred-tool directory in your system prompt for available names.",
                    query
                ),
                is_error: false,
                attachments: Vec::new(),
            });
        }
    }

    if let Ok(mut loaded) = context.loaded_deferred_tools.lock() {
        for def in &selected {
            loaded.insert(def.name.clone());
        }
    }

    let body = serde_json::json!({
        "loaded": selected.iter().map(|d| d.name.clone()).collect::<Vec<_>>(),
        "tools": selected.iter().map(|d| serde_json::json!({
            "name": d.name,
            "description": d.description,
            "input_schema": d.parameters,
        })).collect::<Vec<_>>(),
        "note": "These tools are now available for the rest of this task — you can call them directly without another tool_search.",
    });
    Ok(ToolOutput {
        content: serde_json::to_string_pretty(&body)
            .unwrap_or_else(|_| body.to_string()),
        is_error: false, attachments: Vec::new() })
}

static DEFERRED_TABLE: std::sync::OnceLock<std::sync::Mutex<Vec<ToolDef>>> =
    std::sync::OnceLock::new();

fn deferred_table_cell() -> &'static std::sync::Mutex<Vec<ToolDef>> {
    DEFERRED_TABLE.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

fn deferred_table() -> Vec<ToolDef> {
    deferred_table_cell()
        .lock()
        .map(|v| v.clone())
        .unwrap_or_default()
}

pub fn set_deferred_table(tools: Vec<ToolDef>) {
    if let Ok(mut guard) = deferred_table_cell().lock() {
        *guard = tools;
    }
}

pub fn tool_search_def() -> ToolDef {
    ToolDef {
        name: "tool_search".into(),
        description: "Look up the full JSON schema for one or more deferred tools. The \
                      system prompt lists every deferred tool's name + short description; \
                      this tool returns the full schema you need before you can call them. \
                      \n• Use `query: \"select:name1,name2\"` to fetch exact tools by name.\
                      \n• Use a free-text query (e.g. `query: \"worktree\"`) to search the \
                      directory by keyword.\n\
                      Returns a JSON object `{ loaded: [...], tools: [...] }`. After a \
                      successful fetch the tool stays available for the rest of this task — \
                      you only need to look each one up once."
            .into(),
        parameters: serde_json::json!({
            "type": "object",
            "required": ["query"],
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Either `select:NAME[,NAME2]` for exact lookup, or \
                                     a free-text query searching tool names and descriptions."
                },
                "max_results": {
                    "type": "integer",
                    "description": "Cap on free-text query results. Default 5, max 20."
                }
            }
        }),
    }
}

pub fn build_deferred_tools_directory(all_tool_defs: &[ToolDef]) -> String {
    let deferred: Vec<ToolDef> = all_tool_defs
        .iter()
        .filter(|td| !is_always_on(&td.name))
        .cloned()
        .collect();
    directory_section(&deferred)
}

pub fn directory_section(deferred: &[ToolDef]) -> String {
    if deferred.is_empty() {
        return String::new();
    }
    let mut s = String::from(
        "\n## Deferred tools (use `tool_search` to load their schemas)\n\
         The following tools exist but their full JSON schemas are not in your \
         context to save tokens. To call any of them, first use `tool_search` to \
         fetch its schema, then invoke it normally. Names + short descriptions \
         below:\n\n",
    );
    for d in deferred {
        s.push_str(&directory_line(d));
        s.push('\n');
    }
    s.push_str(
        "\nDo not invent schemas — always run `tool_search` before invoking a \
         deferred tool. Tools loaded via `tool_search` remain available for the \
         rest of the task.\n",
    );
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn td(name: &str, desc: &str) -> ToolDef {
        ToolDef {
            name: name.into(),
            description: desc.into(),
            parameters: json!({"type": "object"}),
        }
    }

    #[test]
    fn directory_line_short_description_kept_intact() {
        let line = directory_line(&td("foo", "do a thing"));
        assert_eq!(line, "- `foo` — do a thing");
        assert!(!line.contains('…'));
    }

    #[test]
    fn directory_line_long_description_truncates_at_preview_chars() {
        let line = directory_line(&td(
            "long_tool",
            "this description is definitely longer than thirty characters by a lot",
        ));
        assert!(line.ends_with('…'), "expected ellipsis suffix, got: {}", line);
        let body_start = line.find("— ").unwrap() + "— ".len();
        let body_chars = line[body_start..].chars().count();
        assert!(
            (29..=32).contains(&body_chars),
            "body chars out of expected range: {} in `{}`",
            body_chars,
            line
        );
    }

    #[test]
    fn directory_line_replaces_newlines_with_spaces() {
        let line = directory_line(&td("bar", "first line\nsecond line"));
        assert!(!line.contains('\n'));
        assert!(line.contains("first line second line") || line.contains("first line…"));
    }

    #[test]
    fn directory_line_handles_multibyte_truncation_cleanly() {
        // Description with multi-byte chars right at the truncation boundary.
        // Must not panic (char_indices saves us) and must produce valid UTF-8.
        let line = directory_line(&td(
            "uni",
            "äöü äöü äöü äöü äöü äöü äöü äöü äöü äöü äöü",
        ));
        assert!(line.is_char_boundary(line.len()));
        assert!(line.starts_with("- `uni` — "));
    }

    #[test]
    fn always_on_includes_spec_eight() {
        for name in [
            "read_file",
            "edit_file",
            "grep_search",
            "glob",
            "run_command",
            "web_search",
            "todo_write",
            "ask_user",
            "tool_search",
            "goal_complete",
        ] {
            assert!(is_always_on(name), "expected `{}` to be always-on", name);
        }
    }

    #[test]
    fn always_on_excludes_known_deferred() {
        for name in [
            "enter_worktree",
            "find_symbol",
            "image_create",
            "spawn_subagent",
            "list_projects",
        ] {
            assert!(!is_always_on(name), "expected `{}` to be deferred", name);
        }
    }

    #[test]
    fn build_deferred_tools_directory_filters_out_always_on() {
        let pool = vec![
            td("read_file", "always-on"),
            td("enter_worktree", "deferred"),
            td("ask_user", "always-on"),
            td("find_symbol", "deferred"),
        ];
        let dir = build_deferred_tools_directory(&pool);
        assert!(dir.contains("enter_worktree"));
        assert!(dir.contains("find_symbol"));
        assert!(!dir.contains("`read_file`"));
        assert!(!dir.contains("`ask_user`"));
    }

    #[test]
    fn build_deferred_tools_directory_empty_when_all_always_on() {
        let pool = vec![
            td("read_file", "x"),
            td("ask_user", "y"),
            td("web_search", "z"),
        ];
        assert_eq!(build_deferred_tools_directory(&pool), "");
    }

    #[test]
    fn set_deferred_table_round_trips() {
        let original = deferred_table();
        let mine = vec![td("only_one", "desc")];
        set_deferred_table(mine.clone());
        let read_back = deferred_table();
        assert_eq!(read_back.len(), 1);
        assert_eq!(read_back[0].name, "only_one");
        set_deferred_table(original);
    }

    #[test]
    fn directory_section_empty_for_empty_input() {
        let out = directory_section(&[]);
        assert_eq!(out, "");
    }

    #[test]
    fn directory_section_has_header_and_footer_when_non_empty() {
        let pool = vec![td("foo", "do foo"), td("bar", "do bar")];
        let out = directory_section(&pool);
        assert!(out.contains("## Deferred tools"));
        assert!(out.contains("`foo`"));
        assert!(out.contains("`bar`"));
        assert!(out.contains("Do not invent schemas"));
    }

    #[test]
    fn directory_preserves_input_order() {
        let pool = vec![
            td("zebra_tool", "z"),
            td("alpha_tool", "a"),
            td("mango_tool", "m"),
        ];
        let dir = build_deferred_tools_directory(&pool);
        let z = dir.find("zebra_tool").unwrap();
        let a = dir.find("alpha_tool").unwrap();
        let m = dir.find("mango_tool").unwrap();
        assert!(z < a && a < m, "order broken: z={}, a={}, m={}", z, a, m);
    }
}

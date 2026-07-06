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
    "create_file",
    "edit_file",
    "list_directory",
    "grep_search",
    "glob",
    "run_command",
    "web_search",
    "todo_write",
    "ask_user",
    "tool_search",
];

pub fn is_always_on(name: &str) -> bool {
    ALWAYS_ON.contains(&name)
}

/// Edit distance between two strings. Standard DP, two rows, lowercase-
/// agnostic at the call site (callers normalize first). Used only for
/// suggesting near-matches on a tool_search miss — not in any hot path.
fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let m = a.len();
    let n = b.len();
    if m == 0 {
        return n;
    }
    if n == 0 {
        return m;
    }
    let mut prev: Vec<usize> = (0..=n).collect();
    let mut curr = vec![0_usize; n + 1];
    for i in 1..=m {
        curr[0] = i;
        for j in 1..=n {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            curr[j] = (curr[j - 1] + 1).min((prev[j] + 1).min(prev[j - 1] + cost));
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[n]
}

/// Return up to `max` deferred tool names that are "close enough" to `query`
/// by Levenshtein distance, ranked by smallest distance. The threshold
/// scales to the shorter side so that 3-char queries get tight matches but
/// long queries (e.g. `web_search_with_typo`) tolerate a few edits.
///
/// We also accept names that contain `query` as a substring even past the
/// edit-distance threshold — that catches "search" → "web_search" / "tool_search"
/// where the user typed a meaningful keyword that just isn't the full name.
fn fuzzy_suggest(query: &str, table: &[ToolDef], max: usize) -> Vec<String> {
    let q = query.to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    let mut scored: Vec<(usize, String)> = table
        .iter()
        .filter_map(|t| {
            let name_lc = t.name.to_lowercase();
            let dist = levenshtein(&q, &name_lc);
            let limit = (name_lc.len().min(q.len()) / 2).max(2);
            let is_substring = name_lc.contains(&q) || q.contains(&name_lc);
            if dist <= limit || is_substring {
                Some((dist, t.name.clone()))
            } else {
                None
            }
        })
        .collect();
    scored.sort_by_key(|(d, _)| *d);
    scored.into_iter().take(max).map(|(_, n)| n).collect()
}

/// Format suggestion list for inclusion in an error body. Returns empty
/// string when there are no suggestions, so callers can unconditionally
/// concatenate it without an extra branch.
fn suggestion_line(suggestions: &[String]) -> String {
    if suggestions.is_empty() {
        return String::new();
    }
    let joined = suggestions
        .iter()
        .map(|s| format!("`{}`", s))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "\nDid you mean: {}? Run tool_search again with one of these names \
         (e.g. `{{ \"query\": \"select:{}\" }}`) to load it.",
        joined, suggestions[0]
    )
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
            is_error: true,
            attachments: Vec::new(),
        });
    }

    let table = deferred_table(context);
    if table.is_empty() {
        return Ok(ToolOutput {
            content: "tool_search: no deferred tools are currently registered. The \
                      always-on set already includes everything available — call those \
                      tools directly."
                .into(),
            is_error: false,
            attachments: Vec::new(),
        });
    }

    let mut selected: Vec<ToolDef> = Vec::new();
    if let Some(rest) = query.strip_prefix("select:") {
        let mut unmatched: Vec<String> = Vec::new();
        for raw in rest.split(',') {
            let want = raw.trim();
            if want.is_empty() {
                continue;
            }
            if let Some(def) = table.iter().find(|t| t.name == want) {
                selected.push(def.clone());
            } else {
                unmatched.push(want.to_string());
            }
        }
        if selected.is_empty() {
            // Pool fuzzy suggestions across every unmatched name so the
            // model sees one consolidated list instead of N hint lines.
            // Dedup by name preserves rank since the per-query lists are
            // already distance-sorted.
            let mut pool: Vec<String> = Vec::new();
            for want in &unmatched {
                for s in fuzzy_suggest(want, &table, 3) {
                    if !pool.contains(&s) {
                        pool.push(s);
                    }
                }
            }
            pool.truncate(3);
            return Ok(ToolOutput {
                content: format!(
                    "tool_search: no deferred tool matched `{}`. Use a free-text query \
                     (e.g. `tool_search({{ \"query\": \"worktree\" }})`) to discover \
                     names first.{}",
                    query,
                    suggestion_line(&pool)
                ),
                is_error: true,
                attachments: Vec::new(),
            });
        }
    } else {
        let terms: Vec<String> = query.split_whitespace().map(|s| s.to_lowercase()).collect();
        let mut scored: Vec<(usize, &ToolDef)> = table
            .iter()
            .filter_map(|t| {
                let blob = format!("{} {}", t.name, t.description).to_lowercase();
                let hits: usize = terms.iter().map(|term| blob.matches(term).count()).sum();
                if hits == 0 {
                    None
                } else {
                    Some((hits, t))
                }
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0));
        for (_, t) in scored.into_iter().take(max_results) {
            selected.push(t.clone());
        }
        if selected.is_empty() {
            // Free-text query produced no substring hits. Fall back to a
            // Levenshtein-based suggestion: in practice this catches typos
            // ("filsystem" → "filesystem") and partial name guesses.
            let suggestions = fuzzy_suggest(&query, &table, 3);
            return Ok(ToolOutput {
                content: format!(
                    "tool_search: nothing matched `{}`. Try a different keyword or read \
                     the deferred-tool directory in your system prompt for available names.{}",
                    query,
                    suggestion_line(&suggestions)
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
        content: serde_json::to_string_pretty(&body).unwrap_or_else(|_| body.to_string()),
        is_error: false,
        attachments: Vec::new(),
    })
}

/// Snapshot the per-task deferred table from the context.
fn deferred_table(context: &ToolContext) -> Vec<ToolDef> {
    context
        .deferred_tools
        .lock()
        .map(|v| v.clone())
        .unwrap_or_default()
}

/// Publish this task's deferred tool set. The slot lives on the `ToolContext`
/// (one per task) — it was previously a process-global static, which let
/// concurrent tasks with different tool pools (e.g. different MCP servers)
/// overwrite each other's table between turns.
pub fn set_deferred_table(slot: &std::sync::Mutex<Vec<ToolDef>>, tools: Vec<ToolDef>) {
    if let Ok(mut guard) = slot.lock() {
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
        assert!(
            line.ends_with('…'),
            "expected ellipsis suffix, got: {}",
            line
        );
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
        let line = directory_line(&td("uni", "äöü äöü äöü äöü äöü äöü äöü äöü äöü äöü äöü"));
        assert!(line.is_char_boundary(line.len()));
        assert!(line.starts_with("- `uni` — "));
    }

    #[test]
    fn always_on_includes_spec_set() {
        for name in [
            "read_file",
            "create_file",
            "edit_file",
            "list_directory",
            "grep_search",
            "glob",
            "run_command",
            "web_search",
            "todo_write",
            "ask_user",
            "tool_search",
        ] {
            assert!(is_always_on(name), "expected `{}` to be always-on", name);
        }
    }

    #[test]
    fn always_on_excludes_known_deferred() {
        for name in [
            "find_symbol",
            "image_create",
            "spawn_subagent",
            "list_all_terminals",
        ] {
            assert!(!is_always_on(name), "expected `{}` to be deferred", name);
        }
    }

    #[test]
    fn build_deferred_tools_directory_filters_out_always_on() {
        let pool = vec![
            td("read_file", "always-on"),
            td("find_symbol", "deferred"),
            td("ask_user", "always-on"),
            td("image_create", "deferred"),
        ];
        let dir = build_deferred_tools_directory(&pool);
        assert!(dir.contains("find_symbol"));
        assert!(dir.contains("image_create"));
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
        let slot = std::sync::Mutex::new(Vec::new());
        let mine = vec![td("only_one", "desc")];
        set_deferred_table(&slot, mine.clone());
        let read_back = slot.lock().unwrap().clone();
        assert_eq!(read_back.len(), 1);
        assert_eq!(read_back[0].name, "only_one");
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

    #[test]
    fn fuzzy_suggest_catches_single_char_typo() {
        let pool = vec![
            td("find_symbol", ""),
            td("find_references", ""),
            td("image_create", ""),
        ];
        let s = fuzzy_suggest("find_symbl", &pool, 3);
        assert!(
            s.first().map(|x| x.as_str()) == Some("find_symbol"),
            "expected `find_symbol` first, got {:?}",
            s
        );
    }

    #[test]
    fn fuzzy_suggest_picks_up_substring_keyword() {
        // User typed a meaningful keyword that's a substring of a real name —
        // edit distance from "search" to "web_search" is high, but the
        // substring fallback should still surface it.
        let pool = vec![
            td("web_search", ""),
            td("image_create", ""),
            td("spawn_subagent", ""),
        ];
        let s = fuzzy_suggest("search", &pool, 3);
        assert!(
            s.iter().any(|x| x == "web_search"),
            "expected `web_search` in suggestions, got {:?}",
            s
        );
    }

    #[test]
    fn fuzzy_suggest_empty_query_returns_nothing() {
        let pool = vec![td("anything", "")];
        assert!(fuzzy_suggest("", &pool, 3).is_empty());
    }

    #[test]
    fn suggestion_line_empty_when_no_suggestions() {
        assert_eq!(suggestion_line(&[]), "");
    }

    #[test]
    fn suggestion_line_includes_first_in_select_hint() {
        let line = suggestion_line(&["find_symbol".into(), "find_references".into()]);
        assert!(line.contains("`find_symbol`"));
        assert!(line.contains("`find_references`"));
        assert!(line.contains("select:find_symbol"));
    }
}

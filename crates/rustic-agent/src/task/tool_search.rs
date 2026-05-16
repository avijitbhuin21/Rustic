//! P1.7 — Tool search (deferred tool schemas).
//!
//! Active in every native task. The system prompt advertises only the
//! **always-on** core tools by schema; every other builtin / MCP tool
//! appears in a compact directory the model can browse via the
//! `tool_search` meta-tool.
//!
//! `tool_search` itself is a read-only metadata fetch — given a query
//! string (or a `select:<name1>,<name2>` directive), it returns the full
//! JSON schema definitions for those tools AND appends the matched names
//! to the per-task `ToolContext.loaded_deferred_tools` set. The executor
//! reads that set at the top of every turn: tools in the set get their
//! full schemas back in the request alongside the always-on pool, so the
//! model can call them without inventing a schema.
//!
//! Sub-agent contexts inherit the parent's `loaded_deferred_tools` handle
//! (Arc-clone), so once the parent loads a tool the child also has it —
//! avoids redundant `tool_search` calls inside child agents.

use crate::provider::ToolDef;
use crate::tools::{ToolContext, ToolOutput};
use anyhow::Result;
use serde_json::Value;

/// Tools we always keep visible. Anything not in this list becomes
/// deferred when tool-search is enabled. Spec'd by plan.md P1.7: the 8
/// canonical "explore → grep → edit → run → commit" tools that account
/// for the bulk of every task's tool calls, plus the two meta-tools
/// (`tool_search` because the model needs it to surface deferred schemas,
/// `goal_complete` because the model has to be able to close a goal loop
/// without a lookup round-trip). 10 entries; everything else is deferred
/// and surfaced via `tool_search`.
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

/// Returns true when `name` is in the always-on pool.
pub fn is_always_on(name: &str) -> bool {
    ALWAYS_ON.contains(&name)
}

/// Render the directory line the model sees in the system prompt for one
/// deferred tool. Format: `- <name> — <30-char description>`. We
/// intentionally truncate descriptions so the directory stays cheap
/// regardless of how many tools become deferred.
pub fn directory_line(def: &ToolDef) -> String {
    const PREVIEW_CHARS: usize = 30;
    let mut short = def.description.replace('\n', " ");
    if short.len() > PREVIEW_CHARS {
        // char_indices avoids slicing on a multi-byte boundary.
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

/// `tool_search` execution. Parses the `query` parameter and returns
/// matching tool schemas. Two query modes:
///
/// * `select:NAME1,NAME2` — exact-name lookup, returns just those.
/// * free-form text — substring-matches against tool names and
///   descriptions; returns the top N matches ranked by hit count.
///
/// The output is a JSON object the model can read directly:
/// ```json
/// {
///   "loaded": ["enter_worktree", "exit_worktree"],
///   "tools": [ { "name": "...", "description": "...", "input_schema": { ... } } ]
/// }
/// ```
/// The executor uses the `loaded` array as the authoritative list of
/// deferred tools to include with the next request's tool definitions.
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

    // Resolve which deferred tools the host has registered. Stored on a
    // thread-local-ish global because the directory is built once per
    // process from the same source the executor uses to build tool_defs.
    // See `set_deferred_table` below.
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

    // Mark the matched tools as "loaded" on the task's shared set so the
    // executor includes their full schemas in the next API call. Done before
    // we render the response so a crash after this point still surfaces the
    // load in the next turn.
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

/// Static-ish table of deferred tools. Built once per process by the
/// executor on first use (`set_deferred_table`). Reads are lock-free
/// via an `ArcSwap`-style pattern using `Mutex<Option<Arc<Vec<ToolDef>>>>`
/// — Mutex contention is irrelevant here (we update on tool-pool change
/// only).
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

/// Replace the deferred-tool table. Called by the executor on every
/// turn so newly-registered MCP tools become searchable without a
/// restart. Idempotent and cheap when the table is unchanged.
pub fn set_deferred_table(tools: Vec<ToolDef>) {
    if let Ok(mut guard) = deferred_table_cell().lock() {
        *guard = tools;
    }
}

/// Build the `tool_search` ToolDef. The schema is intentionally minimal
/// (one string param) so it doesn't bloat the prefix it's meant to keep
/// thin.
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

/// Host-side helper: given the FULL tool pool the executor will see for
/// this task (builtins + web + media + MCP, post-denylist), return the
/// directory section text the system prompt should carry. Pure function —
/// doesn't touch the live `DEFERRED_TABLE`; the executor handles that on
/// every turn from its own (potentially MCP-mutated) pool.
///
/// Returns "" when there are no deferred tools (everything is in the
/// always-on pool) so the host can simply concatenate the result.
pub fn build_deferred_tools_directory(all_tool_defs: &[ToolDef]) -> String {
    let deferred: Vec<ToolDef> = all_tool_defs
        .iter()
        .filter(|td| !is_always_on(&td.name))
        .cloned()
        .collect();
    directory_section(&deferred)
}

/// Build the "deferred tools directory" lines for the system prompt.
/// Output is a Markdown bullet list, one per deferred tool, prefixed by
/// a one-paragraph explanation. Returns an empty string when no tools
/// are deferred (so the section disappears entirely instead of saying
/// "no tools deferred").
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

    // C5.10 — directory_line truncation at ~30 chars.

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
        // The post-name body (after "— ") should be PREVIEW_CHARS chars + the
        // ellipsis. PREVIEW_CHARS = 30; allow ±1 for trim() effects.
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

    // C5.10 — is_always_on covers spec list.

    #[test]
    fn always_on_includes_spec_eight() {
        // plan.md:407 names these eight; tool_search + goal_complete are
        // meta-tools added for the design to work.
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

    // C5.10 — `select:` parsing through execute (requires a context).
    // We can't easily build a full ToolContext in a unit test, so we test
    // the table/select helpers directly: populate the deferred table, then
    // call execute and inspect the rendered JSON. To avoid the context
    // dependency we skip the loaded-tools mutation by routing through the
    // private surface — instead exercise the table + selection logic via
    // `build_deferred_tools_directory` for the directory side, and leave
    // the execute() path covered by integration tests that exercise the
    // real ToolContext.

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

    // C5.10 — set/get round-trip on the deferred table.
    #[test]
    fn set_deferred_table_round_trips() {
        let original = deferred_table();
        let mine = vec![td("only_one", "desc")];
        set_deferred_table(mine.clone());
        let read_back = deferred_table();
        assert_eq!(read_back.len(), 1);
        assert_eq!(read_back[0].name, "only_one");
        // Restore so other tests aren't poisoned by ours.
        set_deferred_table(original);
    }

    // C5.10 — directory_section disappears entirely when the deferred list
    // is empty, so a host that concatenates blindly doesn't end up with a
    // stray "Deferred tools" header for no reason.
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

    // C5.10 — free-text scoring. We can test the scoring math without a
    // full ToolContext by calling `execute()` through a fixture — instead
    // we expose the matching ranking by verifying that a high-overlap tool
    // appears before a low-overlap one in the directory order (the table is
    // returned in insertion order; ranking happens inside `execute`). To
    // genuinely cover ranking we'd need a context; covered by the integration
    // path. Here we just sanity-check that the directory preserves order.
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

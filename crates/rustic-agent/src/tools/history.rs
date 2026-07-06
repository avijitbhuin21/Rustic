//! `search_history` / `read_history` — query conversation turns that context
//! condensing dropped from the live window. The host archives every dropped
//! message verbatim (see `ConversationArchive`); these tools grep that archive
//! and read individual archived messages back.

use super::{ToolContext, ToolOutput};
use crate::provider::{ContentBlock, ToolDef};
use anyhow::Result;
use serde_json::{json, Value};

const SNIPPET_RADIUS: usize = 160;
const DEFAULT_MAX_RESULTS: usize = 8;
const MAX_RESULTS_CAP: usize = 25;
const SNIPPETS_PER_MESSAGE: usize = 2;
const READ_HEAD_BYTES: usize = 4 * 1024;
const READ_TAIL_BYTES: usize = 12 * 1024;

pub fn definitions() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "search_history".into(),
            description: "Regex-search conversation turns that were condensed out of your \
                          context window. When history is condensed, the dropped messages \
                          are archived verbatim — use this to recover exact details \
                          (identifiers, error messages, file contents, earlier decisions) \
                          that the condense summary lost. Returns snippets with \
                          (generation, slot) references; follow up with read_history for \
                          the full message."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Regex pattern (case-insensitive) to search for"
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "Max matching messages to return (default 8, cap 25)"
                    }
                },
                "required": ["query"]
            }),
        },
        ToolDef {
            name: "read_history".into(),
            description: "Read one archived (condensed-away) conversation message in full, \
                          by the (generation, slot) reference returned from search_history."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "generation": {
                        "type": "integer",
                        "description": "Condense generation from the search_history result"
                    },
                    "slot": {
                        "type": "integer",
                        "description": "Message slot within the generation"
                    }
                },
                "required": ["generation", "slot"]
            }),
        },
    ]
}

pub async fn execute(name: &str, params: Value, context: &ToolContext) -> Result<ToolOutput> {
    let Some(archive) = context.conversation_archive.as_ref() else {
        return Ok(error_output(
            "Conversation archive is not available in this context (sub-agents and embedded \
             sessions don't archive condensed history).",
        ));
    };
    let rows = archive.fetch_all(&context.task_id);
    if rows.is_empty() {
        return Ok(ToolOutput {
            content: "The conversation archive is empty — nothing has been condensed away in \
                      this task yet, so the full history is already in your context."
                .into(),
            is_error: false,
            attachments: Vec::new(),
        });
    }
    match name {
        "search_history" => search_history(params, rows),
        "read_history" => read_history(params, rows),
        _ => Ok(error_output(&format!("Unknown history tool: {}", name))),
    }
}

fn search_history(params: Value, rows: Vec<super::ArchivedMessage>) -> Result<ToolOutput> {
    let Some(query) = params
        .get("query")
        .and_then(|v| v.as_str())
        .filter(|q| !q.is_empty())
    else {
        return Ok(error_output("query is required"));
    };
    let max_results = params
        .get("max_results")
        .and_then(|v| v.as_u64())
        .map(|v| (v as usize).clamp(1, MAX_RESULTS_CAP))
        .unwrap_or(DEFAULT_MAX_RESULTS);

    let re = match regex::RegexBuilder::new(query)
        .case_insensitive(true)
        .size_limit(1 << 20)
        .build()
    {
        Ok(re) => re,
        Err(e) => return Ok(error_output(&format!("Invalid regex: {}", e))),
    };

    let generations = rows.iter().map(|r| r.generation).max().unwrap_or(0);
    let total_messages = rows.len();
    let mut matched_messages = 0usize;
    let mut lines: Vec<String> = Vec::new();

    for row in &rows {
        let segments = searchable_segments(&row.content_json);
        let mut snippets: Vec<String> = Vec::new();
        for (kind, text) in &segments {
            for m in re.find_iter(text) {
                if snippets.len() >= SNIPPETS_PER_MESSAGE {
                    break;
                }
                snippets.push(format!(
                    "({}) …{}…",
                    kind,
                    snippet_around(text, m.start(), m.end())
                ));
            }
            if snippets.len() >= SNIPPETS_PER_MESSAGE {
                break;
            }
        }
        if snippets.is_empty() {
            continue;
        }
        matched_messages += 1;
        if matched_messages <= max_results {
            lines.push(format!(
                "[gen {} slot {}] {}: {}",
                row.generation,
                row.slot,
                row.role,
                snippets.join(" | ")
            ));
        }
    }

    if matched_messages == 0 {
        return Ok(ToolOutput {
            content: format!(
                "No matches for /{}/ in the archive ({} archived messages, {} condense generation(s)).",
                query, total_messages, generations
            ),
            is_error: false,
            attachments: Vec::new(),
        });
    }

    let mut out = format!(
        "{} archived message(s) match /{}/ ({} archived total, {} condense generation(s)){}:\n\n",
        matched_messages,
        query,
        total_messages,
        generations,
        if matched_messages > max_results {
            format!(" — showing first {}", max_results)
        } else {
            String::new()
        }
    );
    out.push_str(&lines.join("\n"));
    out.push_str("\n\nUse read_history {generation, slot} to read a full message.");
    Ok(ToolOutput {
        content: out,
        is_error: false,
        attachments: Vec::new(),
    })
}

fn read_history(params: Value, rows: Vec<super::ArchivedMessage>) -> Result<ToolOutput> {
    let (Some(generation), Some(slot)) = (
        params.get("generation").and_then(|v| v.as_i64()),
        params.get("slot").and_then(|v| v.as_i64()),
    ) else {
        return Ok(error_output("generation and slot are required integers"));
    };
    let Some(row) = rows
        .iter()
        .find(|r| r.generation == generation && r.slot == slot)
    else {
        return Ok(error_output(&format!(
            "No archived message at gen {} slot {} — use search_history to find valid references.",
            generation, slot
        )));
    };

    let mut body = String::new();
    for (kind, text) in searchable_segments(&row.content_json) {
        body.push_str(&format!("--- {} ---\n{}\n", kind, text));
    }
    let body = truncate_head_tail_bytes(&body, READ_HEAD_BYTES, READ_TAIL_BYTES);
    Ok(ToolOutput {
        content: format!(
            "[gen {} slot {}] role={}\n{}",
            generation, slot, row.role, body
        ),
        is_error: false,
        attachments: Vec::new(),
    })
}

/// Flatten a message's content_json into searchable/printable (kind, text) segments.
fn searchable_segments(content_json: &str) -> Vec<(String, String)> {
    let Ok(blocks) = serde_json::from_str::<Vec<ContentBlock>>(content_json) else {
        return vec![("raw".into(), content_json.to_string())];
    };
    let mut segments = Vec::new();
    for block in blocks {
        match block {
            ContentBlock::Text { text } => segments.push(("text".into(), text)),
            ContentBlock::ToolUse { name, input, .. } => segments.push((
                format!("tool_use {}", name),
                serde_json::to_string(&input).unwrap_or_default(),
            )),
            ContentBlock::ToolResult {
                content, is_error, ..
            } => segments.push((
                if is_error {
                    "tool_result (error)".into()
                } else {
                    "tool_result".into()
                },
                content,
            )),
            ContentBlock::Thinking { thinking, .. } => segments.push(("thinking".into(), thinking)),
            ContentBlock::RedactedThinking { .. }
            | ContentBlock::Image { .. }
            | ContentBlock::ModelSwitch { .. } => {}
        }
    }
    segments
}

/// Extract a char-boundary-safe snippet of ±SNIPPET_RADIUS bytes around a match.
fn snippet_around(text: &str, start: usize, end: usize) -> String {
    let from = floor_boundary(text, start.saturating_sub(SNIPPET_RADIUS));
    let to = ceil_boundary(text, (end + SNIPPET_RADIUS).min(text.len()));
    text[from..to].replace('\n', " ")
}

/// Keep the first `head` and last `tail` bytes of `s`, char-boundary safe.
fn truncate_head_tail_bytes(s: &str, head: usize, tail: usize) -> String {
    if s.len() <= head + tail {
        return s.to_string();
    }
    let head_end = floor_boundary(s, head);
    let tail_start = ceil_boundary(s, s.len() - tail);
    format!(
        "{}\n\n…[{} bytes omitted from the middle of this archived message]…\n\n{}",
        &s[..head_end],
        tail_start - head_end,
        &s[tail_start..]
    )
}

fn floor_boundary(s: &str, mut i: usize) -> usize {
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn ceil_boundary(s: &str, mut i: usize) -> usize {
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

fn error_output(msg: &str) -> ToolOutput {
    ToolOutput {
        content: msg.into(),
        is_error: true,
        attachments: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippet_and_truncation_are_char_boundary_safe_on_multibyte_text() {
        let text = "héllo wörld — ".repeat(2000);
        let s = snippet_around(&text, 3, 8);
        assert!(!s.is_empty());
        let t = truncate_head_tail_bytes(&text, 100, 100);
        assert!(t.contains("bytes omitted"));
        let small = truncate_head_tail_bytes("short", 100, 100);
        assert_eq!(small, "short");
    }

    #[test]
    fn searchable_segments_flattens_blocks_and_survives_bad_json() {
        let json = r#"[{"type":"text","text":"hello"},{"type":"tool_use","id":"1","name":"read_file","input":{"path":"a.rs"}},{"type":"tool_result","tool_use_id":"1","content":"body","is_error":false}]"#;
        let segs = searchable_segments(json);
        assert_eq!(segs.len(), 3);
        assert_eq!(segs[0].1, "hello");
        assert!(segs[1].0.contains("read_file"));
        assert_eq!(searchable_segments("garbage")[0].0, "raw");
    }
}

//! `edit_notebook` — cell-aware editing for Jupyter `.ipynb` files.
//!
//! Contract (wired in `tools/mod.rs`):
//!   - `pub fn definitions() -> Vec<ToolDef>` — the tool's schema.
//!   - `pub async fn execute(params: Value, context: &ToolContext) -> Result<ToolOutput>`
//!
//! Cell addressing is 1-indexed, matching `read_file`'s `cells: "1-10"`
//! notebook reads (see `file_ops::read_notebook`), so read and edit agree.
//!
//! The write pipeline mirrors `edit_file` in `file_ops.rs`: write-scope check,
//! sensitive-path check, permission approval, pre-write history tracking,
//! per-file lock, atomic write, then index refresh + read-registry
//! invalidation.

use crate::provider::ToolDef;
use crate::task::permissions::PermissionLevel;
use crate::task::PermissionOp;
use crate::tools::file_ops::{
    check_sensitive_path, check_write_scope, maybe_emit_memory_updated, refresh_index_after_write,
    resolve_within_project, track_before_write,
};
use crate::tools::{ToolContext, ToolOutput};
use anyhow::Result;
use serde_json::{json, Value};

pub fn definitions() -> Vec<ToolDef> {
    vec![ToolDef {
        name: "edit_notebook".into(),
        description: "Edit a Jupyter notebook (`.ipynb`) at the CELL level — never hand-edit \
                      notebook JSON with edit_file. Cells are 1-indexed, matching read_file's \
                      `cells: \"1-10\"` addressing, so what you read is what you edit.\n\
                      Modes:\n\
                      • `replace` — overwrite cell N's source (its cell_type and metadata are \
                        kept; for code cells, outputs and execution_count are cleared so stale \
                        results never survive an edit).\n\
                      • `insert_before` / `insert_after` — insert a NEW cell relative to \
                        existing cell N. To append at the end, use `insert_before` with \
                        `cell` = total+1 (or `insert_after` with `cell` = total).\n\
                      • `delete` — remove cell N (deleting the last remaining cell is allowed).\n\
                      `source` is the full new cell source and is required for replace/insert \
                      modes. `cell_type` (default \"code\") applies to inserts only. \
                      All other notebook structure (nbformat, metadata, other cells) is \
                      preserved."
            .into(),
        parameters: json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Relative path from project root; must end in .ipynb"
                },
                "mode": {
                    "type": "string",
                    "enum": ["replace", "insert_before", "insert_after", "delete"],
                    "description": "The edit operation to perform."
                },
                "cell": {
                    "type": "integer",
                    "description": "Target cell number, 1-indexed (same numbering read_file \
                                    shows). For `insert_before`, total+1 is also accepted and \
                                    appends at the end."
                },
                "source": {
                    "type": "string",
                    "description": "Full new cell source. Required for replace/insert_before/\
                                    insert_after; ignored for delete."
                },
                "cell_type": {
                    "type": "string",
                    "enum": ["code", "markdown", "raw"],
                    "description": "Cell type for INSERTED cells only (default \"code\"). \
                                    Replace keeps the existing cell's type."
                }
            },
            "required": ["path", "mode", "cell"]
        }),
    }]
}

/// Edit, insert, or delete a cell in a Jupyter notebook.
pub async fn execute(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    let path = params["path"].as_str().unwrap_or("");

    if context.permissions() == PermissionLevel::Chat {
        return Ok(ToolOutput::text(
            "PERMISSION_DENIED: File writes are not allowed in Chat mode.",
            true,
        ));
    }
    if path.is_empty() {
        return Ok(ToolOutput::text("path is required", true));
    }
    if !path.to_ascii_lowercase().ends_with(".ipynb") {
        return Ok(ToolOutput::text(
            format!(
                "NOT_A_NOTEBOOK: '{}' does not end in .ipynb. edit_notebook only edits \
                 Jupyter notebooks — use edit_file for other file types.",
                path
            ),
            true,
        ));
    }

    if let Some(scope_violation) = check_write_scope(context, path) {
        return Ok(scope_violation);
    }
    let full_path = match resolve_within_project(&context.project_root, path) {
        Ok(p) => p,
        Err(violation) => return Ok(violation),
    };
    if let Some(blocked) = check_sensitive_path(path, &full_path, context).await {
        return Ok(blocked);
    }

    // Parse and validate the edit parameters BEFORE prompting the user for
    // approval — a malformed call shouldn't cost a permission dialog.
    let mode = match params["mode"].as_str().and_then(parse_mode) {
        Some(m) => m,
        None => {
            return Ok(ToolOutput::text(
                "mode is required and must be one of: replace, insert_before, \
                 insert_after, delete",
                true,
            ));
        }
    };
    let cell = match int_param(&params, "cell") {
        Some(n) if n >= 1 => n as usize,
        Some(_) => {
            return Ok(ToolOutput::text(
                "cell must be >= 1 (cells are 1-indexed, matching read_file's notebook \
                 numbering)",
                true,
            ));
        }
        None => {
            return Ok(ToolOutput::text(
                "cell is required (1-indexed integer)",
                true,
            ))
        }
    };
    let source = params["source"].as_str().map(|s| s.to_string());
    if mode != EditMode::Delete && source.is_none() {
        return Ok(ToolOutput::text(
            format!("source is required for mode '{}'", mode.as_str()),
            true,
        ));
    }
    let cell_type = match params["cell_type"].as_str() {
        None => "code",
        Some("code") => "code",
        Some("markdown") => "markdown",
        Some("raw") => "raw",
        Some(other) => {
            return Ok(ToolOutput::text(
                format!(
                    "cell_type must be \"code\", \"markdown\", or \"raw\", got \"{}\"",
                    other
                ),
                true,
            ));
        }
    };

    if context.needs_write_approval() {
        let approved = context
            .permission_broker
            .request(
                &context.event_tx,
                &context.task_id,
                PermissionOp::WriteFile(path.to_string()),
            )
            .await;
        if !approved {
            return Ok(ToolOutput::text(
                "PERMISSION_DENIED: User denied notebook edit.",
                true,
            ));
        }
    }

    // Read without the mutex (mirrors edit_file: the lock is held only for
    // the write so a slow read can't starve concurrent edits).
    let raw = match std::fs::read_to_string(&full_path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ToolOutput::text(
                format!(
                    "CONTENT_DELETED: File '{}' does not exist. It may have been deleted.",
                    path
                ),
                true,
            ));
        }
        Err(e) => {
            return Ok(ToolOutput::text(
                format!("Error reading notebook: {}", e),
                true,
            ));
        }
    };
    let mut nb: Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            return Ok(ToolOutput::text(
                format!(
                    "NOTEBOOK_PARSE_ERROR: '{}' isn't valid JSON: {}. .ipynb files must be \
                     parseable as JSON.",
                    path, e
                ),
                true,
            ));
        }
    };

    let outcome = match apply_edit(&mut nb, mode, cell, source.as_deref(), cell_type) {
        Ok(o) => o,
        Err(msg) => return Ok(ToolOutput::text(msg, true)),
    };

    let mut serialized = match serde_json::to_string_pretty(&nb) {
        Ok(s) => s,
        Err(e) => {
            return Ok(ToolOutput::text(
                format!("Error serializing notebook: {}", e),
                true,
            ));
        }
    };
    if !serialized.ends_with('\n') {
        serialized.push('\n');
    }

    track_before_write(context, &full_path);
    let _guard = match context.file_lock.acquire(&full_path).await {
        Ok(g) => g,
        Err(msg) => return Ok(ToolOutput::text(msg, true)),
    };
    match crate::io_util::atomic_write(&full_path, serialized.as_bytes()) {
        Ok(()) => {
            maybe_emit_memory_updated(path, context);
            refresh_index_after_write(context, &full_path);
            // Cell numbering shifted (insert/delete) or content changed
            // (replace) — earlier cell-range read coverage is stale either way.
            context.file_read_registry.invalidate(&full_path);

            let source_note = match &outcome.source_preview {
                Some(p) => format!(" | source: {}", p),
                None => String::new(),
            };
            Ok(ToolOutput::text(
                format!(
                    "Notebook '{}' edited: {} cell {} [{}]{} — notebook now has {} cell(s).",
                    path,
                    outcome.verb,
                    outcome.cell_number,
                    outcome.cell_type,
                    source_note,
                    outcome.new_total,
                ),
                false,
            ))
        }
        Err(e) => Ok(ToolOutput::text(
            format!("Error writing notebook: {}", e),
            true,
        )),
    }
}

// ─── Pure transform logic (unit-testable without a ToolContext) ─────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditMode {
    Replace,
    InsertBefore,
    InsertAfter,
    Delete,
}

impl EditMode {
    fn as_str(self) -> &'static str {
        match self {
            EditMode::Replace => "replace",
            EditMode::InsertBefore => "insert_before",
            EditMode::InsertAfter => "insert_after",
            EditMode::Delete => "delete",
        }
    }
}

fn parse_mode(s: &str) -> Option<EditMode> {
    match s {
        "replace" => Some(EditMode::Replace),
        "insert_before" => Some(EditMode::InsertBefore),
        "insert_after" => Some(EditMode::InsertAfter),
        "delete" => Some(EditMode::Delete),
        _ => None,
    }
}

/// Integer param tolerant of models stringifying numbers (`"3"`).
fn int_param(params: &Value, key: &str) -> Option<i64> {
    match params.get(key) {
        Some(Value::Number(n)) => n.as_i64(),
        Some(Value::String(s)) => s.trim().parse::<i64>().ok(),
        _ => None,
    }
}

/// Result of a successful in-memory edit, used to build the tool output.
#[derive(Debug)]
struct EditOutcome {
    verb: &'static str,
    cell_number: usize,
    cell_type: String,
    source_preview: Option<String>,
    new_total: usize,
}

/// Split a source string into nbformat's standard list-of-lines form: each
/// element keeps its trailing `\n` except (possibly) the last. Empty source
/// yields an empty list.
fn source_to_lines(source: &str) -> Vec<Value> {
    source
        .split_inclusive('\n')
        .map(|line| Value::String(line.to_string()))
        .collect()
}

/// Build a brand-new cell object. Code cells get empty `outputs` and null
/// `execution_count`; markdown and raw cells get neither field (per nbformat).
fn build_cell(cell_type: &str, source: &str) -> Value {
    let mut cell = json!({
        "cell_type": cell_type,
        "metadata": {},
        "source": source_to_lines(source),
    });
    if cell_type == "code" {
        cell["outputs"] = json!([]);
        cell["execution_count"] = Value::Null;
    }
    cell
}

/// First ~80 chars of the source, newlines flattened, for the result message.
fn source_preview(source: &str) -> String {
    let flat = source.replace('\n', "\\n");
    let mut preview: String = flat.chars().take(80).collect();
    if flat.chars().count() > 80 {
        preview.push('…');
    }
    preview
}

/// Apply one edit to the notebook JSON in place. `cell` is 1-indexed.
/// Everything not touched (nbformat, metadata, other cells, cell ids) is
/// preserved. Returns a summary of what changed, or a clear error message.
fn apply_edit(
    nb: &mut Value,
    mode: EditMode,
    cell: usize,
    source: Option<&str>,
    insert_cell_type: &str,
) -> std::result::Result<EditOutcome, String> {
    let cells = match nb.get_mut("cells").and_then(|c| c.as_array_mut()) {
        Some(a) => a,
        None => {
            return Err(
                "NOTEBOOK_SHAPE_ERROR: no top-level `cells` array. This may not be a \
                 notebook file, or it's saved in an unsupported nbformat."
                    .into(),
            );
        }
    };
    let total = cells.len();

    // Bounds: replace/delete/insert_after target an EXISTING cell (1..=total);
    // insert_before additionally accepts total+1 to append at the end.
    let max_allowed = match mode {
        EditMode::InsertBefore => total + 1,
        _ => total,
    };
    if cell > max_allowed || (mode != EditMode::InsertBefore && total == 0) {
        return Err(format!(
            "CELL_OUT_OF_RANGE: cell {} is out of range for mode '{}' — the notebook has \
             {} cell(s) (valid: 1-{}{}). Cells are 1-indexed, matching read_file's notebook \
             numbering.",
            cell,
            mode.as_str(),
            total,
            max_allowed.max(1),
            if mode == EditMode::InsertBefore {
                "; total+1 appends at the end"
            } else {
                ""
            },
        ));
    }
    let idx = cell - 1;

    match mode {
        EditMode::Replace => {
            let src = source.ok_or("source is required for mode 'replace'")?;
            let existing = &mut cells[idx];
            let cell_type = existing
                .get("cell_type")
                .and_then(|v| v.as_str())
                .unwrap_or("code")
                .to_string();
            existing["source"] = Value::Array(source_to_lines(src));
            if cell_type == "code" {
                // Stale outputs must never survive an edit.
                existing["outputs"] = json!([]);
                existing["execution_count"] = Value::Null;
            }
            Ok(EditOutcome {
                verb: "replaced",
                cell_number: cell,
                cell_type,
                source_preview: Some(source_preview(src)),
                new_total: cells.len(),
            })
        }
        EditMode::InsertBefore | EditMode::InsertAfter => {
            let src =
                source.ok_or_else(|| format!("source is required for mode '{}'", mode.as_str()))?;
            let insert_at = if mode == EditMode::InsertBefore {
                idx
            } else {
                idx + 1
            };
            cells.insert(insert_at, build_cell(insert_cell_type, src));
            Ok(EditOutcome {
                verb: if mode == EditMode::InsertBefore {
                    "inserted new cell before"
                } else {
                    "inserted new cell after"
                },
                cell_number: cell,
                cell_type: insert_cell_type.to_string(),
                source_preview: Some(source_preview(src)),
                new_total: cells.len(),
            })
        }
        EditMode::Delete => {
            let removed = cells.remove(idx);
            let cell_type = removed
                .get("cell_type")
                .and_then(|v| v.as_str())
                .unwrap_or("code")
                .to_string();
            Ok(EditOutcome {
                verb: "deleted",
                cell_number: cell,
                cell_type,
                source_preview: None,
                new_total: cells.len(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notebook(cells: Vec<Value>) -> Value {
        json!({
            "cells": cells,
            "metadata": { "kernelspec": { "name": "python3" } },
            "nbformat": 4,
            "nbformat_minor": 5
        })
    }

    fn code_cell_with_output(src: &str) -> Value {
        json!({
            "cell_type": "code",
            "metadata": { "tags": ["keep-me"] },
            "source": source_to_lines(src),
            "outputs": [ { "output_type": "stream", "name": "stdout", "text": ["hi\n"] } ],
            "execution_count": 7
        })
    }

    fn md_cell(src: &str) -> Value {
        json!({
            "cell_type": "markdown",
            "metadata": {},
            "source": source_to_lines(src)
        })
    }

    fn sources(nb: &Value) -> Vec<String> {
        nb["cells"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| {
                c["source"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|l| l.as_str().unwrap())
                    .collect::<String>()
            })
            .collect()
    }

    // ── source_to_lines ──────────────────────────────────────────────────

    #[test]
    fn source_lines_multiline_with_trailing_newline() {
        let lines = source_to_lines("a = 1\nb = 2\n");
        assert_eq!(
            lines,
            vec![
                Value::String("a = 1\n".into()),
                Value::String("b = 2\n".into())
            ]
        );
    }

    #[test]
    fn source_lines_no_trailing_newline() {
        let lines = source_to_lines("a = 1\nb = 2");
        assert_eq!(
            lines,
            vec![
                Value::String("a = 1\n".into()),
                Value::String("b = 2".into())
            ]
        );
    }

    #[test]
    fn source_lines_empty_is_empty_list() {
        assert!(source_to_lines("").is_empty());
    }

    #[test]
    fn source_lines_blank_lines_preserved() {
        let lines = source_to_lines("a\n\nb\n");
        assert_eq!(
            lines,
            vec![
                Value::String("a\n".into()),
                Value::String("\n".into()),
                Value::String("b\n".into())
            ]
        );
    }

    // ── replace ──────────────────────────────────────────────────────────

    #[test]
    fn replace_clears_outputs_and_execution_count() {
        let mut nb = notebook(vec![code_cell_with_output("old()\n")]);
        let out = apply_edit(&mut nb, EditMode::Replace, 1, Some("new()\n"), "code").unwrap();
        assert_eq!(out.verb, "replaced");
        assert_eq!(out.cell_type, "code");
        assert_eq!(out.new_total, 1);
        let cell = &nb["cells"][0];
        assert_eq!(cell["outputs"], json!([]));
        assert_eq!(cell["execution_count"], Value::Null);
        assert_eq!(sources(&nb), vec!["new()\n"]);
        // Untouched structure survives: metadata on the cell and notebook.
        assert_eq!(cell["metadata"]["tags"], json!(["keep-me"]));
        assert_eq!(nb["nbformat"], json!(4));
        assert_eq!(nb["metadata"]["kernelspec"]["name"], json!("python3"));
    }

    #[test]
    fn replace_keeps_existing_cell_type() {
        let mut nb = notebook(vec![md_cell("# Old\n")]);
        let out = apply_edit(&mut nb, EditMode::Replace, 1, Some("# New\n"), "code").unwrap();
        // cell_type param is insert-only; replace keeps markdown.
        assert_eq!(out.cell_type, "markdown");
        assert_eq!(nb["cells"][0]["cell_type"], json!("markdown"));
        // Markdown cells don't gain outputs/execution_count on replace.
        assert!(nb["cells"][0].get("outputs").is_none());
        assert!(nb["cells"][0].get("execution_count").is_none());
    }

    // ── insert ───────────────────────────────────────────────────────────

    #[test]
    fn insert_before_positions_correctly() {
        let mut nb = notebook(vec![md_cell("one\n"), md_cell("two\n")]);
        let out = apply_edit(&mut nb, EditMode::InsertBefore, 2, Some("mid\n"), "code").unwrap();
        assert_eq!(out.new_total, 3);
        assert_eq!(sources(&nb), vec!["one\n", "mid\n", "two\n"]);
    }

    #[test]
    fn insert_after_positions_correctly() {
        let mut nb = notebook(vec![md_cell("one\n"), md_cell("two\n")]);
        let out = apply_edit(&mut nb, EditMode::InsertAfter, 1, Some("mid\n"), "code").unwrap();
        assert_eq!(out.new_total, 3);
        assert_eq!(sources(&nb), vec!["one\n", "mid\n", "two\n"]);
    }

    #[test]
    fn append_via_insert_before_total_plus_one() {
        let mut nb = notebook(vec![md_cell("one\n"), md_cell("two\n")]);
        let out = apply_edit(&mut nb, EditMode::InsertBefore, 3, Some("end\n"), "code").unwrap();
        assert_eq!(out.new_total, 3);
        assert_eq!(sources(&nb), vec!["one\n", "two\n", "end\n"]);
    }

    #[test]
    fn insert_before_into_empty_notebook() {
        let mut nb = notebook(vec![]);
        let out = apply_edit(&mut nb, EditMode::InsertBefore, 1, Some("first\n"), "code").unwrap();
        assert_eq!(out.new_total, 1);
        assert_eq!(sources(&nb), vec!["first\n"]);
    }

    #[test]
    fn inserted_code_cell_has_empty_outputs_and_null_count() {
        let mut nb = notebook(vec![md_cell("one\n")]);
        apply_edit(&mut nb, EditMode::InsertAfter, 1, Some("x = 1\n"), "code").unwrap();
        let cell = &nb["cells"][1];
        assert_eq!(cell["cell_type"], json!("code"));
        assert_eq!(cell["outputs"], json!([]));
        assert_eq!(cell["execution_count"], Value::Null);
    }

    #[test]
    fn inserted_raw_cell_has_no_outputs_field() {
        let mut nb = notebook(vec![code_cell_with_output("a"), md_cell("b")]);
        apply_edit(&mut nb, EditMode::InsertAfter, 1, Some("%%latex\n"), "raw").unwrap();
        let cell = &nb["cells"][1];
        assert_eq!(cell["cell_type"], json!("raw"));
        assert!(cell.get("outputs").is_none());
        assert!(cell.get("execution_count").is_none());
    }

    #[test]
    fn inserted_markdown_cell_has_no_outputs_field() {
        let mut nb = notebook(vec![md_cell("one\n")]);
        apply_edit(
            &mut nb,
            EditMode::InsertAfter,
            1,
            Some("# Title\n"),
            "markdown",
        )
        .unwrap();
        let cell = &nb["cells"][1];
        assert_eq!(cell["cell_type"], json!("markdown"));
        assert!(cell.get("outputs").is_none());
        assert!(cell.get("execution_count").is_none());
    }

    // ── delete ───────────────────────────────────────────────────────────

    #[test]
    fn delete_removes_the_right_cell() {
        let mut nb = notebook(vec![md_cell("one\n"), md_cell("two\n"), md_cell("three\n")]);
        let out = apply_edit(&mut nb, EditMode::Delete, 2, None, "code").unwrap();
        assert_eq!(out.verb, "deleted");
        assert_eq!(out.new_total, 2);
        assert_eq!(sources(&nb), vec!["one\n", "three\n"]);
    }

    #[test]
    fn delete_last_remaining_cell_leaves_empty_notebook() {
        let mut nb = notebook(vec![code_cell_with_output("only\n")]);
        let out = apply_edit(&mut nb, EditMode::Delete, 1, None, "code").unwrap();
        assert_eq!(out.new_total, 0);
        assert!(nb["cells"].as_array().unwrap().is_empty());
    }

    // ── bounds / shape errors ────────────────────────────────────────────

    #[test]
    fn replace_out_of_range_reports_cell_count() {
        let mut nb = notebook(vec![md_cell("one\n")]);
        let err = apply_edit(&mut nb, EditMode::Replace, 5, Some("x\n"), "code").unwrap_err();
        assert!(err.starts_with("CELL_OUT_OF_RANGE"), "got: {}", err);
        assert!(err.contains("1 cell(s)"), "got: {}", err);
    }

    #[test]
    fn insert_before_beyond_total_plus_one_rejected() {
        let mut nb = notebook(vec![md_cell("one\n")]);
        // total = 1, so 2 (append) is fine but 3 is out of range.
        let err = apply_edit(&mut nb, EditMode::InsertBefore, 3, Some("x\n"), "code").unwrap_err();
        assert!(err.starts_with("CELL_OUT_OF_RANGE"), "got: {}", err);
    }

    #[test]
    fn delete_on_empty_notebook_rejected() {
        let mut nb = notebook(vec![]);
        let err = apply_edit(&mut nb, EditMode::Delete, 1, None, "code").unwrap_err();
        assert!(err.starts_with("CELL_OUT_OF_RANGE"), "got: {}", err);
    }

    #[test]
    fn missing_cells_array_is_shape_error() {
        let mut nb = json!({ "nbformat": 4 });
        let err = apply_edit(&mut nb, EditMode::Delete, 1, None, "code").unwrap_err();
        assert!(err.starts_with("NOTEBOOK_SHAPE_ERROR"), "got: {}", err);
    }

    // ── misc helpers ─────────────────────────────────────────────────────

    #[test]
    fn parse_mode_accepts_all_four_and_rejects_junk() {
        assert_eq!(parse_mode("replace"), Some(EditMode::Replace));
        assert_eq!(parse_mode("insert_before"), Some(EditMode::InsertBefore));
        assert_eq!(parse_mode("insert_after"), Some(EditMode::InsertAfter));
        assert_eq!(parse_mode("delete"), Some(EditMode::Delete));
        assert_eq!(parse_mode("append"), None);
    }

    #[test]
    fn int_param_accepts_number_and_string() {
        assert_eq!(int_param(&json!({"cell": 3}), "cell"), Some(3));
        assert_eq!(int_param(&json!({"cell": "3"}), "cell"), Some(3));
        assert_eq!(int_param(&json!({"cell": "x"}), "cell"), None);
        assert_eq!(int_param(&json!({}), "cell"), None);
    }

    #[test]
    fn source_preview_truncates_and_flattens() {
        let long = "line1\n".repeat(30);
        let p = source_preview(&long);
        assert!(p.chars().count() <= 81); // 80 + ellipsis
        assert!(p.ends_with('…'));
        assert!(p.contains("\\n"));
        assert!(!p.contains('\n'));
        assert_eq!(source_preview("short"), "short");
    }
}

//! Editor commands — server dispatch. Mirrors the desktop bodies in
//! `src-tauri/src/commands/editor.rs`, operating on the shared `AppState`
//! `buffers` / `highlighters` maps and the same `rustic_core` functions.

use std::path::Path;

use serde::Deserialize;
use serde_json::Value;

use rustic_app::context::AppContext;
use rustic_app::path_scope::validate_readable_path;
use rustic_app::state::AppState;
use rustic_core::buffer::Edit;
use rustic_core::syntax::{RenderedLine, SyntaxHighlighter};

use crate::api::{ok, parse, ApiError, PathArg};
use crate::context::ServerContext;

#[derive(serde::Serialize)]
struct EditResponse {
    line_count: usize,
    is_modified: bool,
}

pub async fn dispatch(
    ctx: &ServerContext,
    command: &str,
    args: &Value,
) -> Option<Result<Value, ApiError>> {
    Some(match command {
        "open_file" => match parse::<PathArg>(args) {
            Ok(a) => open_file(ctx, a.path),
            Err(e) => Err(e),
        },
        "open_scratch_buffer" => open_scratch_buffer(ctx, args),
        "get_visible_lines" => get_visible_lines(ctx, args),
        "highlight_buffer" => highlight_buffer(ctx, args),
        "highlight_range" => highlight_range(ctx, args),
        "edit_buffer" => edit_buffer(ctx, args),
        "format_buffer" => format_buffer(ctx, args),
        "save_file" => save_file(ctx, args),
        "buffer_external_change" => buffer_external_change(ctx, args),
        "reload_buffer" => reload_buffer(ctx, args),
        "undo_edit" => undo_edit(ctx, args),
        "redo_edit" => redo_edit(ctx, args),
        "close_buffer" => close_buffer(ctx, args),
        _ => return None,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BufferIdArg {
    buffer_id: u64,
}

fn open_file(ctx: &ServerContext, path: String) -> Result<Value, ApiError> {
    let state = ctx.state();
    let file_path = Path::new(&path);
    validate_readable_path(file_path)?;

    // Check if buffer already exists for this path.
    {
        let buffers = state.buffers.lock().map_err(|e| e.to_string())?;
        for buffer in buffers.values() {
            if buffer.file_path.as_deref() == Some(file_path) {
                return ok(buffer.info());
            }
        }
    }

    let buffer = rustic_core::buffer::Buffer::from_file(file_path).map_err(|e| e.to_string())?;
    let buffer_id = buffer.id;
    let info = buffer.info();

    let mut buffers = state.buffers.lock().map_err(|e| e.to_string())?;
    buffers.insert(buffer_id, buffer);

    ok(info)
}

/// Ensure a highlighter exists for `buffer_id`, creating one from the buffer's
/// detected language if needed. Cheap if one already exists. Performs the
/// (potentially expensive) `SyntaxHighlighter::new()` without holding the
/// highlighters lock so other commands aren't blocked.
fn ensure_highlighter(state: &AppState, buffer_id: u64) -> Result<(), String> {
    {
        let highlighters = state.highlighters.lock().map_err(|e| e.to_string())?;
        if highlighters.contains_key(&buffer_id) {
            return Ok(());
        }
    }
    let language = {
        let buffers = state.buffers.lock().map_err(|e| e.to_string())?;
        let buffer = buffers.get(&buffer_id).ok_or("Buffer not found")?;
        buffer.language.clone()
    };
    let highlighter = language
        .as_deref()
        .and_then(SyntaxHighlighter::new)
        .unwrap_or_else(SyntaxHighlighter::new_generic);
    let mut highlighters = state.highlighters.lock().map_err(|e| e.to_string())?;
    highlighters.entry(buffer_id).or_insert(highlighter);
    Ok(())
}

fn open_scratch_buffer(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct A {
        title: String,
        content: String,
        language: Option<String>,
    }
    let a: A = parse(args)?;
    let state = ctx.state();

    let mut buffer = rustic_core::buffer::Buffer::from_string(&a.content);
    buffer.file_path = Some(std::path::PathBuf::from(&a.title));
    buffer.language = a.language.clone();
    let buffer_id = buffer.id;

    let highlighter = a
        .language
        .as_deref()
        .and_then(SyntaxHighlighter::new)
        .unwrap_or_else(SyntaxHighlighter::new_generic);
    {
        let mut highlighters = state.highlighters.lock().map_err(|e| e.to_string())?;
        highlighters.insert(buffer_id, highlighter);
    }

    let info = buffer.info();
    let mut buffers = state.buffers.lock().map_err(|e| e.to_string())?;
    buffers.insert(buffer_id, buffer);

    ok(info)
}

fn get_visible_lines(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct A {
        buffer_id: u64,
        start: usize,
        end: usize,
    }
    let a: A = parse(args)?;
    let state = ctx.state();

    let buffers = state.buffers.lock().map_err(|e| e.to_string())?;
    let buffer = buffers.get(&a.buffer_id).ok_or("Buffer not found")?;

    // Serve highlighted lines from cache if available (non-blocking try_lock).
    if let Ok(highlighters) = state.highlighters.try_lock() {
        if let Some(highlighter) = highlighters.get(&a.buffer_id) {
            if let Some(lines) = highlighter.get_cached_range(a.start, a.end) {
                return ok(lines);
            }
        }
    }

    // No highlight cache available — return plain text instantly.
    let lines: Vec<RenderedLine> = (a.start..a.end.min(buffer.line_count()))
        .map(|i| RenderedLine {
            line_number: i + 1,
            text: buffer.get_line(i).unwrap_or_default(),
            spans: Vec::new(),
        })
        .collect();
    ok(lines)
}

fn highlight_buffer(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: BufferIdArg = parse(args)?;
    let state = ctx.state();
    let buffer_id = a.buffer_id;

    ensure_highlighter(state, buffer_id)?;

    let rope_clone = {
        let buffers = state.buffers.lock().map_err(|e| e.to_string())?;
        let buffer = buffers.get(&buffer_id).ok_or("Buffer not found")?;
        buffer.rope.clone()
    };

    // Take the highlighter OUT of the map so we can drop the lock before the
    // expensive Tree-sitter parse.
    let mut highlighter = {
        let mut highlighters = state.highlighters.lock().map_err(|e| e.to_string())?;
        match highlighters.remove(&buffer_id) {
            Some(h) => h,
            None => return ok(false),
        }
    };

    highlighter.ensure_highlighted(&rope_clone);

    let mut highlighters = state.highlighters.lock().map_err(|e| e.to_string())?;
    highlighters.insert(buffer_id, highlighter);
    ok(true)
}

fn highlight_range(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct A {
        buffer_id: u64,
        start_line: usize,
        end_line: usize,
    }
    let a: A = parse(args)?;
    let state = ctx.state();
    let buffer_id = a.buffer_id;

    ensure_highlighter(state, buffer_id)?;

    let rope_clone = {
        let buffers = state.buffers.lock().map_err(|e| e.to_string())?;
        let buffer = buffers.get(&buffer_id).ok_or("Buffer not found")?;
        buffer.rope.clone()
    };

    // Check cache first (quick lock).
    {
        let highlighters = state.highlighters.lock().map_err(|e| e.to_string())?;
        if let Some(highlighter) = highlighters.get(&buffer_id) {
            if let Some(lines) = highlighter.get_cached_range(a.start_line, a.end_line) {
                return ok(lines);
            }
        }
    }

    let highlighter_opt = {
        let mut highlighters = state.highlighters.lock().map_err(|e| e.to_string())?;
        highlighters.remove(&buffer_id)
    };

    match highlighter_opt {
        Some(mut highlighter) => {
            let lines = highlighter.highlight_range(&rope_clone, a.start_line, a.end_line);
            let mut highlighters = state.highlighters.lock().map_err(|e| e.to_string())?;
            highlighters.insert(buffer_id, highlighter);
            ok(lines)
        }
        None => {
            let line_count = rope_clone.len_lines();
            let lines: Vec<RenderedLine> = (a.start_line..a.end_line.min(line_count))
                .map(|i| {
                    let text = rope_clone
                        .line(i)
                        .to_string()
                        .trim_end_matches('\n')
                        .trim_end_matches('\r')
                        .to_string();
                    RenderedLine {
                        line_number: i + 1,
                        text,
                        spans: Vec::new(),
                    }
                })
                .collect();
            ok(lines)
        }
    }
}

fn edit_buffer(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct A {
        buffer_id: u64,
        line: usize,
        col: usize,
        new_text: String,
        delete_count: usize,
    }
    let a: A = parse(args)?;
    let state = ctx.state();
    let buffer_id = a.buffer_id;

    let mut buffers = state.buffers.lock().map_err(|e| e.to_string())?;
    let buffer = buffers.get_mut(&buffer_id).ok_or("Buffer not found")?;

    // Convert line/col to byte offset.
    let line_idx = a.line.min(buffer.line_count().saturating_sub(1));
    let line_start_byte = buffer.byte_offset_of_line(line_idx);
    let line_text = buffer.get_line(line_idx).unwrap_or_default();

    let col_byte: usize = line_text.chars().take(a.col).map(|c| c.len_utf8()).sum();
    let byte_offset = line_start_byte + col_byte;

    // Slice only the affected range out of the rope to compute old_text.
    let old_text = if a.delete_count > 0 {
        let total_bytes = buffer.rope.len_bytes();
        let end_byte = (byte_offset + a.delete_count).min(total_bytes);
        let start_char = buffer.rope.byte_to_char(byte_offset);
        let end_char = buffer.rope.byte_to_char(end_byte);
        buffer.rope.slice(start_char..end_char).to_string()
    } else {
        String::new()
    };

    let old_text_len = old_text.len();
    let new_text_len = a.new_text.len();
    let start_row = buffer.rope.byte_to_line(byte_offset);
    let start_line_byte = buffer.byte_offset_of_line(start_row);
    let start_column = byte_offset - start_line_byte;

    let old_end_byte = byte_offset + old_text_len;
    let old_end_row = buffer
        .rope
        .byte_to_line(old_end_byte.min(buffer.rope.len_bytes()));
    let old_end_line_byte = buffer.byte_offset_of_line(old_end_row);
    let old_end_column = old_end_byte.saturating_sub(old_end_line_byte);

    let edit = Edit {
        byte_offset,
        old_text,
        new_text: a.new_text.clone(),
    };

    buffer.apply_edit(edit).map_err(|e| e.to_string())?;

    let new_end_byte = byte_offset + new_text_len;
    let new_end_row = buffer
        .rope
        .byte_to_line(new_end_byte.min(buffer.rope.len_bytes()));
    let new_end_line_byte = buffer.byte_offset_of_line(new_end_row);
    let new_end_column = new_end_byte.saturating_sub(new_end_line_byte);

    let new_source = buffer.rope.to_string();
    drop(buffers);

    let input_edit = rustic_core::tree_sitter::InputEdit {
        start_byte: byte_offset,
        old_end_byte,
        new_end_byte,
        start_position: rustic_core::tree_sitter::Point {
            row: start_row,
            column: start_column,
        },
        old_end_position: rustic_core::tree_sitter::Point {
            row: old_end_row,
            column: old_end_column,
        },
        new_end_position: rustic_core::tree_sitter::Point {
            row: new_end_row,
            column: new_end_column,
        },
    };

    {
        let mut highlighters = state.highlighters.lock().map_err(|e| e.to_string())?;
        if let Some(highlighter) = highlighters.get_mut(&buffer_id) {
            highlighter.apply_edit(input_edit, &new_source);
        }
    }

    let buffers = state.buffers.lock().map_err(|e| e.to_string())?;
    let buffer = buffers.get(&buffer_id).ok_or("Buffer not found")?;
    ok(EditResponse {
        line_count: buffer.line_count(),
        is_modified: buffer.is_modified(),
    })
}

fn format_buffer(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct A {
        buffer_id: u64,
        indent_size: usize,
    }
    let a: A = parse(args)?;
    let state = ctx.state();
    let buffer_id = a.buffer_id;

    let mut buffers = state.buffers.lock().map_err(|e| e.to_string())?;
    let buffer = buffers.get_mut(&buffer_id).ok_or("Buffer not found")?;

    let language = buffer.language.as_deref().unwrap_or("text").to_string();
    let source = buffer.rope.to_string();

    let (detected_use_tabs, detected_indent_size) = detect_indent_style(&source);
    let effective_indent_size = if detected_indent_size > 0 {
        detected_indent_size
    } else {
        a.indent_size
    };

    match rustic_core::formatter::format_code(
        &source,
        &language,
        effective_indent_size,
        detected_use_tabs,
    ) {
        Some(formatted) => {
            buffer.rope = rustic_core::buffer::Rope::from_str(&formatted);
            drop(buffers);
            if let Ok(mut highlighters) = state.highlighters.lock() {
                if let Some(highlighter) = highlighters.get_mut(&buffer_id) {
                    highlighter.invalidate_cache();
                }
            }
            let buffers = state.buffers.lock().map_err(|e| e.to_string())?;
            let buffer = buffers.get(&buffer_id).ok_or("Buffer not found")?;
            ok(Some(buffer.line_count()))
        }
        None => ok(Option::<usize>::None),
    }
}

fn save_file(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct A {
        buffer_id: u64,
        force: Option<bool>,
    }
    let a: A = parse(args)?;
    let state = ctx.state();

    let mut buffers = state.buffers.lock().map_err(|e| e.to_string())?;
    let buffer = buffers.get_mut(&a.buffer_id).ok_or("Buffer not found")?;
    if !a.force.unwrap_or(false) && buffer.external_change_detected() {
        return Err(ApiError::from("EXTERNAL_CHANGE_DETECTED".to_string()));
    }
    buffer.save().map_err(|e| e.to_string())?;
    ok(serde_json::json!(null))
}

fn buffer_external_change(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: BufferIdArg = parse(args)?;
    let state = ctx.state();
    let buffers = state.buffers.lock().map_err(|e| e.to_string())?;
    let buffer = buffers.get(&a.buffer_id).ok_or("Buffer not found")?;
    ok(buffer.external_change_detected())
}

fn reload_buffer(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: BufferIdArg = parse(args)?;
    let state = ctx.state();
    let buffer_id = a.buffer_id;

    let mut buffers = state.buffers.lock().map_err(|e| e.to_string())?;
    let buffer = buffers.get_mut(&buffer_id).ok_or("Buffer not found")?;
    buffer.reload_from_disk().map_err(|e| e.to_string())?;

    drop(buffers);
    if let Ok(mut highlighters) = state.highlighters.lock() {
        if let Some(highlighter) = highlighters.get_mut(&buffer_id) {
            highlighter.invalidate_cache();
        }
    }

    let buffers = state.buffers.lock().map_err(|e| e.to_string())?;
    let buffer = buffers.get(&buffer_id).ok_or("Buffer not found")?;
    ok(buffer.info())
}

fn undo_edit(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: BufferIdArg = parse(args)?;
    let state = ctx.state();
    let buffer_id = a.buffer_id;

    let mut buffers = state.buffers.lock().map_err(|e| e.to_string())?;
    let buffer = buffers.get_mut(&buffer_id).ok_or("Buffer not found")?;
    buffer.undo();

    let mut highlighters = state.highlighters.lock().map_err(|e| e.to_string())?;
    if let Some(highlighter) = highlighters.get_mut(&buffer_id) {
        highlighter.invalidate_cache();
    }

    ok(EditResponse {
        line_count: buffer.line_count(),
        is_modified: buffer.is_modified(),
    })
}

fn redo_edit(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: BufferIdArg = parse(args)?;
    let state = ctx.state();
    let buffer_id = a.buffer_id;

    let mut buffers = state.buffers.lock().map_err(|e| e.to_string())?;
    let buffer = buffers.get_mut(&buffer_id).ok_or("Buffer not found")?;
    buffer.redo();

    let mut highlighters = state.highlighters.lock().map_err(|e| e.to_string())?;
    if let Some(highlighter) = highlighters.get_mut(&buffer_id) {
        highlighter.invalidate_cache();
    }

    ok(EditResponse {
        line_count: buffer.line_count(),
        is_modified: buffer.is_modified(),
    })
}

fn close_buffer(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: BufferIdArg = parse(args)?;
    let state = ctx.state();

    let mut buffers = state.buffers.lock().map_err(|e| e.to_string())?;
    buffers.remove(&a.buffer_id);

    let mut highlighters = state.highlighters.lock().map_err(|e| e.to_string())?;
    highlighters.remove(&a.buffer_id);

    ok(serde_json::json!(null))
}

fn detect_indent_style(source: &str) -> (bool, usize) {
    let mut tab_lines: usize = 0;
    let mut space_lines: usize = 0;
    let mut space_run_counts: std::collections::HashMap<usize, usize> =
        std::collections::HashMap::new();

    for line in source.lines().take(500) {
        if line.starts_with('\t') {
            tab_lines += 1;
        } else if line.starts_with("  ") {
            let run = line.len() - line.trim_start_matches(' ').len();
            if run > 0 {
                space_lines += 1;
                *space_run_counts.entry(run).or_insert(0) += 1;
            }
        }
    }

    if tab_lines > space_lines {
        return (true, 0);
    }
    if space_lines == 0 {
        return (false, 0);
    }
    let most_common = space_run_counts
        .into_iter()
        .max_by_key(|&(_, count)| count)
        .map(|(run, _)| run)
        .unwrap_or(0);
    (false, most_common)
}

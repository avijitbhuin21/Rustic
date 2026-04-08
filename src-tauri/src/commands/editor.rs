use crate::state::AppState;
use rustic_core::buffer::{BufferInfo, Edit};
use rustic_core::syntax::{RenderedLine, SyntaxHighlighter};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tauri::State;

#[derive(Debug, Serialize, Deserialize)]
pub struct EditResponse {
    pub line_count: usize,
    pub is_modified: bool,
}

#[tauri::command]
pub async fn open_file(
    state: State<'_, AppState>,
    path: String,
) -> Result<BufferInfo, String> {
    let file_path = Path::new(&path);

    // Check if buffer already exists for this path
    {
        let buffers = state.buffers.lock().map_err(|e| e.to_string())?;
        for buffer in buffers.values() {
            if buffer.file_path.as_deref() == Some(file_path) {
                return Ok(buffer.info());
            }
        }
    }

    let buffer =
        rustic_core::buffer::Buffer::from_file(file_path).map_err(|e| e.to_string())?;

    let buffer_id = buffer.id;
    let info = buffer.info();
    let lang = buffer.language.as_deref().unwrap_or("unknown");

    // Create highlighter: try Tree-sitter first, fall back to generic regex
    let ts_highlighter = buffer.language.as_deref().and_then(SyntaxHighlighter::new);
    let engine = if ts_highlighter.is_some() { "tree-sitter" } else { "generic" };
    eprintln!("[SyntaxHighlight] open_file: path={:?} lang={} engine={} buffer_id={}", path, lang, engine, buffer_id);

    let highlighter = ts_highlighter.unwrap_or_else(SyntaxHighlighter::new_generic);
    {
        let mut highlighters = state.highlighters.lock().map_err(|e| e.to_string())?;
        highlighters.insert(buffer_id, highlighter);
    }

    let mut buffers = state.buffers.lock().map_err(|e| e.to_string())?;
    buffers.insert(buffer_id, buffer);

    Ok(info)
}

#[tauri::command]
pub async fn open_scratch_buffer(
    state: State<'_, AppState>,
    title: String,
    content: String,
    language: Option<String>,
) -> Result<BufferInfo, String> {
    let mut buffer = rustic_core::buffer::Buffer::from_string(&content);

    // Use title as a synthetic file path so the tab shows a meaningful name
    buffer.file_path = Some(std::path::PathBuf::from(&title));
    buffer.language = language.clone();

    let buffer_id = buffer.id;

    // Create highlighter based on language
    let highlighter = language
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

    Ok(info)
}

#[tauri::command]
pub async fn get_visible_lines(
    state: State<'_, AppState>,
    buffer_id: u64,
    start: usize,
    end: usize,
) -> Result<Vec<RenderedLine>, String> {
    let buffers = state.buffers.lock().map_err(|e| e.to_string())?;
    let buffer = buffers.get(&buffer_id).ok_or("Buffer not found")?;

    // Try to serve highlighted lines from cache.
    // Use try_lock so we never block if highlight_buffer is running a parse.
    if let Ok(highlighters) = state.highlighters.try_lock() {
        if let Some(highlighter) = highlighters.get(&buffer_id) {
            if let Some(lines) = highlighter.get_cached_range(start, end) {
                let span_count: usize = lines.iter().map(|l| l.spans.len()).sum();
                eprintln!("[SyntaxHighlight] get_visible_lines: buffer_id={} range={}..{} source=cache spans={}", buffer_id, start, end, span_count);
                return Ok(lines);
            }
        }
    }

    // No highlight cache available — return plain text instantly
    eprintln!("[SyntaxHighlight] get_visible_lines: buffer_id={} range={}..{} source=plain (no cache)", buffer_id, start, end);
    let lines = (start..end.min(buffer.line_count()))
        .map(|i| RenderedLine {
            line_number: i + 1,
            text: buffer.get_line(i).unwrap_or_default(),
            spans: Vec::new(),
        })
        .collect();
    Ok(lines)
}

/// Trigger a full Tree-sitter parse for a buffer.
/// Returns true if highlighting was performed, false if no highlighter exists.
/// The result is cached — subsequent get_visible_lines calls will return highlighted data.
#[tauri::command]
pub async fn highlight_buffer(
    state: State<'_, AppState>,
    buffer_id: u64,
) -> Result<bool, String> {
    // Clone the rope so we don't hold the buffers lock during parsing.
    // Ropey's clone is O(1) — it shares the underlying data via reference counting.
    let rope_clone = {
        let buffers = state.buffers.lock().map_err(|e| e.to_string())?;
        let buffer = buffers.get(&buffer_id).ok_or("Buffer not found")?;
        buffer.rope.clone()
    }; // buffers lock dropped here

    let mut highlighters = state.highlighters.lock().map_err(|e| e.to_string())?;
    if let Some(highlighter) = highlighters.get_mut(&buffer_id) {
        let was_cached = highlighter.is_cached();
        highlighter.ensure_highlighted(&rope_clone);
        let is_cached = highlighter.is_cached();
        eprintln!("[SyntaxHighlight] highlight_buffer: buffer_id={} was_cached={} is_cached={}", buffer_id, was_cached, is_cached);
        Ok(true)
    } else {
        eprintln!("[SyntaxHighlight] highlight_buffer: buffer_id={} no highlighter found!", buffer_id);
        Ok(false)
    }
}

#[tauri::command]
pub async fn edit_buffer(
    state: State<'_, AppState>,
    buffer_id: u64,
    line: usize,
    col: usize,
    new_text: String,
    delete_count: usize,
) -> Result<EditResponse, String> {
    let mut buffers = state.buffers.lock().map_err(|e| e.to_string())?;
    let buffer = buffers.get_mut(&buffer_id).ok_or("Buffer not found")?;

    // Convert line/col to byte offset
    let line_idx = line.min(buffer.line_count().saturating_sub(1));
    let line_start_byte = buffer.byte_offset_of_line(line_idx);
    let line_text = buffer.get_line(line_idx).unwrap_or_default();

    // Convert col to byte offset within line
    let col_byte: usize = line_text
        .chars()
        .take(col)
        .map(|c| c.len_utf8())
        .sum();
    let byte_offset = line_start_byte + col_byte;

    // Compute old_text (what will be deleted)
    let old_text = if delete_count > 0 {
        let rope_str = buffer.rope.to_string();
        let end_byte = (byte_offset + delete_count).min(rope_str.len());
        rope_str[byte_offset..end_byte].to_string()
    } else {
        String::new()
    };

    let edit = Edit {
        byte_offset,
        old_text,
        new_text,
    };

    buffer.apply_edit(edit).map_err(|e| e.to_string())?;

    // Invalidate highlight cache since buffer content changed
    let mut highlighters = state.highlighters.lock().map_err(|e| e.to_string())?;
    if let Some(highlighter) = highlighters.get_mut(&buffer_id) {
        highlighter.invalidate_cache();
    }

    Ok(EditResponse {
        line_count: buffer.line_count(),
        is_modified: buffer.is_modified,
    })
}

/// Format a buffer's content using the built-in formatter.
/// Returns the new line count if formatting changed anything, or None if no changes.
#[tauri::command]
pub async fn format_buffer(
    state: State<'_, AppState>,
    buffer_id: u64,
    indent_size: usize,
) -> Result<Option<usize>, String> {
    let mut buffers = state.buffers.lock().map_err(|e| e.to_string())?;
    let buffer = buffers.get_mut(&buffer_id).ok_or("Buffer not found")?;

    let language = buffer.language.as_deref().unwrap_or("text");
    let source = buffer.rope.to_string();

    match rustic_core::formatter::format_code(&source, language, indent_size) {
        Some(formatted) => {
            eprintln!("[Formatter] buffer_id={} lang={} changed=true", buffer_id, language);
            buffer.rope = rustic_core::buffer::Rope::from_str(&formatted);
            buffer.is_modified = true;

            // Invalidate highlighting cache since content changed
            drop(buffers);
            if let Ok(mut highlighters) = state.highlighters.lock() {
                if let Some(highlighter) = highlighters.get_mut(&buffer_id) {
                    highlighter.invalidate_cache();
                }
            }

            let buffers = state.buffers.lock().map_err(|e| e.to_string())?;
            let buffer = buffers.get(&buffer_id).ok_or("Buffer not found")?;
            Ok(Some(buffer.line_count()))
        }
        None => {
            eprintln!("[Formatter] buffer_id={} lang={} changed=false", buffer_id, language);
            Ok(None)
        }
    }
}

#[tauri::command]
pub async fn save_file(
    state: State<'_, AppState>,
    buffer_id: u64,
) -> Result<(), String> {
    let mut buffers = state.buffers.lock().map_err(|e| e.to_string())?;
    let buffer = buffers.get_mut(&buffer_id).ok_or("Buffer not found")?;
    buffer.save().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn undo_edit(
    state: State<'_, AppState>,
    buffer_id: u64,
) -> Result<EditResponse, String> {
    let mut buffers = state.buffers.lock().map_err(|e| e.to_string())?;
    let buffer = buffers.get_mut(&buffer_id).ok_or("Buffer not found")?;

    buffer.undo();

    // Invalidate highlight cache since buffer content changed
    let mut highlighters = state.highlighters.lock().map_err(|e| e.to_string())?;
    if let Some(highlighter) = highlighters.get_mut(&buffer_id) {
        highlighter.invalidate_cache();
    }

    Ok(EditResponse {
        line_count: buffer.line_count(),
        is_modified: buffer.is_modified,
    })
}

#[tauri::command]
pub async fn redo_edit(
    state: State<'_, AppState>,
    buffer_id: u64,
) -> Result<EditResponse, String> {
    let mut buffers = state.buffers.lock().map_err(|e| e.to_string())?;
    let buffer = buffers.get_mut(&buffer_id).ok_or("Buffer not found")?;

    buffer.redo();

    // Invalidate highlight cache since buffer content changed
    let mut highlighters = state.highlighters.lock().map_err(|e| e.to_string())?;
    if let Some(highlighter) = highlighters.get_mut(&buffer_id) {
        highlighter.invalidate_cache();
    }

    Ok(EditResponse {
        line_count: buffer.line_count(),
        is_modified: buffer.is_modified,
    })
}

#[tauri::command]
pub async fn close_buffer(
    state: State<'_, AppState>,
    buffer_id: u64,
) -> Result<(), String> {
    let mut buffers = state.buffers.lock().map_err(|e| e.to_string())?;
    buffers.remove(&buffer_id);

    let mut highlighters = state.highlighters.lock().map_err(|e| e.to_string())?;
    highlighters.remove(&buffer_id);

    Ok(())
}

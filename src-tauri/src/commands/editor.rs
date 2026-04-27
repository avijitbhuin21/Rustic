use crate::path_scope::validate_readable_path;
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
    validate_readable_path(file_path)?;

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
    tracing::debug!("[SyntaxHighlight] open_file: path={:?} lang={} buffer_id={}", path, lang, buffer_id);

    // Highlighter is created lazily on the first highlight_range / highlight_buffer call
    // so we don't pay the tree-sitter grammar + query compilation cost on the response path.

    let mut buffers = state.buffers.lock().map_err(|e| e.to_string())?;
    buffers.insert(buffer_id, buffer);

    Ok(info)
}

/// Ensure a highlighter exists in the map for this buffer, creating one based
/// on the buffer's detected language if needed. Cheap if one already exists.
/// Performs the (potentially expensive) `SyntaxHighlighter::new()` without
/// holding the highlighters lock so other commands aren't blocked.
fn ensure_highlighter(
    state: &State<'_, AppState>,
    buffer_id: u64,
) -> Result<(), String> {
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
    // If another command raced us and created one already, keep theirs to avoid
    // discarding any cache it may have already populated.
    highlighters.entry(buffer_id).or_insert(highlighter);
    Ok(())
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
                return Ok(lines);
            }
        }
    }

    // No highlight cache available — return plain text instantly
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
    // Lazily create the highlighter on first use (it's no longer created in open_file).
    ensure_highlighter(&state, buffer_id)?;

    // Clone the rope so we don't hold the buffers lock during parsing.
    // Ropey's clone is O(1) — it shares the underlying data via reference counting.
    let rope_clone = {
        let buffers = state.buffers.lock().map_err(|e| e.to_string())?;
        let buffer = buffers.get(&buffer_id).ok_or("Buffer not found")?;
        buffer.rope.clone()
    }; // buffers lock dropped here

    // CRITICAL: Take the highlighter OUT of the map so we can drop the lock
    // before running the expensive Tree-sitter parse. This prevents blocking
    // open_file and other commands that need the highlighters lock.
    let mut highlighter = {
        let mut highlighters = state.highlighters.lock().map_err(|e| e.to_string())?;
        match highlighters.remove(&buffer_id) {
            Some(h) => h,
            None => {
                // Another concurrent highlight call has the highlighter; skip.
                return Ok(false);
            }
        }
    }; // highlighters lock dropped here — other commands can proceed

    // Run the expensive parse WITHOUT holding any lock
    highlighter.ensure_highlighted(&rope_clone);

    // Put the highlighter back
    let mut highlighters = state.highlighters.lock().map_err(|e| e.to_string())?;
    highlighters.insert(buffer_id, highlighter);
    Ok(true)
}

/// Highlight only a specific line range of a buffer.
/// Does a full Tree-sitter parse for correctness but only builds span data
/// for the requested range — much faster than highlight_buffer for viewport use.
#[tauri::command]
pub async fn highlight_range(
    state: State<'_, AppState>,
    buffer_id: u64,
    start_line: usize,
    end_line: usize,
) -> Result<Vec<RenderedLine>, String> {
    // Lazily create the highlighter on first use (it's no longer created in open_file).
    ensure_highlighter(&state, buffer_id)?;

    // Clone the rope so we don't hold the buffers lock during parsing
    let rope_clone = {
        let buffers = state.buffers.lock().map_err(|e| e.to_string())?;
        let buffer = buffers.get(&buffer_id).ok_or("Buffer not found")?;
        buffer.rope.clone()
    };

    // Check cache first (quick lock)
    {
        let highlighters = state.highlighters.lock().map_err(|e| e.to_string())?;
        if let Some(highlighter) = highlighters.get(&buffer_id) {
            if let Some(lines) = highlighter.get_cached_range(start_line, end_line) {
                return Ok(lines);
            }
        }
    } // lock dropped

    // No cache — take highlighter out, parse without lock, put it back.
    // If another command (highlight_buffer) has temporarily removed the highlighter,
    // fall back to plain text instead of erroring.
    let highlighter_opt = {
        let mut highlighters = state.highlighters.lock().map_err(|e| e.to_string())?;
        highlighters.remove(&buffer_id)
    }; // lock dropped — other commands can proceed

    match highlighter_opt {
        Some(mut highlighter) => {
            let lines = highlighter.highlight_range(&rope_clone, start_line, end_line);
            // Put it back
            let mut highlighters = state.highlighters.lock().map_err(|e| e.to_string())?;
            highlighters.insert(buffer_id, highlighter);
            Ok(lines)
        }
        None => {
            // Highlighter temporarily taken by another command — return plain text
            let line_count = rope_clone.len_lines();
            let lines = (start_line..end_line.min(line_count))
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
            Ok(lines)
        }
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

    // Compute old_text (what will be deleted) by slicing only the affected
    // range out of the rope — never materialize the entire buffer to a String.
    // For a 5MB file this saves a 5MB allocation per backspace.
    let old_text = if delete_count > 0 {
        let total_bytes = buffer.rope.len_bytes();
        let end_byte = (byte_offset + delete_count).min(total_bytes);
        let start_char = buffer.rope.byte_to_char(byte_offset);
        let end_char = buffer.rope.byte_to_char(end_byte);
        buffer.rope.slice(start_char..end_char).to_string()
    } else {
        String::new()
    };

    // Capture pre-edit positional info for the InputEdit. Tree-sitter wants
    // (start_byte, old_end_byte, new_end_byte) plus row/col Points for each.
    let old_text_len = old_text.len();
    let new_text_len = new_text.len();
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
        new_text: new_text.clone(),
    };

    buffer.apply_edit(edit).map_err(|e| e.to_string())?;

    // Compute the post-edit end position from the now-updated rope.
    let new_end_byte = byte_offset + new_text_len;
    let new_end_row = buffer
        .rope
        .byte_to_line(new_end_byte.min(buffer.rope.len_bytes()));
    let new_end_line_byte = buffer.byte_offset_of_line(new_end_row);
    let new_end_column = new_end_byte.saturating_sub(new_end_line_byte);

    // Snapshot the post-edit source for the highlighter — clone now while we
    // still hold the buffers lock, then drop the lock before doing the
    // (potentially expensive) parse.
    let new_source = buffer.rope.to_string();
    drop(buffers);

    // Push the edit into the persistent Tree-sitter tree so the next
    // highlight call reparses incrementally instead of from scratch.
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

    // Reacquire the buffers lock to read the response fields.
    let buffers = state.buffers.lock().map_err(|e| e.to_string())?;
    let buffer = buffers.get(&buffer_id).ok_or("Buffer not found")?;
    Ok(EditResponse {
        line_count: buffer.line_count(),
        is_modified: buffer.is_modified(),
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
            tracing::warn!("[Formatter] buffer_id={} lang={} changed=true", buffer_id, language);
            buffer.rope = rustic_core::buffer::Rope::from_str(&formatted);

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
            tracing::warn!("[Formatter] buffer_id={} lang={} changed=false", buffer_id, language);
            Ok(None)
        }
    }
}

#[tauri::command]
pub async fn save_file(
    state: State<'_, AppState>,
    buffer_id: u64,
    force: Option<bool>,
) -> Result<(), String> {
    let mut buffers = state.buffers.lock().map_err(|e| e.to_string())?;
    let buffer = buffers.get_mut(&buffer_id).ok_or("Buffer not found")?;
    // If the on-disk file changed since we loaded it and the caller did not
    // pass force=true, refuse with a sentinel string so the frontend can
    // prompt the user to reload / overwrite / cancel.
    if !force.unwrap_or(false) && buffer.external_change_detected() {
        return Err("EXTERNAL_CHANGE_DETECTED".to_string());
    }
    buffer.save().map_err(|e| e.to_string())
}

/// Re-stat the file backing this buffer and report whether it changed on disk.
/// Cheap (one stat call). Used by the frontend on watcher events / on focus.
#[tauri::command]
pub async fn buffer_external_change(
    state: State<'_, AppState>,
    buffer_id: u64,
) -> Result<bool, String> {
    let buffers = state.buffers.lock().map_err(|e| e.to_string())?;
    let buffer = buffers.get(&buffer_id).ok_or("Buffer not found")?;
    Ok(buffer.external_change_detected())
}

/// Discard in-memory edits and reload the buffer from disk. Returns the
/// updated buffer info so the frontend can refresh its view.
#[tauri::command]
pub async fn reload_buffer(
    state: State<'_, AppState>,
    buffer_id: u64,
) -> Result<rustic_core::buffer::BufferInfo, String> {
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
    Ok(buffer.info())
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
        is_modified: buffer.is_modified(),
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
        is_modified: buffer.is_modified(),
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

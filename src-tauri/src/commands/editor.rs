//! Editor buffer commands. Bodies live in `rustic_app::host_ops::editor`
//! (shared with rustic-server); these are the Tauri adapters.

use crate::state::AppState;
use rustic_app::host_ops::editor as ops;
pub use rustic_app::host_ops::editor::EditResponse;
use rustic_core::buffer::BufferInfo;
use rustic_core::syntax::RenderedLine;
use tauri::State;

#[tauri::command]
pub async fn open_file(state: State<'_, AppState>, path: String) -> Result<BufferInfo, String> {
    ops::open_file(&state, &path)
}

#[tauri::command]
pub async fn open_scratch_buffer(
    state: State<'_, AppState>,
    title: String,
    content: String,
    language: Option<String>,
) -> Result<BufferInfo, String> {
    ops::open_scratch_buffer(&state, &title, &content, language)
}

#[tauri::command]
pub async fn get_visible_lines(
    state: State<'_, AppState>,
    buffer_id: u64,
    start: usize,
    end: usize,
) -> Result<Vec<RenderedLine>, String> {
    ops::get_visible_lines(&state, buffer_id, start, end)
}

#[tauri::command]
pub async fn highlight_buffer(state: State<'_, AppState>, buffer_id: u64) -> Result<bool, String> {
    ops::highlight_buffer(&state, buffer_id)
}

#[tauri::command]
pub async fn highlight_range(
    state: State<'_, AppState>,
    buffer_id: u64,
    start_line: usize,
    end_line: usize,
) -> Result<Vec<RenderedLine>, String> {
    ops::highlight_range(&state, buffer_id, start_line, end_line)
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
    ops::edit_buffer(&state, buffer_id, line, col, new_text, delete_count)
}

/// Format a buffer's content using the built-in formatter.
/// Returns the new line count if formatting changed anything, or None if no changes.
#[tauri::command]
pub async fn format_buffer(
    state: State<'_, AppState>,
    buffer_id: u64,
    indent_size: usize,
) -> Result<Option<usize>, String> {
    ops::format_buffer(&state, buffer_id, indent_size)
}

#[tauri::command]
pub async fn save_file(
    state: State<'_, AppState>,
    buffer_id: u64,
    force: Option<bool>,
) -> Result<(), String> {
    ops::save_file(&state, buffer_id, force)
}

#[tauri::command]
pub async fn buffer_external_change(
    state: State<'_, AppState>,
    buffer_id: u64,
) -> Result<bool, String> {
    ops::buffer_external_change(&state, buffer_id)
}

#[tauri::command]
pub async fn reload_buffer(
    state: State<'_, AppState>,
    buffer_id: u64,
) -> Result<BufferInfo, String> {
    ops::reload_buffer(&state, buffer_id)
}

#[tauri::command]
pub async fn undo_edit(state: State<'_, AppState>, buffer_id: u64) -> Result<EditResponse, String> {
    ops::undo_redo(&state, buffer_id, false)
}

#[tauri::command]
pub async fn redo_edit(state: State<'_, AppState>, buffer_id: u64) -> Result<EditResponse, String> {
    ops::undo_redo(&state, buffer_id, true)
}

#[tauri::command]
pub async fn close_buffer(state: State<'_, AppState>, buffer_id: u64) -> Result<(), String> {
    ops::close_buffer(&state, buffer_id)
}

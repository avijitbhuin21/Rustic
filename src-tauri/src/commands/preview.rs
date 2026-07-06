//! Preview commands — desktop adapters over `rustic_app::preview_ops`.
//! The desktop keeps its stricter read preconditions (existence check, 100MB
//! base64 cap) via the ops flags, plus the human-formatted size response.

use rustic_app::preview_ops::{self, FileBase64Response, HexChunkResponse};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct FileSizeResponse {
    pub size: u64,
    pub formatted: String,
}

/// Read a file and return its contents as base64-encoded string.
#[tauri::command]
pub async fn read_file_base64(path: String) -> Result<FileBase64Response, String> {
    preview_ops::read_file_base64(&path, true)
}

/// Write a base64-encoded payload to disk, replacing the file's contents.
/// Used by binary editors (XLSX preview) to persist changes, and by the
/// chat composer to persist pasted/attached images under `.rustic/uploaded/`
/// so the agent can reference them by path.
#[tauri::command]
pub async fn write_file_base64(path: String, data: String) -> Result<u64, String> {
    preview_ops::write_file_base64(&path, &data)
}

/// Read a chunk of a file as hex data for the hex viewer.
#[tauri::command]
pub async fn read_hex_chunk(
    path: String,
    offset: u64,
    length: usize,
) -> Result<HexChunkResponse, String> {
    preview_ops::read_hex_chunk(&path, offset, length)
}

/// Get file size info.
#[tauri::command]
pub async fn get_file_size(path: String) -> Result<FileSizeResponse, String> {
    let size = preview_ops::file_size(&path, true)?;
    let formatted = format_file_size(size);
    Ok(FileSizeResponse { size, formatted })
}

fn format_file_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

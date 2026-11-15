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

/// Runs blocking decoder work off the async runtime's worker threads.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())?
}

/// Streamed waveform + format facts for an audio file (constant memory).
#[tauri::command]
pub async fn audio_overview(
    path: String,
    buckets: Option<usize>,
) -> Result<rustic_app::audio_stream::AudioOverview, String> {
    blocking(move || rustic_app::audio_stream::audio_overview(&path, buckets.unwrap_or(4000))).await
}

/// PCM for a time window, resampled to the playback device rate.
#[tauri::command]
pub async fn audio_pcm_chunk(
    path: String,
    start: f64,
    seconds: f64,
    out_rate: Option<u32>,
) -> Result<rustic_app::audio_stream::PcmChunk, String> {
    blocking(move || {
        rustic_app::audio_stream::audio_pcm_chunk(&path, start, seconds, out_rate.unwrap_or(0))
    })
    .await
}

/// Decode HEIC/HEIF, TIFF, camera RAW or ICNS into web-displayable images.
#[tauri::command]
pub async fn decode_image_preview(
    path: String,
) -> Result<rustic_app::rich_previews::ImagePreview, String> {
    blocking(move || rustic_app::rich_previews::decode_image_preview(&path)).await
}

/// List the members of a zip-family, tar, tar.gz or gz archive.
#[tauri::command]
pub async fn archive_list(
    path: String,
) -> Result<rustic_app::rich_previews::ArchiveListing, String> {
    blocking(move || rustic_app::rich_previews::archive_list(&path)).await
}

/// Read one archive member's bytes (base64).
#[tauri::command]
pub async fn archive_read_entry(
    path: String,
    entry: String,
) -> Result<rustic_app::rich_previews::ArchiveEntryContent, String> {
    blocking(move || rustic_app::rich_previews::archive_read_entry(&path, &entry)).await
}

/// Tables/views of a SQLite file, opened read-only.
#[tauri::command]
pub async fn sqlite_schema(
    path: String,
) -> Result<Vec<rustic_app::rich_previews::SqliteTable>, String> {
    blocking(move || rustic_app::rich_previews::sqlite_schema(&path)).await
}

/// One page of rows from a table or a read-only query.
#[tauri::command]
pub async fn sqlite_rows(
    path: String,
    table: Option<String>,
    query: Option<String>,
    offset: Option<i64>,
    page_size: Option<i64>,
) -> Result<rustic_app::rich_previews::SqliteRows, String> {
    blocking(move || {
        rustic_app::rich_previews::sqlite_rows(
            &path,
            table.as_deref(),
            query.as_deref(),
            offset.unwrap_or(0),
            page_size.unwrap_or(200),
        )
    })
    .await
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

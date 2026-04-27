use base64::Engine as _;
use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::path_scope::validate_readable_path;

#[derive(Debug, Serialize, Deserialize)]
pub struct FileBase64Response {
    pub data: String,
    pub size: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HexChunkResponse {
    /// Hex-encoded bytes, each byte as two hex chars
    pub hex: Vec<String>,
    /// ASCII representation (printable chars or '.')
    pub ascii: Vec<String>,
    /// Offset of first byte in this chunk
    pub offset: u64,
    /// Total file size
    pub total_size: u64,
    /// Number of bytes actually read
    pub bytes_read: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FileSizeResponse {
    pub size: u64,
    pub formatted: String,
}

/// Read a file and return its contents as base64-encoded string.
#[tauri::command]
pub async fn read_file_base64(path: String) -> Result<FileBase64Response, String> {
    let file_path = Path::new(&path);
    validate_readable_path(file_path)?;
    if !file_path.exists() || !file_path.is_file() {
        return Err(format!("File does not exist: {}", file_path.display()));
    }

    let metadata = std::fs::metadata(file_path).map_err(|e| e.to_string())?;
    let size = metadata.len();

    // Limit to 100MB for base64 encoding
    if size > 100 * 1024 * 1024 {
        return Err("File too large for base64 preview (>100MB)".to_string());
    }

    let bytes = std::fs::read(file_path).map_err(|e| e.to_string())?;
    let data = base64::engine::general_purpose::STANDARD.encode(&bytes);

    Ok(FileBase64Response { data, size })
}

/// Read a chunk of a file as hex data for the hex viewer.
#[tauri::command]
pub async fn read_hex_chunk(
    path: String,
    offset: u64,
    length: usize,
) -> Result<HexChunkResponse, String> {
    use std::io::{Read, Seek, SeekFrom};

    let file_path = Path::new(&path);
    validate_readable_path(file_path)?;
    if !file_path.exists() || !file_path.is_file() {
        return Err(format!("File does not exist: {}", file_path.display()));
    }

    let metadata = std::fs::metadata(file_path).map_err(|e| e.to_string())?;
    let total_size = metadata.len();

    // Cap length at 64KB per chunk
    let length = length.min(65536);

    let mut file = std::fs::File::open(file_path).map_err(|e| e.to_string())?;
    file.seek(SeekFrom::Start(offset)).map_err(|e| e.to_string())?;

    let mut buf = vec![0u8; length];
    let bytes_read = file.read(&mut buf).map_err(|e| e.to_string())?;
    buf.truncate(bytes_read);

    let hex: Vec<String> = buf.iter().map(|b| format!("{:02x}", b)).collect();
    let ascii: Vec<String> = buf
        .iter()
        .map(|&b| {
            if b >= 0x20 && b <= 0x7e {
                String::from(b as char)
            } else {
                ".".to_string()
            }
        })
        .collect();

    Ok(HexChunkResponse {
        hex,
        ascii,
        offset,
        total_size,
        bytes_read,
    })
}

/// Get file size info.
#[tauri::command]
pub async fn get_file_size(path: String) -> Result<FileSizeResponse, String> {
    let file_path = Path::new(&path);
    validate_readable_path(file_path)?;
    if !file_path.exists() || !file_path.is_file() {
        return Err(format!("File does not exist: {}", file_path.display()));
    }

    let metadata = std::fs::metadata(file_path).map_err(|e| e.to_string())?;
    let size = metadata.len();

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

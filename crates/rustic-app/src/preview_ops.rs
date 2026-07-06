//! Shared preview command bodies: large/binary file reads (base64, hex, size)
//! and base64 writes. The transport-agnostic core behind
//! `src-tauri/src/commands/preview.rs` and
//! `rustic-server/src/commands/preview.rs`.
//!
//! Historical divergence, preserved deliberately: the desktop applies
//! preview-oriented preconditions on reads (explicit "file does not exist"
//! errors, a 100MB base64-read cap) that the server build never had. The
//! `require_existing_file` / `enforce_preview_limits` flags keep each host's
//! error strings byte-identical to what its frontend already handles.

use std::path::Path;

use base64::Engine as _;
use serde::{Deserialize, Serialize};

use crate::path_scope::{validate_readable_path, validate_writable_path};

/// Cap for base64 reads (desktop preview) and all base64 writes.
const MAX_BASE64_BYTES: u64 = 100 * 1024 * 1024;

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

fn ensure_existing_file(p: &Path) -> Result<(), String> {
    if !p.exists() || !p.is_file() {
        return Err(format!("File does not exist: {}", p.display()));
    }
    Ok(())
}

/// File size in bytes. With `require_existing_file` a missing path yields the
/// desktop's "File does not exist: …" error; without it (server) the raw
/// `fs::metadata` io error surfaces.
pub fn file_size(path: &str, require_existing_file: bool) -> Result<u64, String> {
    let p = Path::new(path);
    validate_readable_path(p)?;
    if require_existing_file {
        ensure_existing_file(p)?;
    }
    let meta = std::fs::metadata(p).map_err(|e| e.to_string())?;
    Ok(meta.len())
}

/// Read a file and return its contents as a base64-encoded string.
/// `enforce_preview_limits` adds the desktop's existence check and 100MB cap.
pub fn read_file_base64(
    path: &str,
    enforce_preview_limits: bool,
) -> Result<FileBase64Response, String> {
    let p = Path::new(path);
    validate_readable_path(p)?;

    if enforce_preview_limits {
        ensure_existing_file(p)?;
        let size = std::fs::metadata(p).map_err(|e| e.to_string())?.len();
        if size > MAX_BASE64_BYTES {
            return Err("File too large for base64 preview (>100MB)".to_string());
        }
    }

    let bytes = std::fs::read(p).map_err(|e| e.to_string())?;
    let size = bytes.len() as u64;
    let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Ok(FileBase64Response { data, size })
}

/// Write a base64-encoded payload to disk, replacing the file's contents.
/// Used by binary editors (XLSX preview) to persist changes, and by the
/// chat composer to persist pasted/attached images under `.rustic/uploaded/`
/// so the agent can reference them by path. Returns the byte count written.
pub fn write_file_base64(path: &str, data: &str) -> Result<u64, String> {
    let file_path = Path::new(path);
    validate_writable_path(file_path)?;

    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data.as_bytes())
        .map_err(|e| format!("invalid base64: {e}"))?;

    if bytes.len() as u64 > MAX_BASE64_BYTES {
        return Err("Refusing to write file larger than 100MB".to_string());
    }

    // Make sure the destination directory exists. This lets callers write to
    // paths like `<project>/.rustic/uploaded/<task>/file.png` without having
    // to pre-create the per-task directory.
    if let Some(parent) = file_path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
    }

    std::fs::write(file_path, &bytes).map_err(|e| e.to_string())?;
    Ok(bytes.len() as u64)
}

/// Read a chunk of a file as hex data for the hex viewer.
pub fn read_hex_chunk(path: &str, offset: u64, length: usize) -> Result<HexChunkResponse, String> {
    use std::io::{Read, Seek, SeekFrom};

    let file_path = Path::new(path);
    validate_readable_path(file_path)?;
    ensure_existing_file(file_path)?;

    let metadata = std::fs::metadata(file_path).map_err(|e| e.to_string())?;
    let total_size = metadata.len();

    // Cap length at 64KB per chunk
    let length = length.min(65536);

    let mut file = std::fs::File::open(file_path).map_err(|e| e.to_string())?;
    file.seek(SeekFrom::Start(offset))
        .map_err(|e| e.to_string())?;

    let mut buf = vec![0u8; length];
    let bytes_read = file.read(&mut buf).map_err(|e| e.to_string())?;
    buf.truncate(bytes_read);

    let hex: Vec<String> = buf.iter().map(|b| format!("{:02x}", b)).collect();
    let ascii: Vec<String> = buf
        .iter()
        .map(|&b| {
            if (0x20..=0x7e).contains(&b) {
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

//! Preview commands: large/binary file reads (base64, hex, size).

use std::path::Path;

use serde_json::Value;

use rustic_app::path_scope::{validate_readable_path, validate_writable_path};

use crate::api::{ok, parse, ApiError, PathArg};
use crate::context::ServerContext;

pub async fn dispatch(
    _ctx: &ServerContext,
    command: &str,
    args: &Value,
) -> Option<Result<Value, ApiError>> {
    Some(match command {
        "get_file_size" => match parse::<PathArg>(args) {
            Ok(a) => get_file_size(a.path),
            Err(e) => Err(e),
        },
        "read_file_base64" => match parse::<PathArg>(args) {
            Ok(a) => read_file_base64(a.path),
            Err(e) => Err(e),
        },
        "write_file_base64" => write_file_base64(args),
        "read_hex_chunk" => read_hex_chunk(args),
        _ => return None,
    })
}

fn get_file_size(path: String) -> Result<Value, ApiError> {
    let p = Path::new(&path);
    validate_readable_path(p)?;
    let meta = std::fs::metadata(p).map_err(|e| e.to_string())?;
    ok(meta.len())
}

fn read_file_base64(path: String) -> Result<Value, ApiError> {
    use base64::Engine;
    let p = Path::new(&path);
    validate_readable_path(p)?;
    let bytes = std::fs::read(p).map_err(|e| e.to_string())?;
    ok(base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// Write a base64-encoded payload to disk, replacing the file's contents.
/// Mirrors the desktop `write_file_base64`.
fn write_file_base64(args: &Value) -> Result<Value, ApiError> {
    use base64::Engine;
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct A {
        path: String,
        data: String,
    }
    let a: A = parse(args)?;
    let file_path = Path::new(&a.path);
    validate_writable_path(file_path)?;

    let bytes = base64::engine::general_purpose::STANDARD
        .decode(a.data.as_bytes())
        .map_err(|e| format!("invalid base64: {e}"))?;

    if bytes.len() as u64 > 100 * 1024 * 1024 {
        return Err(ApiError::bad("Refusing to write file larger than 100MB"));
    }

    // Ensure the destination directory exists.
    if let Some(parent) = file_path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
    }

    std::fs::write(file_path, &bytes).map_err(|e| e.to_string())?;
    ok(bytes.len() as u64)
}

#[derive(serde::Serialize)]
struct HexChunkResponse {
    hex: Vec<String>,
    ascii: Vec<String>,
    offset: u64,
    total_size: u64,
    bytes_read: usize,
}

/// Read a chunk of a file as hex data for the hex viewer. Mirrors the desktop
/// `read_hex_chunk`.
fn read_hex_chunk(args: &Value) -> Result<Value, ApiError> {
    use std::io::{Read, Seek, SeekFrom};

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct A {
        path: String,
        offset: u64,
        length: usize,
    }
    let a: A = parse(args)?;

    let file_path = Path::new(&a.path);
    validate_readable_path(file_path)?;
    if !file_path.exists() || !file_path.is_file() {
        return Err(ApiError::bad(format!(
            "File does not exist: {}",
            file_path.display()
        )));
    }

    let metadata = std::fs::metadata(file_path).map_err(|e| e.to_string())?;
    let total_size = metadata.len();

    // Cap length at 64KB per chunk.
    let length = a.length.min(65536);

    let mut file = std::fs::File::open(file_path).map_err(|e| e.to_string())?;
    file.seek(SeekFrom::Start(a.offset)).map_err(|e| e.to_string())?;

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

    ok(HexChunkResponse {
        hex,
        ascii,
        offset: a.offset,
        total_size,
        bytes_read,
    })
}

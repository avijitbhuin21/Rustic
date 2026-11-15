//! Preview commands: large/binary file reads (base64, hex, size). The bodies
//! live in `rustic_app::preview_ops`; the server historically skips the
//! desktop's preview-read preconditions (existence message, 100MB read cap)
//! and returns the bare size for `get_file_size`, so the flags below preserve
//! that wire behavior exactly.

use serde_json::Value;

use rustic_app::preview_ops;

use crate::api::{ok, parse, ApiError, PathArg};
use crate::context::ServerContext;

pub async fn dispatch(
    _ctx: &ServerContext,
    command: &str,
    args: &Value,
) -> Option<Result<Value, ApiError>> {
    Some(match command {
        "get_file_size" => match parse::<PathArg>(args) {
            Ok(a) => preview_ops::file_size(&a.path, false)
                .map_err(ApiError::from)
                .and_then(ok),
            Err(e) => Err(e),
        },
        "read_file_base64" => match parse::<PathArg>(args) {
            Ok(a) => preview_ops::read_file_base64(&a.path, false)
                .map_err(ApiError::from)
                .and_then(ok),
            Err(e) => Err(e),
        },
        "write_file_base64" => write_file_base64(args),
        "read_hex_chunk" => read_hex_chunk(args),
        "decode_image_preview"
        | "archive_list"
        | "archive_read_entry"
        | "sqlite_schema"
        | "sqlite_rows"
        | "audio_overview"
        | "audio_pcm_chunk" => rich_preview(command, args).await,
        _ => return None,
    })
}

/// Rich previews (HEIC/RAW/TIFF/ICNS, archives, SQLite) — same shared
/// decoders as desktop, run on a blocking thread.
async fn rich_preview(command: &str, args: &Value) -> Result<Value, ApiError> {
    use rustic_app::rich_previews as rp;
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct A {
        path: String,
        entry: Option<String>,
        table: Option<String>,
        query: Option<String>,
        offset: Option<i64>,
        page_size: Option<i64>,
        buckets: Option<usize>,
        start: Option<f64>,
        seconds: Option<f64>,
        out_rate: Option<u32>,
    }
    let a: A = parse(args)?;
    let command = command.to_string();
    fn json<T: serde::Serialize>(v: Result<T, String>) -> Result<Value, String> {
        v.and_then(|x| serde_json::to_value(x).map_err(|e| e.to_string()))
    }
    let out = tokio::task::spawn_blocking(move || -> Result<Value, String> {
        match command.as_str() {
            "decode_image_preview" => json(rp::decode_image_preview(&a.path)),
            "archive_list" => json(rp::archive_list(&a.path)),
            "archive_read_entry" => json(rp::archive_read_entry(
                &a.path,
                a.entry.as_deref().ok_or("entry is required")?,
            )),
            "sqlite_schema" => json(rp::sqlite_schema(&a.path)),
            "audio_overview" => json(rustic_app::audio_stream::audio_overview(
                &a.path,
                a.buckets.unwrap_or(4000),
            )),
            "audio_pcm_chunk" => json(rustic_app::audio_stream::audio_pcm_chunk(
                &a.path,
                a.start.unwrap_or(0.0),
                a.seconds.unwrap_or(4.0),
                a.out_rate.unwrap_or(0),
            )),
            _ => json(rp::sqlite_rows(
                &a.path,
                a.table.as_deref(),
                a.query.as_deref(),
                a.offset.unwrap_or(0),
                a.page_size.unwrap_or(200),
            )),
        }
    })
    .await
    .map_err(|e| ApiError::from(e.to_string()))?;
    ok(out?)
}

fn write_file_base64(args: &Value) -> Result<Value, ApiError> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct A {
        path: String,
        data: String,
    }
    let a: A = parse(args)?;
    ok(preview_ops::write_file_base64(&a.path, &a.data)?)
}

fn read_hex_chunk(args: &Value) -> Result<Value, ApiError> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct A {
        path: String,
        offset: u64,
        length: usize,
    }
    let a: A = parse(args)?;
    ok(preview_ops::read_hex_chunk(&a.path, a.offset, a.length)?)
}

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
        _ => return None,
    })
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

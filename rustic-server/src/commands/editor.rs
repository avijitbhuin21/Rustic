//! Editor commands — server dispatch. Bodies live in
//! `rustic_app::host_ops::editor`, shared with the desktop host.

use serde::Deserialize;
use serde_json::Value;

use rustic_app::context::AppContext;
use rustic_app::host_ops::editor as ops;

use crate::api::{ok, parse, ApiError, PathArg};
use crate::context::ServerContext;

pub async fn dispatch(
    ctx: &ServerContext,
    command: &str,
    args: &Value,
) -> Option<Result<Value, ApiError>> {
    Some(match command {
        "open_file" => match parse::<PathArg>(args) {
            Ok(a) => wrap(ops::open_file(ctx.state(), &a.path)),
            Err(e) => Err(e),
        },
        "open_scratch_buffer" => open_scratch_buffer(ctx, args),
        "get_visible_lines" => get_visible_lines(ctx, args),
        "highlight_buffer" => by_id(args, |id| wrap(ops::highlight_buffer(ctx.state(), id))),
        "highlight_range" => highlight_range(ctx, args),
        "edit_buffer" => edit_buffer(ctx, args),
        "format_buffer" => format_buffer(ctx, args),
        "save_file" => save_file(ctx, args),
        "buffer_external_change" => by_id(args, |id| {
            wrap(ops::buffer_external_change(ctx.state(), id))
        }),
        "reload_buffer" => by_id(args, |id| wrap(ops::reload_buffer(ctx.state(), id))),
        "undo_edit" => by_id(args, |id| wrap(ops::undo_redo(ctx.state(), id, false))),
        "redo_edit" => by_id(args, |id| wrap(ops::undo_redo(ctx.state(), id, true))),
        "close_buffer" => by_id(args, |id| {
            ops::close_buffer(ctx.state(), id)?;
            ok(serde_json::json!(null))
        }),
        _ => return None,
    })
}

fn wrap<T: serde::Serialize>(r: Result<T, String>) -> Result<Value, ApiError> {
    ok(r?)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BufferIdArg {
    buffer_id: u64,
}

fn by_id(args: &Value, f: impl FnOnce(u64) -> Result<Value, ApiError>) -> Result<Value, ApiError> {
    let a: BufferIdArg = parse(args)?;
    f(a.buffer_id)
}

fn open_scratch_buffer(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct A {
        title: String,
        content: String,
        language: Option<String>,
    }
    let a: A = parse(args)?;
    wrap(ops::open_scratch_buffer(
        ctx.state(),
        &a.title,
        &a.content,
        a.language,
    ))
}

fn get_visible_lines(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct A {
        buffer_id: u64,
        start: usize,
        end: usize,
    }
    let a: A = parse(args)?;
    wrap(ops::get_visible_lines(
        ctx.state(),
        a.buffer_id,
        a.start,
        a.end,
    ))
}

fn highlight_range(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct A {
        buffer_id: u64,
        start_line: usize,
        end_line: usize,
    }
    let a: A = parse(args)?;
    wrap(ops::highlight_range(
        ctx.state(),
        a.buffer_id,
        a.start_line,
        a.end_line,
    ))
}

fn edit_buffer(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct A {
        buffer_id: u64,
        line: usize,
        col: usize,
        new_text: String,
        delete_count: usize,
    }
    let a: A = parse(args)?;
    wrap(ops::edit_buffer(
        ctx.state(),
        a.buffer_id,
        a.line,
        a.col,
        a.new_text,
        a.delete_count,
    ))
}

fn format_buffer(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct A {
        buffer_id: u64,
        indent_size: usize,
    }
    let a: A = parse(args)?;
    wrap(ops::format_buffer(ctx.state(), a.buffer_id, a.indent_size))
}

fn save_file(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct A {
        buffer_id: u64,
        force: Option<bool>,
    }
    let a: A = parse(args)?;
    ops::save_file(ctx.state(), a.buffer_id, a.force)?;
    ok(serde_json::json!(null))
}

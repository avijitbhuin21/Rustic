//! Notebook kernel commands — server dispatch. Thin wrapper over the shared
//! core in `rustic_app::notebook_kernel`; replies broadcast onto the WS hub
//! as `notebook-kernel-output`, byte-identical to the desktop event shape.
//! Mirrors `src-tauri/src/commands/notebook_kernel.rs` — keep in sync.

use std::sync::Arc;

use serde::Deserialize;
use serde_json::Value;

use rustic_app::context::EventEmitterExt;
use rustic_app::notebook_kernel as core;

use crate::api::{ok, parse, ApiError};
use crate::context::ServerContext;

pub async fn dispatch(
    ctx: &ServerContext,
    command: &str,
    args: &Value,
) -> Option<Result<Value, ApiError>> {
    Some(match command {
        "notebook_kernel_start" => kernel_start(ctx, args),
        "notebook_kernel_exec" => kernel_exec(args),
        "notebook_kernel_stop" => kernel_stop(args),
        _ => return None,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartArg {
    notebook_id: String,
    cwd: String,
}

/// Starts (or restarts) the notebook's Python kernel, emitting replies on the WS hub.
fn kernel_start(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: StartArg = parse(args)?;
    let emit_ctx = ctx.clone();
    let emit: core::KernelEmit = Arc::new(move |ev: core::KernelEvent| {
        emit_ctx.emit("notebook-kernel-output", ev);
    });
    let python = core::start(&a.notebook_id, &a.cwd, emit)?;
    ok(python)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExecArg {
    notebook_id: String,
    cell_id: String,
    code: String,
}

/// Sends a cell's code to the notebook's kernel.
fn kernel_exec(args: &Value) -> Result<Value, ApiError> {
    let a: ExecArg = parse(args)?;
    core::exec(&a.notebook_id, &a.cell_id, &a.code)?;
    ok(serde_json::json!(null))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StopArg {
    notebook_id: String,
}

/// Stops the notebook's kernel.
fn kernel_stop(args: &Value) -> Result<Value, ApiError> {
    let a: StopArg = parse(args)?;
    core::stop(&a.notebook_id);
    ok(serde_json::json!(null))
}

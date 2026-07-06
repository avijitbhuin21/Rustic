//! Search commands — server dispatch. The bodies live in
//! `rustic_app::search_ops`; this module only parses the wire args and
//! publishes streaming `FileMatch`/`Completed` events on the WS hub via the
//! `ServerContext` emitter instead of Tauri's `AppHandle::emit`. Payload
//! shapes are identical to desktop.

use std::sync::Arc;

use serde::Deserialize;
use serde_json::{json, Value};

use rustic_app::context::{AppContext, EventEmitter};
use rustic_app::search_ops::{self, FileReplacePlan, SearchParams};

use crate::api::{ok, parse, ApiError};
use crate::context::ServerContext;

pub async fn dispatch(
    ctx: &ServerContext,
    command: &str,
    args: &Value,
) -> Option<Result<Value, ApiError>> {
    Some(match command {
        "start_search" => start_search(ctx, args),
        "cancel_search" => {
            search_ops::cancel_search(ctx.state());
            ok(json!(null))
        }
        "replace_in_file" => replace_in_file(args),
        "replace_all_in_files" => replace_all_in_files(args),
        _ => return None,
    })
}

fn start_search(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let params: SearchParams = parse(args)?;
    // Clone the context so the spawned search can publish events on the hub.
    let emitter: Arc<dyn EventEmitter> = Arc::new(ctx.clone());
    ok(search_ops::start_search(ctx.state(), emitter, params))
}

fn replace_in_file(args: &Value) -> Result<Value, ApiError> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct A {
        path: String,
        pattern: String,
        replacement: String,
        is_regex: bool,
        case_sensitive: bool,
        whole_word: bool,
    }
    let a: A = parse(args)?;
    let result = search_ops::replace_in_file(
        &a.path,
        &a.pattern,
        &a.replacement,
        a.is_regex,
        a.case_sensitive,
        a.whole_word,
    )?;
    ok(result)
}

fn replace_all_in_files(args: &Value) -> Result<Value, ApiError> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct A {
        plans: Vec<FileReplacePlan>,
        pattern: String,
        replacement: String,
        is_regex: bool,
        case_sensitive: bool,
        whole_word: bool,
    }
    let a: A = parse(args)?;
    ok(search_ops::replace_all_in_files(
        a.plans,
        &a.pattern,
        &a.replacement,
        a.is_regex,
        a.case_sensitive,
        a.whole_word,
    ))
}

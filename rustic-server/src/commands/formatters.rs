//! formatters commands — server dispatch.
//!
//! All logic lives in `rustic_app::host_ops::formatters` (shared with the
//! desktop host); this file only parses arguments and maps errors.

use serde::Deserialize;
use serde_json::Value;

use rustic_app::context::AppContext;
use rustic_app::host_ops::formatters::{self as ops, CustomFormatter, FormatRequest};

use crate::api::{ok, parse, ApiError};
use crate::context::ServerContext;

pub async fn dispatch(
    ctx: &ServerContext,
    command: &str,
    args: &Value,
) -> Option<Result<Value, ApiError>> {
    Some(match command {
        "formatter_registry" => ok(ops::registry()),
        "formatter_list" => run(ops::list(ctx.state(), &ctx.data_dir)),
        "formatter_uninstall" => match parse::<IdArg>(args) {
            Ok(a) => run(ops::uninstall(ctx.state(), &ctx.data_dir, &a.id)),
            Err(e) => Err(e),
        },
        "formatter_add_custom" => match parse::<FormatterArg>(args) {
            Ok(a) => run(ops::add_custom(ctx.state(), a.formatter)),
            Err(e) => Err(e),
        },
        "formatter_update_custom" => match parse::<FormatterArg>(args) {
            Ok(a) => run(ops::update_custom(ctx.state(), a.formatter)),
            Err(e) => Err(e),
        },
        "formatter_remove_custom" => match parse::<IdArg>(args) {
            Ok(a) => run(ops::remove_custom(ctx.state(), &a.id)),
            Err(e) => Err(e),
        },
        "formatter_format" => match parse::<FormatReqArg>(args) {
            Ok(a) => run(ops::format(ctx.state(), &ctx.data_dir, a.req).await),
            Err(e) => Err(e),
        },
        // Update = install: always fetches the latest release and overwrites.
        "formatter_install" | "formatter_update" => match parse::<IdArg>(args) {
            Ok(a) => run(ops::install(ctx.state(), &ctx.data_dir, &a.id).await),
            Err(e) => Err(e),
        },
        "formatter_check_update" => match parse::<IdArg>(args) {
            Ok(a) => run(ops::check_update(&a.id).await),
            Err(e) => Err(e),
        },
        _ => return None,
    })
}

fn run<T: serde::Serialize>(r: Result<T, String>) -> Result<Value, ApiError> {
    ok(r?)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdArg {
    id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FormatterArg {
    formatter: CustomFormatter,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FormatReqArg {
    req: FormatRequest,
}

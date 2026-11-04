//! workflows commands — server dispatch over `rustic_app::host_ops::workflows`.

use serde::Deserialize;
use serde_json::Value;

use rustic_app::host_ops::workflows as ops;

use super::rules::{handle, reply};
use crate::api::{ok, parse, ApiError};
use crate::context::ServerContext;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NameArg {
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateArg {
    name: String,
    body: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateArg {
    original_name: String,
    name: String,
    body: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SourceArg {
    source: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SourcePathArg {
    source: String,
    path: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct InstallArg {
    source: String,
    paths: Vec<String>,
    names: Option<Vec<String>>,
}

pub async fn dispatch(
    _ctx: &ServerContext,
    command: &str,
    args: &Value,
) -> Option<Result<Value, ApiError>> {
    Some(match command {
        "list_workflows" => ok(ops::list_workflows()),
        "get_workflow_body" => handle(args, |a: NameArg| ops::get_workflow_body(&a.name)),
        "create_workflow" => handle(args, |a: CreateArg| ops::create_workflow(&a.name, &a.body)),
        "update_workflow" => handle(args, |a: UpdateArg| {
            ops::update_workflow(&a.original_name, &a.name, &a.body)
        }),
        "delete_workflow" => handle(args, |a: NameArg| ops::delete_workflow(&a.name)),
        "list_repo_workflows" => match parse::<SourceArg>(args) {
            Ok(a) => reply(ops::list_repo_workflows(&a.source).await),
            Err(e) => Err(e),
        },
        "preview_repo_workflow" => match parse::<SourcePathArg>(args) {
            Ok(a) => reply(ops::preview_repo_workflow(&a.source, &a.path).await),
            Err(e) => Err(e),
        },
        "install_repo_workflows" => match parse::<InstallArg>(args) {
            Ok(a) => reply(ops::install_repo_workflows(&a.source, &a.paths, a.names.as_deref()).await),
            Err(e) => Err(e),
        },
        _ => return None,
    })
}

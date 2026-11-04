//! skills commands — server dispatch over `rustic_app::host_ops::skills`.

use serde::Deserialize;
use serde_json::Value;

use rustic_app::host_ops::skills as ops;

use super::rules::{handle, reply};
use crate::api::ApiError;
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
        "list_skills" => crate::api::ok(ops::list_skills()),
        "get_skill_body" => handle(args, |a: NameArg| ops::get_skill_body(&a.name)),
        "create_skill" => handle(args, |a: CreateArg| ops::create_skill(&a.name, &a.body)),
        "update_skill" => handle(args, |a: UpdateArg| {
            ops::update_skill(&a.original_name, &a.name, &a.body)
        }),
        "delete_skill" => handle(args, |a: NameArg| ops::delete_skill(&a.name)),
        "list_repo_skills" => match crate::api::parse::<SourceArg>(args) {
            Ok(a) => reply(ops::list_repo_skills(&a.source).await),
            Err(e) => Err(e),
        },
        "preview_repo_skill" => match crate::api::parse::<SourcePathArg>(args) {
            Ok(a) => reply(ops::preview_repo_skill(&a.source, &a.path).await),
            Err(e) => Err(e),
        },
        "install_repo_skills" => match crate::api::parse::<InstallArg>(args) {
            Ok(a) => reply(ops::install_repo_skills(&a.source, &a.paths, a.names.as_deref()).await),
            Err(e) => Err(e),
        },
        _ => return None,
    })
}

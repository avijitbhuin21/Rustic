//! rules commands — server dispatch over `rustic_app::host_ops::rules`.

use serde::Deserialize;
use serde_json::Value;

use rustic_app::host_ops::rules as ops;

use crate::api::{ok, parse, ApiError};
use crate::context::ServerContext;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListArg {
    project_root: Option<String>,
}

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
struct ActivationArg {
    name: String,
    state: String,
    project_root: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectsArg {
    name: String,
    project_roots: Vec<String>,
}

pub async fn dispatch(
    _ctx: &ServerContext,
    command: &str,
    args: &Value,
) -> Option<Result<Value, ApiError>> {
    Some(match command {
        "list_rules" => handle(args, |a: ListArg| Ok(ops::list_rules(a.project_root.as_deref()))),
        "get_rule_body" => handle(args, |a: NameArg| ops::get_rule_body(&a.name)),
        "create_rule" => handle(args, |a: CreateArg| ops::create_rule(&a.name, &a.body)),
        "update_rule" => handle(args, |a: UpdateArg| {
            ops::update_rule(&a.original_name, &a.name, &a.body)
        }),
        "delete_rule" => handle(args, |a: NameArg| ops::delete_rule(&a.name)),
        "set_rule_activation" => handle(args, |a: ActivationArg| {
            ops::set_rule_activation(&a.name, &a.state, a.project_root.as_deref())
        }),
        "set_rule_projects" => handle(args, |a: ProjectsArg| {
            ops::set_rule_projects(&a.name, a.project_roots)
        }),
        _ => return None,
    })
}

/// Serialize a host-op result (for async arms that can't use `handle`).
pub(super) fn reply<T: serde::Serialize>(r: Result<T, String>) -> Result<Value, ApiError> {
    ok(r?)
}

/// Parse `args` into `A`, run `f`, serialize the result. Shared by the
/// thin library adapters (skills/workflows/rules).
pub(super) fn handle<A, T, F>(args: &Value, f: F) -> Result<Value, ApiError>
where
    A: serde::de::DeserializeOwned,
    T: serde::Serialize,
    F: FnOnce(A) -> Result<T, String>,
{
    ok(f(parse(args)?)?)
}

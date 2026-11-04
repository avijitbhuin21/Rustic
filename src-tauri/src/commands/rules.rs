//! Global rules — desktop adapter over `rustic_app::host_ops::rules`.

use rustic_app::host_ops::rules as ops;

pub use ops::RuleInfo;

#[tauri::command(async)]
pub fn list_rules(project_root: Option<String>) -> Result<Vec<RuleInfo>, String> {
    Ok(ops::list_rules(project_root.as_deref()))
}

#[tauri::command(async)]
pub fn get_rule_body(name: String) -> Result<String, String> {
    ops::get_rule_body(&name)
}

#[tauri::command(async)]
pub fn create_rule(name: String, body: String) -> Result<RuleInfo, String> {
    ops::create_rule(&name, &body)
}

#[tauri::command(async)]
pub fn update_rule(original_name: String, name: String, body: String) -> Result<RuleInfo, String> {
    ops::update_rule(&original_name, &name, &body)
}

#[tauri::command(async)]
pub fn delete_rule(name: String) -> Result<(), String> {
    ops::delete_rule(&name)
}

#[tauri::command(async)]
pub fn set_rule_activation(
    name: String,
    state: String,
    project_root: Option<String>,
) -> Result<RuleInfo, String> {
    ops::set_rule_activation(&name, &state, project_root.as_deref())
}

#[tauri::command(async)]
pub fn set_rule_projects(name: String, project_roots: Vec<String>) -> Result<RuleInfo, String> {
    ops::set_rule_projects(&name, project_roots)
}

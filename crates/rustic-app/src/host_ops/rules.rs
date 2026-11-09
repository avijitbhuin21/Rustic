//! Global rules (markdown files under `~/.rustic/rules`) — shared by both hosts.

use std::path::{Path, PathBuf};

use rustic_agent::{
    discover_global_rules, forget_rule, global_rules_dir, rule_active_projects, rule_body,
    set_rule_projects as agent_set_rule_projects, set_rule_state, RuleDef, RuleState,
};
use serde::Serialize;

use crate::path_scope::validate_simple_name;

#[derive(Clone, Serialize)]
pub struct RuleInfo {
    pub name: String,
    pub description: String,
    /// "inactive" | "global" | "project"
    pub state: String,
    /// Project keys (forward-slash root paths) where this rule is active.
    /// Always populated so the UI can pre-fill the project picker.
    pub active_projects: Vec<String>,
}

fn state_to_string(s: RuleState) -> String {
    match s {
        RuleState::Inactive => "inactive",
        RuleState::Global => "global",
        RuleState::Project => "project",
    }
    .to_string()
}

fn parse_state(s: &str) -> Result<RuleState, String> {
    match s {
        "inactive" => Ok(RuleState::Inactive),
        "global" => Ok(RuleState::Global),
        "project" => Ok(RuleState::Project),
        other => Err(format!("Invalid rule state: {}", other)),
    }
}

fn rules_root() -> Result<PathBuf, String> {
    let dir = global_rules_dir().ok_or_else(|| "Could not resolve home directory".to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

pub(crate) use super::library::{sanitize_name, summarize};

fn rule_file_content(name: &str, description: &str, body: &str) -> String {
    super::library::with_frontmatter(name, description, body)
}

fn to_info(rule: &RuleDef, project_root: Option<&Path>) -> RuleInfo {
    let state = match project_root {
        Some(root) => rustic_agent::rule_state(&rule.name, root),
        None => {
            let s = rustic_agent::load_rules_state();
            if s.active_global.iter().any(|n| n == &rule.name) {
                RuleState::Global
            } else if !rule_active_projects(&rule.name).is_empty() {
                RuleState::Project
            } else {
                RuleState::Inactive
            }
        }
    };
    RuleInfo {
        name: rule.name.clone(),
        description: rule.description.clone(),
        state: state_to_string(state),
        active_projects: rule_active_projects(&rule.name),
    }
}

fn find_rule(name: &str) -> Result<RuleDef, String> {
    discover_global_rules()
        .into_iter()
        .find(|r| r.name == name)
        .ok_or_else(|| format!("Rule not found: {}", name))
}

pub fn list_rules(project_root: Option<&str>) -> Vec<RuleInfo> {
    let root = project_root.map(Path::new);
    discover_global_rules()
        .iter()
        .map(|r| to_info(r, root))
        .collect()
}

pub fn get_rule_body(name: &str) -> Result<String, String> {
    validate_simple_name(name)?;
    let rule = find_rule(name)?;
    let content = std::fs::read_to_string(&rule.path).map_err(|e| e.to_string())?;
    Ok(rule_body(&content).to_string())
}

pub fn create_rule(name: &str, body: &str) -> Result<RuleInfo, String> {
    let root = rules_root()?;
    let safe_name = sanitize_name(name);
    if safe_name.is_empty() {
        return Err("Invalid rule name".to_string());
    }
    let rule_path = root.join(format!("{}.md", safe_name));
    if rule_path.exists() {
        return Err(format!("Rule already exists: {}", safe_name));
    }
    let description = summarize(body);
    rustic_core::io_util::atomic_write(
        &rule_path,
        rule_file_content(&safe_name, &description, body).as_bytes(),
    )
    .map_err(|e| e.to_string())?;
    Ok(RuleInfo {
        name: safe_name,
        description,
        state: "inactive".to_string(),
        active_projects: Vec::new(),
    })
}

pub fn update_rule(original_name: &str, name: &str, body: &str) -> Result<RuleInfo, String> {
    validate_simple_name(original_name)?;
    let root = rules_root()?;
    let original_path = root.join(format!("{}.md", original_name));
    if !original_path.exists() {
        return Err(format!("Rule not found: {}", original_name));
    }
    let new_safe_name = sanitize_name(name);
    if new_safe_name.is_empty() {
        return Err("Invalid rule name".to_string());
    }
    validate_simple_name(&new_safe_name)?;

    let final_path = if new_safe_name != original_name {
        let target = root.join(format!("{}.md", new_safe_name));
        if target.exists() {
            return Err(format!("Rule already exists: {}", new_safe_name));
        }
        std::fs::rename(&original_path, &target).map_err(|e| e.to_string())?;
        // Migrate activation state under the new name.
        let mut state = rustic_agent::load_rules_state();
        let was_global = state.active_global.iter().any(|n| n == original_name);
        state.active_global.retain(|n| n != original_name);
        if was_global {
            state.active_global.push(new_safe_name.clone());
        }
        for list in state.active_project.values_mut() {
            let had = list.iter().any(|n| n == original_name);
            list.retain(|n| n != original_name);
            if had {
                list.push(new_safe_name.clone());
            }
        }
        rustic_agent::rules::save_rules_state(&state)?;
        target
    } else {
        original_path
    };

    let description = summarize(body);
    rustic_core::io_util::atomic_write(
        &final_path,
        rule_file_content(&new_safe_name, &description, body).as_bytes(),
    )
    .map_err(|e| e.to_string())?;

    let s = rustic_agent::load_rules_state();
    let state_str = if s.active_global.iter().any(|n| n == &new_safe_name) {
        "global"
    } else if s
        .active_project
        .values()
        .any(|v| v.iter().any(|n| n == &new_safe_name))
    {
        // Best-effort display when project_root is unknown.
        "project"
    } else {
        "inactive"
    };
    Ok(RuleInfo {
        active_projects: rule_active_projects(&new_safe_name),
        name: new_safe_name,
        description,
        state: state_str.to_string(),
    })
}

pub fn delete_rule(name: &str) -> Result<(), String> {
    validate_simple_name(name)?;
    let root = rules_root()?;
    let path = root.join(format!("{}.md", name));
    if !path.exists() {
        return Err(format!("Rule not found: {}", name));
    }
    let canon_root = root.canonicalize().map_err(|e| e.to_string())?;
    let canon_path = path.canonicalize().map_err(|e| e.to_string())?;
    if !canon_path.starts_with(&canon_root) {
        return Err("Refusing to delete path outside rules root".to_string());
    }
    std::fs::remove_file(&canon_path).map_err(|e| e.to_string())?;
    let _ = forget_rule(name);
    Ok(())
}

pub fn set_rule_activation(
    name: &str,
    state: &str,
    project_root: Option<&str>,
) -> Result<RuleInfo, String> {
    validate_simple_name(name)?;
    let new_state = parse_state(state)?;
    if matches!(new_state, RuleState::Project) && project_root.is_none() {
        return Err("Cannot set rule as project-active without an open project".to_string());
    }
    let root_path = match project_root {
        Some(p) => PathBuf::from(p),
        None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    };
    set_rule_state(name, new_state, &root_path)?;
    Ok(to_info(&find_rule(name)?, Some(&root_path)))
}

/// Replace the rule's project membership wholesale. Empty list disables it
/// everywhere (and clears it from `active_global` too).
pub fn set_rule_projects(name: &str, project_roots: Vec<String>) -> Result<RuleInfo, String> {
    validate_simple_name(name)?;
    let bufs: Vec<PathBuf> = project_roots
        .into_iter()
        .map(|s| PathBuf::from(s.trim()))
        .filter(|p| !p.as_os_str().is_empty())
        .collect();
    let refs: Vec<&Path> = bufs.iter().map(|p| p.as_path()).collect();
    agent_set_rule_projects(name, &refs)?;
    Ok(to_info(&find_rule(name)?, None))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_lowercases_and_trims() {
        assert_eq!(sanitize_name("My Rule!"), "my-rule");
        assert_eq!(sanitize_name("--a_b--"), "a-b");
        assert_eq!(sanitize_name("!!!"), "");
    }

    #[test]
    fn summarize_flattens_and_caps() {
        assert_eq!(summarize("a\n\n b   c"), "a b c");
        let long = "word ".repeat(100);
        let s = summarize(&long);
        assert!(s.ends_with("..."));
        assert!(s.chars().count() <= 153);
    }

    #[test]
    fn state_round_trips() {
        for s in ["inactive", "global", "project"] {
            assert_eq!(state_to_string(parse_state(s).unwrap()), s);
        }
        assert!(parse_state("bogus").is_err());
    }
}

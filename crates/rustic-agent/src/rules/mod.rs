use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleDef {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RuleState {
    Inactive,
    Global,
    Project,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct RulesState {
    #[serde(default)]
    pub active_global: Vec<String>,
    #[serde(default)]
    pub active_project: HashMap<String, Vec<String>>,
}

/// Parse rule frontmatter. Returns `(name, description)` or `None`.
pub fn parse_rule_frontmatter(content: &str) -> Option<(String, String)> {
    let content = content.trim_start();
    if !content.starts_with("---") {
        return None;
    }
    let rest = &content[3..];
    let end = rest.find("\n---")?;
    let frontmatter = &rest[..end];

    let mut name: Option<String> = None;
    let mut description: Option<String> = None;

    for line in frontmatter.lines() {
        let line = line.trim();
        if let Some(v) = line.strip_prefix("name:") {
            name = Some(v.trim().to_string());
        } else if let Some(v) = line.strip_prefix("description:") {
            description = Some(v.trim().to_string());
        }
    }

    Some((name?, description?))
}

/// Return the body of a rule file (content after the closing `---`).
pub fn rule_body(content: &str) -> &str {
    let content = content.trim_start();
    if !content.starts_with("---") {
        return content;
    }
    if let Some(rest) = content.get(3..) {
        if let Some(end) = rest.find("\n---") {
            let after = &rest[end + 4..];
            return after.trim_start_matches('\n');
        }
    }
    content
}

/// `~/.rustic/rules/`
pub fn global_rules_dir() -> Option<PathBuf> {
    crate::skills::home_dir().map(|h| h.join(".rustic").join("rules"))
}

/// `~/.rustic/rules-state.json`
pub fn rules_state_path() -> Option<PathBuf> {
    crate::skills::home_dir().map(|h| h.join(".rustic").join("rules-state.json"))
}

pub fn load_rules_state() -> RulesState {
    let Some(path) = rules_state_path() else {
        return RulesState::default();
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return RulesState::default();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

pub fn save_rules_state(state: &RulesState) -> Result<(), String> {
    let path = rules_state_path().ok_or_else(|| "Could not resolve home directory".to_string())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(state).map_err(|e| e.to_string())?;
    crate::io_util::atomic_write(&path, text.as_bytes()).map_err(|e| e.to_string())
}

fn project_key(project_root: &Path) -> String {
    project_root.to_string_lossy().replace('\\', "/")
}

/// Current activation state for `name` in the context of `project_root`.
pub fn rule_state(name: &str, project_root: &Path) -> RuleState {
    let state = load_rules_state();
    if state.active_global.iter().any(|n| n == name) {
        return RuleState::Global;
    }
    let key = project_key(project_root);
    if let Some(list) = state.active_project.get(&key) {
        if list.iter().any(|n| n == name) {
            return RuleState::Project;
        }
    }
    RuleState::Inactive
}

/// Set rule `name` to the given state in the context of `project_root`.
pub fn set_rule_state(
    name: &str,
    new_state: RuleState,
    project_root: &Path,
) -> Result<(), String> {
    let mut state = load_rules_state();
    state.active_global.retain(|n| n != name);
    let key = project_key(project_root);
    if let Some(list) = state.active_project.get_mut(&key) {
        list.retain(|n| n != name);
    }
    match new_state {
        RuleState::Inactive => {}
        RuleState::Global => {
            if !state.active_global.iter().any(|n| n == name) {
                state.active_global.push(name.to_string());
            }
        }
        RuleState::Project => {
            let list = state.active_project.entry(key).or_default();
            if !list.iter().any(|n| n == name) {
                list.push(name.to_string());
            }
        }
    }
    // Clean up empty project lists
    state.active_project.retain(|_, v| !v.is_empty());
    save_rules_state(&state)
}

/// Set the exact set of projects in which `name` is active. Removes the
/// rule from `active_global` (project + global are mutually exclusive in
/// the picker UX) and from every project list it isn't in the new set.
/// `project_roots` paths are normalised the same way `project_key` would.
/// Empty `project_roots` is equivalent to deactivating the rule entirely.
pub fn set_rule_projects(name: &str, project_roots: &[&Path]) -> Result<(), String> {
    let mut state = load_rules_state();
    state.active_global.retain(|n| n != name);
    // Strip the rule from every existing project list first.
    for list in state.active_project.values_mut() {
        list.retain(|n| n != name);
    }
    // Re-add to each requested project.
    for root in project_roots {
        let key = project_key(root);
        let list = state.active_project.entry(key).or_default();
        if !list.iter().any(|n| n == name) {
            list.push(name.to_string());
        }
    }
    // Clean up empty project lists.
    state.active_project.retain(|_, v| !v.is_empty());
    save_rules_state(&state)
}

/// Project keys (as stored in `active_project`) where `name` is active.
/// Used by the settings UI to pre-fill the project-picker dialog.
pub fn rule_active_projects(name: &str) -> Vec<String> {
    let state = load_rules_state();
    state
        .active_project
        .iter()
        .filter_map(|(key, list)| {
            if list.iter().any(|n| n == name) {
                Some(key.clone())
            } else {
                None
            }
        })
        .collect()
}

/// Remove a rule from all activation state (on delete / rename).
pub fn forget_rule(name: &str) -> Result<(), String> {
    let mut state = load_rules_state();
    state.active_global.retain(|n| n != name);
    for list in state.active_project.values_mut() {
        list.retain(|n| n != name);
    }
    state.active_project.retain(|_, v| !v.is_empty());
    save_rules_state(&state)
}

pub fn discover_global_rules() -> Vec<RuleDef> {
    let mut rules: Vec<RuleDef> = Vec::new();
    if let Some(dir) = global_rules_dir() {
        scan_rules_dir(&dir, &mut rules);
    }
    rules
}

fn scan_rules_dir(dir: &Path, out: &mut Vec<RuleDef>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let (name, description) = if let Some(fm) = parse_rule_frontmatter(&content) {
            fm
        } else {
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string();
            (stem, String::new())
        };
        if out.iter().any(|r| r.name == name) {
            continue;
        }
        out.push(RuleDef { name, description, path });
    }
}

/// Build the "User-defined rules" section for the system prompt, using
/// currently-active rules (global + this project).
pub fn build_user_rules_system_section(project_root: &Path) -> String {
    let all = discover_global_rules();
    if all.is_empty() {
        return String::new();
    }
    let state = load_rules_state();
    let key = project_key(project_root);
    let project_active = state.active_project.get(&key).cloned().unwrap_or_default();

    let mut bodies: Vec<(String, String)> = Vec::new();
    for rule in &all {
        let active = state.active_global.iter().any(|n| n == &rule.name)
            || project_active.iter().any(|n| n == &rule.name);
        if !active {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&rule.path) else {
            continue;
        };
        let body = rule_body(&content).trim().to_string();
        if !body.is_empty() {
            bodies.push((rule.name.clone(), body));
        }
    }

    if bodies.is_empty() {
        return String::new();
    }

    let mut section = String::from(
        "\n\n## User-defined rules\nThe user has explicitly defined the following rules. \
         Follow them strictly for the remainder of this conversation:\n",
    );
    for (name, body) in bodies {
        section.push_str(&format!("\n### {}\n{}\n", name, body));
    }
    section
}

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowDef {
    pub name: String,
    pub description: String,
    /// Absolute path to the workflow `.md` file
    pub path: PathBuf,
}

/// Parse workflow frontmatter. Returns `(name, description)` or `None`.
pub fn parse_workflow_frontmatter(content: &str) -> Option<(String, String)> {
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

/// Return the body of a workflow file (content after the closing `---` of the frontmatter).
pub fn workflow_body(content: &str) -> &str {
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

/// Root directory for globally-installed workflows: `~/.rustic/workflows/`.
pub fn global_workflows_dir() -> Option<PathBuf> {
    crate::skills::home_dir().map(|h| h.join(".rustic").join("workflows"))
}

/// Discover workflows from the global workflows directory only.
pub fn discover_global_workflows() -> Vec<WorkflowDef> {
    let mut workflows: Vec<WorkflowDef> = Vec::new();
    if let Some(dir) = global_workflows_dir() {
        scan_workflows_dir(&dir, &mut workflows);
    }
    workflows
}

fn scan_workflows_dir(dir: &Path, out: &mut Vec<WorkflowDef>) {
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
        let (name, description) = if let Some(fm) = parse_workflow_frontmatter(&content) {
            fm
        } else {
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string();
            (stem, String::new())
        };
        if out.iter().any(|w| w.name == name) {
            continue;
        }
        out.push(WorkflowDef { name, description, path });
    }
}

/// Build the system prompt section listing available workflows. Advertises
/// each workflow by name + short description so the model can choose to
/// trigger one when relevant, even without an explicit user tag.
pub fn build_workflows_system_section(workflows: &[WorkflowDef]) -> String {
    if workflows.is_empty() {
        return String::new();
    }
    let mut section = String::from(
        "\n\n## Workflows\nThe following workflows are available. Each one is a \
         predefined prompt for a recurring task:\n",
    );
    for w in workflows {
        section.push_str(&format!("- **{}**: {}\n", w.name, w.description));
    }
    section.push_str(
        "\nWhen the user's request clearly matches a workflow's purpose, you \
         may call `read_workflow(name)` to load its full prompt and then \
         follow those instructions. Prefer a matching workflow over \
         reinventing the same procedure.",
    );
    section
}

/// Discover workflows from `<project>/.rustic/workflows/*.md` AND `~/.rustic/workflows/*.md`.
pub fn discover_workflows(project_root: &Path) -> Vec<WorkflowDef> {
    let mut workflows: Vec<WorkflowDef> = Vec::new();
    scan_workflows_dir(&project_root.join(".rustic/workflows"), &mut workflows);
    if let Some(dir) = global_workflows_dir() {
        scan_workflows_dir(&dir, &mut workflows);
    }
    workflows
}

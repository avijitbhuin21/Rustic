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
///
/// Expected format:
/// ```text
/// ---
/// name: deploy-staging
/// description: Deploy current branch to staging environment
/// ---
/// Body follows here.
/// ```
pub fn parse_workflow_frontmatter(content: &str) -> Option<(String, String)> {
    let content = content.trim_start();
    if !content.starts_with("---") {
        return None;
    }
    let rest = &content[3..];
    // Find closing ---
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

/// Discover all workflows from `<project>/.rustic/workflows/*.md`.
pub fn discover_workflows(project_root: &Path) -> Vec<WorkflowDef> {
    let mut workflows: Vec<WorkflowDef> = Vec::new();
    let workflows_dir = project_root.join(".rustic/workflows");

    let Ok(entries) = std::fs::read_dir(&workflows_dir) else {
        return workflows;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        // Only .md files
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };

        let (name, description) = if let Some(fm) = parse_workflow_frontmatter(&content) {
            fm
        } else {
            // Fallback: use filename stem as name
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string();
            (stem, String::new())
        };

        // Skip if already loaded (first wins)
        if workflows.iter().any(|w| w.name == name) {
            continue;
        }

        workflows.push(WorkflowDef {
            name,
            description,
            path,
        });
    }

    workflows
}

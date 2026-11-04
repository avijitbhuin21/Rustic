//! Global workflows (`~/.rustic/workflows/<name>.md`) — shared by both hosts.

use std::io::Read;
use std::path::PathBuf;

use rustic_agent::{
    discover_global_workflows, global_workflows_dir, workflow_body,
    workflows::parse_workflow_frontmatter,
};
use serde::Serialize;

use super::library::{
    download_repo_zip, download_text, filename_stem, matches_subpath, name_override, open_zip,
    parse_github_source, sanitize_name, strip_zip_top_dir, summarize, with_frontmatter,
    GithubSource,
};
use crate::path_scope::validate_simple_name;

#[derive(Clone, Serialize)]
pub struct WorkflowInfo {
    pub name: String,
    pub description: String,
}

/// A workflow discovered inside a GitHub source.
#[derive(Clone, Serialize)]
pub struct RepoWorkflowInfo {
    pub name: String,
    pub description: String,
    /// Zip entry or raw URL (used as the install key).
    pub path: String,
}

fn workflows_root() -> Result<PathBuf, String> {
    let dir = global_workflows_dir().ok_or_else(|| "Could not resolve home directory".to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

pub fn list_workflows() -> Vec<WorkflowInfo> {
    discover_global_workflows()
        .into_iter()
        .map(|w| WorkflowInfo { name: w.name, description: w.description })
        .collect()
}

pub fn get_workflow_body(name: &str) -> Result<String, String> {
    validate_simple_name(name)?;
    let workflow = discover_global_workflows()
        .into_iter()
        .find(|w| w.name == name)
        .ok_or_else(|| format!("Workflow not found: {}", name))?;
    let content = std::fs::read_to_string(&workflow.path).map_err(|e| e.to_string())?;
    Ok(workflow_body(&content).to_string())
}

pub fn create_workflow(name: &str, body: &str) -> Result<WorkflowInfo, String> {
    let root = workflows_root()?;
    let safe_name = sanitize_name(name);
    if safe_name.is_empty() {
        return Err("Invalid workflow name".to_string());
    }
    let workflow_path = root.join(format!("{}.md", safe_name));
    if workflow_path.exists() {
        return Err(format!("Workflow already exists: {}", safe_name));
    }
    let description = summarize(body);
    rustic_core::io_util::atomic_write(&workflow_path, with_frontmatter(&safe_name, &description, body).as_bytes())
        .map_err(|e| e.to_string())?;
    Ok(WorkflowInfo { name: safe_name, description })
}

/// Renames the `.md` file if the name changes.
pub fn update_workflow(original_name: &str, name: &str, body: &str) -> Result<WorkflowInfo, String> {
    validate_simple_name(original_name)?;
    let root = workflows_root()?;
    let original_path = root.join(format!("{}.md", original_name));
    if !original_path.exists() {
        return Err(format!("Workflow not found: {}", original_name));
    }
    let new_safe_name = sanitize_name(name);
    if new_safe_name.is_empty() {
        return Err("Invalid workflow name".to_string());
    }
    validate_simple_name(&new_safe_name)?;

    let final_path = if new_safe_name != original_name {
        let target = root.join(format!("{}.md", new_safe_name));
        if target.exists() {
            return Err(format!("Workflow already exists: {}", new_safe_name));
        }
        std::fs::rename(&original_path, &target).map_err(|e| e.to_string())?;
        target
    } else {
        original_path
    };
    let description = summarize(body);
    rustic_core::io_util::atomic_write(&final_path, with_frontmatter(&new_safe_name, &description, body).as_bytes())
        .map_err(|e| e.to_string())?;
    Ok(WorkflowInfo { name: new_safe_name, description })
}

pub fn delete_workflow(name: &str) -> Result<(), String> {
    validate_simple_name(name)?;
    let root = workflows_root()?;
    let path = root.join(format!("{}.md", name));
    if !path.exists() {
        return Err(format!("Workflow not found: {}", name));
    }
    let canon_root = root.canonicalize().map_err(|e| e.to_string())?;
    let canon_path = path.canonicalize().map_err(|e| e.to_string())?;
    if !canon_path.starts_with(&canon_root) {
        return Err("Refusing to delete path outside workflows root".to_string());
    }
    std::fs::remove_file(&canon_path).map_err(|e| e.to_string())
}

// ─── GitHub install ─────────────────────────────────────────────────────────

/// Repo sources scan the zip for `.md` files under `workflows/`; single-file
/// URLs are fetched directly.
pub async fn list_repo_workflows(source: &str) -> Result<Vec<RepoWorkflowInfo>, String> {
    match parse_github_source(source)? {
        GithubSource::RawFile { url } => {
            let text = download_text(&url).await?;
            let (name, description) = match parse_workflow_frontmatter(&text) {
                Some(fm) => fm,
                None => (filename_stem(&url), summarize(&text)),
            };
            Ok(vec![RepoWorkflowInfo { name, description, path: url }])
        }
        GithubSource::RepoZip { owner, repo, subpath } => {
            let zip_bytes = download_repo_zip(&owner, &repo).await?;
            scan_workflows_in_zip(&zip_bytes, subpath.as_deref())
        }
    }
}

pub async fn preview_repo_workflow(source: &str, path: &str) -> Result<String, String> {
    super::library::preview_entry(source, path).await
}

pub async fn install_repo_workflows(
    source: &str,
    paths: &[String],
    names: Option<&[String]>,
) -> Result<Vec<WorkflowInfo>, String> {
    match parse_github_source(source)? {
        GithubSource::RawFile { url } => {
            let text = download_text(&url).await?;
            let override_name = name_override(names, 0).unwrap_or_else(|| filename_stem(&url));
            Ok(vec![install_workflow_from_text(&text, Some(&override_name))?])
        }
        GithubSource::RepoZip { owner, repo, .. } => {
            let zip_bytes = download_repo_zip(&owner, &repo).await?;
            extract_workflows_from_zip(&zip_bytes, paths, names)
        }
    }
}

fn install_workflow_from_text(text: &str, override_name: Option<&str>) -> Result<WorkflowInfo, String> {
    let (raw_name, description, body) = match parse_workflow_frontmatter(text) {
        // Strip frontmatter; re-emit with canonical frontmatter below.
        Some((n, d)) => (n, d, workflow_body(text).to_string()),
        None => (
            override_name.unwrap_or("workflow").to_string(),
            summarize(text),
            text.to_string(),
        ),
    };
    let name = override_name.map(|s| s.to_string()).unwrap_or(raw_name);
    let safe_name = sanitize_name(&name);
    if safe_name.is_empty() {
        return Err("Invalid workflow name".to_string());
    }
    let out_path = workflows_root()?.join(format!("{}.md", safe_name));
    if out_path.exists() {
        return Err(format!("Workflow already exists: {}", safe_name));
    }
    rustic_core::io_util::atomic_write(&out_path, with_frontmatter(&safe_name, &description, &body).as_bytes())
        .map_err(|e| e.to_string())?;
    Ok(WorkflowInfo { name: safe_name, description })
}

fn scan_workflows_in_zip(zip_bytes: &[u8], subpath_filter: Option<&str>) -> Result<Vec<RepoWorkflowInfo>, String> {
    let mut archive = open_zip(zip_bytes)?;
    let mut results = Vec::new();
    for i in 0..archive.len() {
        let mut file = archive.by_index(i).map_err(|e| e.to_string())?;
        let name = file.name().to_string();
        if !name.ends_with(".md") {
            continue;
        }
        let inside = strip_zip_top_dir(&name);
        let wanted = match subpath_filter {
            Some(filter) => matches_subpath(inside, filter),
            None => inside.contains("workflows/"),
        };
        if !wanted {
            continue;
        }
        let mut content = String::new();
        if file.read_to_string(&mut content).is_err() {
            continue;
        }
        let Some((wf_name, description)) = parse_workflow_frontmatter(&content) else {
            continue;
        };
        results.push(RepoWorkflowInfo { name: wf_name, description, path: name });
    }
    if results.is_empty() {
        return Err("No workflow .md files with valid frontmatter were found".to_string());
    }
    Ok(results)
}

fn extract_workflows_from_zip(
    zip_bytes: &[u8],
    paths: &[String],
    names: Option<&[String]>,
) -> Result<Vec<WorkflowInfo>, String> {
    let mut archive = open_zip(zip_bytes)?;
    let workflows_dir = workflows_root()?;
    let mut installed = Vec::new();
    for (i, workflow_path) in paths.iter().enumerate() {
        let mut content = String::new();
        archive
            .by_name(workflow_path)
            .map_err(|e| e.to_string())?
            .read_to_string(&mut content)
            .map_err(|e| e.to_string())?;
        let Some((parsed_name, description)) = parse_workflow_frontmatter(&content) else {
            continue;
        };
        let safe_name = sanitize_name(&name_override(names, i).unwrap_or(parsed_name));
        rustic_core::io_util::atomic_write(&workflows_dir.join(format!("{}.md", safe_name)), content.as_bytes())
            .map_err(|e| e.to_string())?;
        installed.push(WorkflowInfo { name: safe_name, description });
    }
    Ok(installed)
}

//! workflows commands — server dispatch.
//!
//! Mirrors `src-tauri/src/commands/workflows.rs`. CRUD/list/get are pure
//! filesystem + agent-crate calls and are wired here. The "repo" commands
//! (list_repo_workflows, preview_repo_workflow, install_repo_workflows) require
//! `reqwest` + the `zip` crate, neither of which is a rustic-server dependency,
//! so they fall through to a 501.

use std::io::Read;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use rustic_agent::{
    discover_global_workflows, global_workflows_dir, workflow_body,
    workflows::parse_workflow_frontmatter, WorkflowDef,
};
use rustic_app::path_scope::validate_simple_name;

use crate::api::{ok, parse, ApiError};
use crate::context::ServerContext;

#[derive(Clone, Serialize)]
struct WorkflowInfo {
    name: String,
    description: String,
}

/// Describes a workflow discovered inside a GitHub repo archive. Mirrors desktop.
#[derive(Clone, Serialize)]
struct RepoWorkflowInfo {
    name: String,
    description: String,
    /// Path of the workflow .md inside the archive (used as install key)
    path: String,
}

fn to_workflow_info(w: &WorkflowDef) -> WorkflowInfo {
    WorkflowInfo {
        name: w.name.clone(),
        description: w.description.clone(),
    }
}

fn workflows_root() -> Result<PathBuf, String> {
    let dir =
        global_workflows_dir().ok_or_else(|| "Could not resolve home directory".to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

fn sanitize_name(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

/// Build a one-line frontmatter description from the first 150 chars of the
/// body. Collapses whitespace/newlines so it fits on a single YAML line.
fn summarize(body: &str) -> String {
    const MAX: usize = 150;
    let flat: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= MAX {
        flat
    } else {
        let truncated: String = flat.chars().take(MAX).collect();
        format!("{}...", truncated.trim_end())
    }
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

pub async fn dispatch(
    _ctx: &ServerContext,
    command: &str,
    args: &Value,
) -> Option<Result<Value, ApiError>> {
    Some(match command {
        "list_workflows" => list_workflows(),
        "get_workflow_body" => get_workflow_body(args),
        "create_workflow" => create_workflow(args),
        "update_workflow" => update_workflow(args),
        "delete_workflow" => delete_workflow(args),
        "list_repo_workflows" => list_repo_workflows(args).await,
        "preview_repo_workflow" => preview_repo_workflow(args).await,
        "install_repo_workflows" => install_repo_workflows(args).await,
        _ => return None,
    })
}

fn list_workflows() -> Result<Value, ApiError> {
    let workflows = discover_global_workflows();
    ok(workflows.iter().map(to_workflow_info).collect::<Vec<_>>())
}

fn get_workflow_body(args: &Value) -> Result<Value, ApiError> {
    let a: NameArg = parse(args)?;
    validate_simple_name(&a.name)?;
    let workflows = discover_global_workflows();
    let workflow = workflows
        .iter()
        .find(|w| w.name == a.name)
        .ok_or_else(|| ApiError::from(format!("Workflow not found: {}", a.name)))?;
    let content = std::fs::read_to_string(&workflow.path).map_err(|e| e.to_string())?;
    ok(workflow_body(&content).to_string())
}

fn create_workflow(args: &Value) -> Result<Value, ApiError> {
    let a: CreateArg = parse(args)?;
    let root = workflows_root()?;
    let safe_name = sanitize_name(&a.name);
    if safe_name.is_empty() {
        return Err(ApiError::from("Invalid workflow name".to_string()));
    }

    let workflow_path = root.join(format!("{}.md", safe_name));
    if workflow_path.exists() {
        return Err(ApiError::from(format!("Workflow already exists: {}", safe_name)));
    }

    let description = summarize(&a.body);
    let content = format!(
        "---\nname: {}\ndescription: {}\n---\n\n{}",
        safe_name, description, a.body
    );
    rustic_core::io_util::atomic_write(&workflow_path, content.as_bytes())
        .map_err(|e| e.to_string())?;

    ok(WorkflowInfo {
        name: safe_name,
        description,
    })
}

fn update_workflow(args: &Value) -> Result<Value, ApiError> {
    let a: UpdateArg = parse(args)?;
    validate_simple_name(&a.original_name)?;
    let root = workflows_root()?;
    let original_path = root.join(format!("{}.md", a.original_name));
    if !original_path.exists() {
        return Err(ApiError::from(format!("Workflow not found: {}", a.original_name)));
    }

    let new_safe_name = sanitize_name(&a.name);
    if new_safe_name.is_empty() {
        return Err(ApiError::from("Invalid workflow name".to_string()));
    }
    validate_simple_name(&new_safe_name)?;

    let final_path = if new_safe_name != a.original_name {
        let target = root.join(format!("{}.md", new_safe_name));
        if target.exists() {
            return Err(ApiError::from(format!("Workflow already exists: {}", new_safe_name)));
        }
        std::fs::rename(&original_path, &target).map_err(|e| e.to_string())?;
        target
    } else {
        original_path
    };

    let description = summarize(&a.body);
    let content = format!(
        "---\nname: {}\ndescription: {}\n---\n\n{}",
        new_safe_name, description, a.body
    );
    rustic_core::io_util::atomic_write(&final_path, content.as_bytes())
        .map_err(|e| e.to_string())?;

    ok(WorkflowInfo {
        name: new_safe_name,
        description,
    })
}

fn delete_workflow(args: &Value) -> Result<Value, ApiError> {
    let a: NameArg = parse(args)?;
    validate_simple_name(&a.name)?;
    let root = workflows_root()?;
    let path = root.join(format!("{}.md", a.name));
    if !path.exists() {
        return Err(ApiError::from(format!("Workflow not found: {}", a.name)));
    }
    let canon_root = root.canonicalize().map_err(|e| e.to_string())?;
    let canon_path = path.canonicalize().map_err(|e| e.to_string())?;
    if !canon_path.starts_with(&canon_root) {
        return Err(ApiError::from(
            "Refusing to delete path outside workflows root".to_string(),
        ));
    }
    std::fs::remove_file(&canon_path).map_err(|e| e.to_string())?;
    ok(serde_json::json!(null))
}

// ─── GitHub install ─────────────────────────────────────────────────────────

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

/// List all workflows contained in a GitHub source.
async fn list_repo_workflows(args: &Value) -> Result<Value, ApiError> {
    let a: SourceArg = parse(args)?;
    match parse_github_source(&a.source)? {
        GithubSource::RawFile { url } => {
            let text = download_text(&url).await?;
            let (name, description) = match parse_workflow_frontmatter(&text) {
                Some(fm) => fm,
                None => (filename_stem(&url), summarize(&text)),
            };
            ok(vec![RepoWorkflowInfo {
                name,
                description,
                path: url,
            }])
        }
        GithubSource::RepoZip {
            owner,
            repo,
            subpath,
        } => {
            let zip_bytes = download_repo_zip(&owner, &repo).await?;
            ok(scan_workflows_in_zip(&zip_bytes, subpath.as_deref())?)
        }
    }
}

/// Fetch a workflow-file's raw content from a GitHub source, for preview.
async fn preview_repo_workflow(args: &Value) -> Result<Value, ApiError> {
    let a: SourcePathArg = parse(args)?;
    match parse_github_source(&a.source)? {
        GithubSource::RawFile { .. } => ok(download_text(&a.path).await?),
        GithubSource::RepoZip { owner, repo, .. } => {
            let zip_bytes = download_repo_zip(&owner, &repo).await?;
            ok(read_zip_text(&zip_bytes, &a.path)?)
        }
    }
}

async fn install_repo_workflows(args: &Value) -> Result<Value, ApiError> {
    let a: InstallArg = parse(args)?;
    let names = a.names;
    let name_at = |i: usize| -> Option<String> {
        names
            .as_ref()
            .and_then(|v| v.get(i))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };

    match parse_github_source(&a.source)? {
        GithubSource::RawFile { url } => {
            let text = download_text(&url).await?;
            let override_name = name_at(0).unwrap_or_else(|| filename_stem(&url));
            ok(vec![install_workflow_from_text(&text, Some(&override_name))?])
        }
        GithubSource::RepoZip { owner, repo, .. } => {
            let zip_bytes = download_repo_zip(&owner, &repo).await?;
            ok(extract_workflows_from_zip(
                &zip_bytes,
                &a.paths,
                names.as_deref(),
            )?)
        }
    }
}

// ─── helpers ────────────────────────────────────────────────────────────────

enum GithubSource {
    RepoZip {
        owner: String,
        repo: String,
        subpath: Option<String>,
    },
    RawFile {
        url: String,
    },
}

fn parse_github_source(source: &str) -> Result<GithubSource, String> {
    let source = source.trim();

    if source.starts_with("https://raw.githubusercontent.com/")
        || source.starts_with("http://raw.githubusercontent.com/")
    {
        return Ok(GithubSource::RawFile {
            url: source.to_string(),
        });
    }

    let path = source
        .trim_start_matches("https://github.com/")
        .trim_start_matches("http://github.com/")
        .trim_start_matches("github.com/")
        .trim_end_matches('/');

    let segments: Vec<&str> = path.split('/').collect();
    if segments.len() < 2 || segments[0].is_empty() || segments[1].is_empty() {
        return Err(format!(
            "Invalid GitHub source: \"{}\" — expected owner/repo, a github.com URL, or a raw.githubusercontent.com URL",
            source
        ));
    }

    let owner = segments[0].to_string();
    let repo = segments[1].trim_end_matches(".git").to_string();

    if segments.len() > 4 && segments[2] == "blob" {
        let branch = segments[3];
        let subpath = segments[4..].join("/");
        if subpath.ends_with(".md") {
            let url = format!(
                "https://raw.githubusercontent.com/{}/{}/{}/{}",
                owner, repo, branch, subpath
            );
            return Ok(GithubSource::RawFile { url });
        }
        return Ok(GithubSource::RepoZip {
            owner,
            repo,
            subpath: Some(subpath),
        });
    }

    Ok(GithubSource::RepoZip {
        owner,
        repo,
        subpath: None,
    })
}

async fn download_text(url: &str) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .user_agent("Rustic-Agent/0.1")
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client.get(url).send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {} downloading {}", resp.status(), url));
    }
    resp.text().await.map_err(|e| e.to_string())
}

fn install_workflow_from_text(
    text: &str,
    override_name: Option<&str>,
) -> Result<WorkflowInfo, String> {
    let (raw_name, description, body) = match parse_workflow_frontmatter(text) {
        Some((n, d)) => {
            let b = workflow_body(text).to_string();
            (n, d, b)
        }
        None => {
            let fallback = override_name
                .map(|s| s.to_string())
                .unwrap_or_else(|| "workflow".to_string());
            (fallback, summarize(text), text.to_string())
        }
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
    let content = format!(
        "---\nname: {}\ndescription: {}\n---\n\n{}",
        safe_name, description, body
    );
    rustic_core::io_util::atomic_write(&out_path, content.as_bytes()).map_err(|e| e.to_string())?;
    Ok(WorkflowInfo {
        name: safe_name,
        description,
    })
}

fn filename_stem(url_or_path: &str) -> String {
    let last = url_or_path
        .rsplit(&['/', '\\'])
        .next()
        .unwrap_or(url_or_path);
    let stem = last.rsplit_once('.').map(|(a, _)| a).unwrap_or(last);
    stem.to_string()
}

async fn download_repo_zip(owner: &str, repo: &str) -> Result<Vec<u8>, String> {
    let main_url = format!(
        "https://codeload.github.com/{}/{}/zip/refs/heads/main",
        owner, repo
    );
    match download_bytes(&main_url).await {
        Ok(b) => Ok(b),
        Err(_) => {
            let master_url = format!(
                "https://codeload.github.com/{}/{}/zip/refs/heads/master",
                owner, repo
            );
            download_bytes(&master_url).await
        }
    }
}

async fn download_bytes(url: &str) -> Result<Vec<u8>, String> {
    let client = reqwest::Client::builder()
        .user_agent("Rustic-Agent/0.1")
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client.get(url).send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {} downloading {}", resp.status(), url));
    }
    resp.bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(|e| e.to_string())
}

fn read_zip_text(zip_bytes: &[u8], entry: &str) -> Result<String, String> {
    use std::io::Cursor;
    let cursor = Cursor::new(zip_bytes);
    let mut archive = zip::ZipArchive::new(cursor).map_err(|e| e.to_string())?;
    let mut file = archive.by_name(entry).map_err(|e| e.to_string())?;
    let mut s = String::new();
    file.read_to_string(&mut s).map_err(|e| e.to_string())?;
    Ok(s)
}

fn strip_zip_top_dir(path: &str) -> &str {
    match path.find('/') {
        Some(i) => &path[i + 1..],
        None => path,
    }
}

fn scan_workflows_in_zip(
    zip_bytes: &[u8],
    subpath_filter: Option<&str>,
) -> Result<Vec<RepoWorkflowInfo>, String> {
    use std::io::Cursor;
    let cursor = Cursor::new(zip_bytes);
    let mut archive = zip::ZipArchive::new(cursor).map_err(|e| e.to_string())?;

    let mut results: Vec<RepoWorkflowInfo> = Vec::new();
    for i in 0..archive.len() {
        let mut file = archive.by_index(i).map_err(|e| e.to_string())?;
        let name = file.name().to_string();
        if !name.ends_with(".md") {
            continue;
        }
        let inside = strip_zip_top_dir(&name);

        if let Some(filter) = subpath_filter {
            let filter = filter.trim_end_matches('/');
            if inside != filter
                && !inside.ends_with(&format!("/{}", filter))
                && !filter.ends_with(inside)
            {
                continue;
            }
        } else if !inside.contains("workflows/") {
            continue;
        }

        let mut content = String::new();
        if file.read_to_string(&mut content).is_err() {
            continue;
        }
        let Some((wf_name, description)) = parse_workflow_frontmatter(&content) else {
            continue;
        };
        results.push(RepoWorkflowInfo {
            name: wf_name,
            description,
            path: name,
        });
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
    use std::io::Cursor;
    let cursor = Cursor::new(zip_bytes);
    let mut archive = zip::ZipArchive::new(cursor).map_err(|e| e.to_string())?;

    let workflows_dir = workflows_root()?;
    let mut installed: Vec<WorkflowInfo> = Vec::new();

    for (i, workflow_path) in paths.iter().enumerate() {
        let mut file = archive.by_name(workflow_path).map_err(|e| e.to_string())?;
        let mut content = String::new();
        file.read_to_string(&mut content).map_err(|e| e.to_string())?;
        drop(file);

        let Some((parsed_name, description)) = parse_workflow_frontmatter(&content) else {
            continue;
        };
        let override_name = names
            .and_then(|v| v.get(i))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let chosen = override_name.unwrap_or(parsed_name);
        let safe_name = sanitize_name(&chosen);
        let out_path = workflows_dir.join(format!("{}.md", safe_name));
        rustic_core::io_util::atomic_write(&out_path, content.as_bytes())
            .map_err(|e| e.to_string())?;

        installed.push(WorkflowInfo {
            name: safe_name,
            description,
        });
    }

    Ok(installed)
}

use rustic_agent::{
    discover_global_workflows, global_workflows_dir, workflow_body,
    workflows::parse_workflow_frontmatter, WorkflowDef,
};
use serde::Serialize;
use std::io::Read;
use std::path::PathBuf;

use rustic_app::github_download::{
    self, DownloadError, MAX_TEXT_DOWNLOAD_BYTES, MAX_ZIP_DOWNLOAD_BYTES,
};

use crate::path_scope::validate_simple_name;

#[derive(Clone, Serialize)]
pub struct WorkflowInfo {
    pub name: String,
    pub description: String,
}

/// Describes a workflow discovered inside a GitHub repo archive.
#[derive(Clone, Serialize)]
pub struct RepoWorkflowInfo {
    pub name: String,
    pub description: String,
    /// Path of the workflow .md inside the archive (used as install key)
    pub path: String,
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
        .map(|c| {
            if c.is_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
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

#[tauri::command]
pub fn list_workflows() -> Result<Vec<WorkflowInfo>, String> {
    let workflows = discover_global_workflows();
    Ok(workflows.iter().map(to_workflow_info).collect())
}

#[tauri::command]
pub fn get_workflow_body(name: String) -> Result<String, String> {
    validate_simple_name(&name)?;
    let workflows = discover_global_workflows();
    let workflow = workflows
        .iter()
        .find(|w| w.name == name)
        .ok_or_else(|| format!("Workflow not found: {}", name))?;
    let content = std::fs::read_to_string(&workflow.path).map_err(|e| e.to_string())?;
    Ok(workflow_body(&content).to_string())
}

#[tauri::command]
pub fn create_workflow(name: String, body: String) -> Result<WorkflowInfo, String> {
    let root = workflows_root()?;
    let safe_name = sanitize_name(&name);
    if safe_name.is_empty() {
        return Err("Invalid workflow name".to_string());
    }

    let workflow_path = root.join(format!("{}.md", safe_name));
    if workflow_path.exists() {
        return Err(format!("Workflow already exists: {}", safe_name));
    }

    let description = summarize(&body);
    let content = format!(
        "---\nname: {}\ndescription: {}\n---\n\n{}",
        safe_name, description, body
    );
    rustic_core::io_util::atomic_write(&workflow_path, content.as_bytes())
        .map_err(|e| e.to_string())?;

    Ok(WorkflowInfo {
        name: safe_name,
        description,
    })
}

/// Update an existing workflow. Renames the `.md` file if the name changes.
#[tauri::command]
pub fn update_workflow(
    original_name: String,
    name: String,
    body: String,
) -> Result<WorkflowInfo, String> {
    validate_simple_name(&original_name)?;
    let root = workflows_root()?;
    let original_path = root.join(format!("{}.md", original_name));
    if !original_path.exists() {
        return Err(format!("Workflow not found: {}", original_name));
    }

    let new_safe_name = sanitize_name(&name);
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

    let description = summarize(&body);
    let content = format!(
        "---\nname: {}\ndescription: {}\n---\n\n{}",
        new_safe_name, description, body
    );
    rustic_core::io_util::atomic_write(&final_path, content.as_bytes())
        .map_err(|e| e.to_string())?;

    Ok(WorkflowInfo {
        name: new_safe_name,
        description,
    })
}

#[tauri::command]
pub fn delete_workflow(name: String) -> Result<(), String> {
    validate_simple_name(&name)?;
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
    std::fs::remove_file(&canon_path).map_err(|e| e.to_string())?;
    Ok(())
}

// ─── GitHub install ─────────────────────────────────────────────────────────

/// List all workflows contained in a GitHub source. The source may be a repo
/// (zip is scanned for .md files under `workflows/`) or a single-file URL
/// (raw or blob) which is fetched directly.
#[tauri::command]
pub async fn list_repo_workflows(source: String) -> Result<Vec<RepoWorkflowInfo>, String> {
    match parse_github_source(&source)? {
        GithubSource::RawFile { url } => {
            let text = download_text(&url).await?;
            let (name, description) = match parse_workflow_frontmatter(&text) {
                Some(fm) => fm,
                None => (filename_stem(&url), summarize(&text)),
            };
            Ok(vec![RepoWorkflowInfo {
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
            scan_workflows_in_zip(&zip_bytes, subpath.as_deref())
        }
    }
}

/// Fetch a workflow-file's raw content from a GitHub source, for preview
/// before installing.
#[tauri::command]
pub async fn preview_repo_workflow(source: String, path: String) -> Result<String, String> {
    match parse_github_source(&source)? {
        GithubSource::RawFile { .. } => download_text(&path).await,
        GithubSource::RepoZip { owner, repo, .. } => {
            let zip_bytes = download_repo_zip(&owner, &repo).await?;
            read_zip_text(&zip_bytes, &path)
        }
    }
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

#[tauri::command]
pub async fn install_repo_workflows(
    source: String,
    paths: Vec<String>,
    names: Option<Vec<String>>,
) -> Result<Vec<WorkflowInfo>, String> {
    let name_at = |i: usize| -> Option<String> {
        names
            .as_ref()
            .and_then(|v| v.get(i))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };

    match parse_github_source(&source)? {
        GithubSource::RawFile { url } => {
            let text = download_text(&url).await?;
            let override_name = name_at(0).unwrap_or_else(|| filename_stem(&url));
            Ok(vec![install_workflow_from_text(
                &text,
                Some(&override_name),
            )?])
        }
        GithubSource::RepoZip { owner, repo, .. } => {
            let zip_bytes = download_repo_zip(&owner, &repo).await?;
            extract_workflows_from_zip(&zip_bytes, &paths, names.as_deref())
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

/// Map a shared [`DownloadError`] to this host's user-facing error strings.
fn map_download_err(e: DownloadError) -> String {
    match e {
        DownloadError::Transport(msg) => msg,
        DownloadError::Http { status, url } => format!("HTTP {} downloading {}", status, url),
        DownloadError::TooLarge { len, cap, url } => format!(
            "Download too large ({} bytes, cap {}) for {}",
            len, cap, url
        ),
        DownloadError::ExceededCap { cap, url } => {
            format!("Download exceeded {} byte cap for {}", cap, url)
        }
    }
}

async fn download_text(url: &str) -> Result<String, String> {
    github_download::download_text(url, MAX_TEXT_DOWNLOAD_BYTES)
        .await
        .map_err(map_download_err)
}

fn install_workflow_from_text(
    text: &str,
    override_name: Option<&str>,
) -> Result<WorkflowInfo, String> {
    let (raw_name, description, body) = match parse_workflow_frontmatter(text) {
        Some((n, d)) => {
            // Strip frontmatter for the body; re-emit with canonical frontmatter later.
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
    github_download::download_repo_zip(owner, repo, MAX_ZIP_DOWNLOAD_BYTES)
        .await
        .map_err(map_download_err)
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
        file.read_to_string(&mut content)
            .map_err(|e| e.to_string())?;
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

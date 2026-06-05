//! skills commands — server dispatch.
//!
//! Mirrors `src-tauri/src/commands/skills.rs`. The CRUD/list/get commands are
//! pure filesystem + agent-crate calls and are wired here verbatim. The "repo"
//! commands (list_repo_skills, preview_repo_skill, install_repo_skills) need
//! `reqwest` (GitHub HTTP download) and the `zip` crate, neither of which is a
//! rustic-server dependency — they fall through to a 501 until the server gains
//! those deps.

use std::io::Read;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use rustic_agent::{
    discover_global_skills, global_skills_dir, skill_body, skills::parse_skill_frontmatter,
    SkillDef, SkillScope,
};
use rustic_app::path_scope::validate_simple_name;

use crate::api::{ok, parse, ApiError};
use crate::context::ServerContext;

/// Serializable skill info returned to the frontend. Mirrors the desktop struct.
#[derive(Clone, Serialize)]
struct SkillInfo {
    name: String,
    description: String,
    scope: String,
    allowed_tools: Option<Vec<String>>,
}

/// Describes a skill discovered inside a GitHub repo archive. Mirrors desktop.
#[derive(Clone, Serialize)]
struct RepoSkillInfo {
    name: String,
    description: String,
    /// Path of the SKILL.md inside the archive (used as install key)
    path: String,
}

fn to_skill_info(s: &SkillDef) -> SkillInfo {
    SkillInfo {
        name: s.name.clone(),
        description: s.description.clone(),
        scope: match s.scope {
            SkillScope::Project => "project".to_string(),
            SkillScope::Global => "global".to_string(),
        },
        allowed_tools: s.allowed_tools.clone(),
    }
}

fn skills_root() -> Result<PathBuf, String> {
    let dir = global_skills_dir().ok_or_else(|| "Could not resolve home directory".to_string())?;
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
        "list_skills" => list_skills(),
        "get_skill_body" => get_skill_body(args),
        "create_skill" => create_skill(args),
        "update_skill" => update_skill(args),
        "delete_skill" => delete_skill(args),
        "list_repo_skills" => list_repo_skills(args).await,
        "preview_repo_skill" => preview_repo_skill(args).await,
        "install_repo_skills" => install_repo_skills(args).await,
        _ => return None,
    })
}

fn list_skills() -> Result<Value, ApiError> {
    let skills = discover_global_skills();
    ok(skills.iter().map(to_skill_info).collect::<Vec<_>>())
}

fn get_skill_body(args: &Value) -> Result<Value, ApiError> {
    let a: NameArg = parse(args)?;
    validate_simple_name(&a.name)?;
    let skills = discover_global_skills();
    let skill = skills
        .iter()
        .find(|s| s.name == a.name)
        .ok_or_else(|| ApiError::from(format!("Skill not found: {}", a.name)))?;
    let content = std::fs::read_to_string(&skill.path).map_err(|e| e.to_string())?;
    ok(skill_body(&content).to_string())
}

fn create_skill(args: &Value) -> Result<Value, ApiError> {
    let a: CreateArg = parse(args)?;
    let root = skills_root()?;
    let safe_name = sanitize_name(&a.name);
    if safe_name.is_empty() {
        return Err(ApiError::from("Invalid skill name".to_string()));
    }

    let skill_dir = root.join(&safe_name);
    std::fs::create_dir_all(&skill_dir).map_err(|e| e.to_string())?;

    let skill_md_path = skill_dir.join("SKILL.md");
    if skill_md_path.exists() {
        return Err(ApiError::from(format!("Skill already exists: {}", safe_name)));
    }

    let description = summarize(&a.body);
    let content = format!(
        "---\nname: {}\ndescription: {}\n---\n\n{}",
        safe_name, description, a.body
    );
    rustic_core::io_util::atomic_write(&skill_md_path, content.as_bytes())
        .map_err(|e| e.to_string())?;

    ok(SkillInfo {
        name: safe_name,
        description,
        scope: "global".to_string(),
        allowed_tools: None,
    })
}

fn update_skill(args: &Value) -> Result<Value, ApiError> {
    let a: UpdateArg = parse(args)?;
    validate_simple_name(&a.original_name)?;
    let root = skills_root()?;
    let original_dir = root.join(&a.original_name);
    if !original_dir.exists() {
        return Err(ApiError::from(format!("Skill not found: {}", a.original_name)));
    }

    let new_safe_name = sanitize_name(&a.name);
    if new_safe_name.is_empty() {
        return Err(ApiError::from("Invalid skill name".to_string()));
    }
    validate_simple_name(&new_safe_name)?;

    let final_dir = if new_safe_name != a.original_name {
        let target = root.join(&new_safe_name);
        if target.exists() {
            return Err(ApiError::from(format!("Skill already exists: {}", new_safe_name)));
        }
        std::fs::rename(&original_dir, &target).map_err(|e| e.to_string())?;
        target
    } else {
        original_dir
    };

    let skill_md_path = final_dir.join("SKILL.md");
    let description = summarize(&a.body);
    let content = format!(
        "---\nname: {}\ndescription: {}\n---\n\n{}",
        new_safe_name, description, a.body
    );
    rustic_core::io_util::atomic_write(&skill_md_path, content.as_bytes())
        .map_err(|e| e.to_string())?;

    ok(SkillInfo {
        name: new_safe_name,
        description,
        scope: "global".to_string(),
        allowed_tools: None,
    })
}

fn delete_skill(args: &Value) -> Result<Value, ApiError> {
    let a: NameArg = parse(args)?;
    validate_simple_name(&a.name)?;
    let root = skills_root()?;
    let skill_dir = root.join(&a.name);
    if !skill_dir.exists() {
        return Err(ApiError::from(format!("Skill not found: {}", a.name)));
    }
    // Defense in depth: ensure the resolved directory is still inside `root`.
    let canon_root = root.canonicalize().map_err(|e| e.to_string())?;
    let canon_dir = skill_dir.canonicalize().map_err(|e| e.to_string())?;
    if !canon_dir.starts_with(&canon_root) {
        return Err(ApiError::from(
            "Refusing to delete path outside skills root".to_string(),
        ));
    }
    std::fs::remove_dir_all(&canon_dir).map_err(|e| e.to_string())?;
    ok(serde_json::json!(null))
}

// ─── GitHub install (two-step: list → install selected) ────────────────────

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

/// List all skills contained in a GitHub source. The source may be a repo
/// (zip is scanned for every SKILL.md) or a single-file URL (raw or blob).
async fn list_repo_skills(args: &Value) -> Result<Value, ApiError> {
    let a: SourceArg = parse(args)?;
    match parse_github_source(&a.source)? {
        GithubSource::RawFile { url } => {
            let text = download_text(&url).await?;
            let (name, description) = match parse_skill_frontmatter(&text) {
                Some((n, d, _)) => (n, d),
                None => (filename_stem(&url), summarize(&text)),
            };
            ok(vec![RepoSkillInfo {
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
            ok(scan_skills_in_zip(&zip_bytes, subpath.as_deref())?)
        }
    }
}

/// Fetch the raw content of a single skill-file from a GitHub source, for
/// preview before installing.
async fn preview_repo_skill(args: &Value) -> Result<Value, ApiError> {
    let a: SourcePathArg = parse(args)?;
    match parse_github_source(&a.source)? {
        GithubSource::RawFile { .. } => ok(download_text(&a.path).await?),
        GithubSource::RepoZip { owner, repo, .. } => {
            let zip_bytes = download_repo_zip(&owner, &repo).await?;
            ok(read_zip_text(&zip_bytes, &a.path)?)
        }
    }
}

/// Install a set of skills from a GitHub source.
async fn install_repo_skills(args: &Value) -> Result<Value, ApiError> {
    let a: InstallArg = parse(args)?;
    let names = a.names;
    let name_override = |i: usize| -> Option<String> {
        names
            .as_ref()
            .and_then(|v| v.get(i))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };

    match parse_github_source(&a.source)? {
        GithubSource::RawFile { url } => {
            let text = download_text(&url).await?;
            let override_name = name_override(0).unwrap_or_else(|| filename_stem(&url));
            ok(vec![install_skill_from_text(&text, Some(&override_name))?])
        }
        GithubSource::RepoZip { owner, repo, .. } => {
            let zip_bytes = download_repo_zip(&owner, &repo).await?;
            ok(extract_skills_from_zip(
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

/// Install a skill from raw file text.
fn install_skill_from_text(text: &str, override_name: Option<&str>) -> Result<SkillInfo, String> {
    let (raw_name, description, allowed_tools, body): (
        String,
        String,
        Option<Vec<String>>,
        String,
    ) = match parse_skill_frontmatter(text) {
        Some((n, d, at)) => (n, d, at, skill_body(text).to_string()),
        None => {
            let fallback_name = override_name
                .map(|s| s.to_string())
                .unwrap_or_else(|| "skill".to_string());
            (fallback_name, summarize(text), None, text.to_string())
        }
    };

    let name = override_name.map(|s| s.to_string()).unwrap_or(raw_name);
    let safe_name = sanitize_name(&name);
    if safe_name.is_empty() {
        return Err("Invalid skill name".to_string());
    }
    let skill_dir = skills_root()?.join(&safe_name);
    if skill_dir.exists() {
        return Err(format!("Skill already exists: {}", safe_name));
    }
    std::fs::create_dir_all(&skill_dir).map_err(|e| e.to_string())?;

    let content = format!(
        "---\nname: {}\ndescription: {}\n---\n\n{}",
        safe_name, description, body
    );
    rustic_core::io_util::atomic_write(&skill_dir.join("SKILL.md"), content.as_bytes())
        .map_err(|e| e.to_string())?;

    Ok(SkillInfo {
        name: safe_name,
        description,
        scope: "global".to_string(),
        allowed_tools,
    })
}

/// Derive a friendly default name from a URL or file path.
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

fn scan_skills_in_zip(
    zip_bytes: &[u8],
    subpath_filter: Option<&str>,
) -> Result<Vec<RepoSkillInfo>, String> {
    use std::io::Cursor;
    let cursor = Cursor::new(zip_bytes);
    let mut archive = zip::ZipArchive::new(cursor).map_err(|e| e.to_string())?;

    let mut results: Vec<RepoSkillInfo> = Vec::new();
    for i in 0..archive.len() {
        let mut file = archive.by_index(i).map_err(|e| e.to_string())?;
        let name = file.name().to_string();
        if !(name.ends_with("/SKILL.md") || name == "SKILL.md") {
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
        }

        let mut content = String::new();
        file.read_to_string(&mut content).map_err(|e| e.to_string())?;
        let Some((skill_name, description, _)) = parse_skill_frontmatter(&content) else {
            continue;
        };
        results.push(RepoSkillInfo {
            name: skill_name,
            description,
            path: name,
        });
    }

    if results.is_empty() {
        return Err("No SKILL.md files found in the archive".to_string());
    }
    Ok(results)
}

fn extract_skills_from_zip(
    zip_bytes: &[u8],
    paths: &[String],
    names: Option<&[String]>,
) -> Result<Vec<SkillInfo>, String> {
    use std::io::Cursor;
    let cursor = Cursor::new(zip_bytes);
    let mut archive = zip::ZipArchive::new(cursor).map_err(|e| e.to_string())?;

    let skills_dir = skills_root()?;
    let mut installed: Vec<SkillInfo> = Vec::new();

    for (i, skill_md_path) in paths.iter().enumerate() {
        let mut file = archive.by_name(skill_md_path).map_err(|e| e.to_string())?;
        let mut content = String::new();
        file.read_to_string(&mut content).map_err(|e| e.to_string())?;
        drop(file);

        let Some((parsed_name, description, allowed_tools)) = parse_skill_frontmatter(&content)
        else {
            continue;
        };

        let dir_prefix = skill_md_path
            .rfind('/')
            .map(|i| &skill_md_path[..i + 1])
            .unwrap_or("")
            .to_string();

        let override_name = names
            .and_then(|v| v.get(i))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let chosen = override_name.unwrap_or(parsed_name);
        let safe_name = sanitize_name(&chosen);
        let skill_out_dir = skills_dir.join(&safe_name);
        std::fs::create_dir_all(&skill_out_dir).map_err(|e| e.to_string())?;

        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
            let entry_name = entry.name().to_string();
            if !dir_prefix.is_empty() && !entry_name.starts_with(&dir_prefix) {
                continue;
            }
            if entry.is_dir() {
                continue;
            }
            let relative = if dir_prefix.is_empty() {
                entry_name.as_str()
            } else {
                &entry_name[dir_prefix.len()..]
            };
            if relative.is_empty() || relative.contains('/') {
                continue;
            }
            let out_path = skill_out_dir.join(relative);
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf).map_err(|e| e.to_string())?;
            rustic_core::io_util::atomic_write(&out_path, &buf).map_err(|e| e.to_string())?;
        }

        installed.push(SkillInfo {
            name: safe_name,
            description,
            scope: "global".to_string(),
            allowed_tools,
        });
    }

    Ok(installed)
}

fn strip_zip_top_dir(path: &str) -> &str {
    match path.find('/') {
        Some(i) => &path[i + 1..],
        None => path,
    }
}

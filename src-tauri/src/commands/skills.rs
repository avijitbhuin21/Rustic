use rustic_agent::{
    discover_global_skills, global_skills_dir, skill_body, skills::parse_skill_frontmatter,
    SkillDef, SkillScope,
};
use serde::Serialize;
use std::io::Read;
use std::path::PathBuf;

use rustic_app::github_download::{
    self, DownloadError, MAX_TEXT_DOWNLOAD_BYTES, MAX_ZIP_DOWNLOAD_BYTES,
};

use crate::path_scope::validate_simple_name;

/// Serializable skill info returned to the frontend.
#[derive(Clone, Serialize)]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    pub scope: String,
    pub allowed_tools: Option<Vec<String>>,
}

/// Describes a skill discovered inside a GitHub repo archive.
#[derive(Clone, Serialize)]
pub struct RepoSkillInfo {
    pub name: String,
    pub description: String,
    /// Path of the SKILL.md inside the archive (used as install key)
    pub path: String,
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
pub fn list_skills() -> Result<Vec<SkillInfo>, String> {
    let skills = discover_global_skills();
    Ok(skills.iter().map(to_skill_info).collect())
}

#[tauri::command]
pub fn get_skill_body(name: String) -> Result<String, String> {
    validate_simple_name(&name)?;
    let skills = discover_global_skills();
    let skill = skills
        .iter()
        .find(|s| s.name == name)
        .ok_or_else(|| format!("Skill not found: {}", name))?;
    let content = std::fs::read_to_string(&skill.path).map_err(|e| e.to_string())?;
    Ok(skill_body(&content).to_string())
}

#[tauri::command]
pub fn create_skill(name: String, body: String) -> Result<SkillInfo, String> {
    let root = skills_root()?;
    let safe_name = sanitize_name(&name);
    if safe_name.is_empty() {
        return Err("Invalid skill name".to_string());
    }

    let skill_dir = root.join(&safe_name);
    std::fs::create_dir_all(&skill_dir).map_err(|e| e.to_string())?;

    let skill_md_path = skill_dir.join("SKILL.md");
    if skill_md_path.exists() {
        return Err(format!("Skill already exists: {}", safe_name));
    }

    let description = summarize(&body);
    let content = format!(
        "---\nname: {}\ndescription: {}\n---\n\n{}",
        safe_name, description, body
    );
    rustic_core::io_util::atomic_write(&skill_md_path, content.as_bytes())
        .map_err(|e| e.to_string())?;

    Ok(SkillInfo {
        name: safe_name,
        description,
        scope: "global".to_string(),
        allowed_tools: None,
    })
}

/// Update an existing skill. If `name` differs from `original_name`, the
/// underlying folder is renamed. Rewrites SKILL.md with the new frontmatter
/// and body.
#[tauri::command]
pub fn update_skill(
    original_name: String,
    name: String,
    body: String,
) -> Result<SkillInfo, String> {
    validate_simple_name(&original_name)?;
    let root = skills_root()?;
    let original_dir = root.join(&original_name);
    if !original_dir.exists() {
        return Err(format!("Skill not found: {}", original_name));
    }

    let new_safe_name = sanitize_name(&name);
    if new_safe_name.is_empty() {
        return Err("Invalid skill name".to_string());
    }
    validate_simple_name(&new_safe_name)?;

    let final_dir = if new_safe_name != original_name {
        let target = root.join(&new_safe_name);
        if target.exists() {
            return Err(format!("Skill already exists: {}", new_safe_name));
        }
        std::fs::rename(&original_dir, &target).map_err(|e| e.to_string())?;
        target
    } else {
        original_dir
    };

    let skill_md_path = final_dir.join("SKILL.md");
    let description = summarize(&body);
    let content = format!(
        "---\nname: {}\ndescription: {}\n---\n\n{}",
        new_safe_name, description, body
    );
    rustic_core::io_util::atomic_write(&skill_md_path, content.as_bytes())
        .map_err(|e| e.to_string())?;

    Ok(SkillInfo {
        name: new_safe_name,
        description,
        scope: "global".to_string(),
        allowed_tools: None,
    })
}

#[tauri::command]
pub fn delete_skill(name: String) -> Result<(), String> {
    validate_simple_name(&name)?;
    let root = skills_root()?;
    let skill_dir = root.join(&name);
    if !skill_dir.exists() {
        return Err(format!("Skill not found: {}", name));
    }
    // Defense in depth: ensure the resolved directory is still inside `root`.
    let canon_root = root.canonicalize().map_err(|e| e.to_string())?;
    let canon_dir = skill_dir.canonicalize().map_err(|e| e.to_string())?;
    if !canon_dir.starts_with(&canon_root) {
        return Err("Refusing to delete path outside skills root".to_string());
    }
    std::fs::remove_dir_all(&canon_dir).map_err(|e| e.to_string())?;
    Ok(())
}

// ─── GitHub install (two-step: list → install selected) ────────────────────

/// List all skills contained in a GitHub source. The source may be a repo
/// (zip is scanned for every SKILL.md) or a single-file URL (raw or blob).
/// For single-file URLs without valid frontmatter, the filename stem is used
/// as the name and the body is summarized for the description — so the user
/// can always install the file.
#[tauri::command]
pub async fn list_repo_skills(source: String) -> Result<Vec<RepoSkillInfo>, String> {
    match parse_github_source(&source)? {
        GithubSource::RawFile { url } => {
            let text = download_text(&url).await?;
            let (name, description) = match parse_skill_frontmatter(&text) {
                Some((n, d, _)) => (n, d),
                None => (filename_stem(&url), summarize(&text)),
            };
            Ok(vec![RepoSkillInfo {
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
            scan_skills_in_zip(&zip_bytes, subpath.as_deref())
        }
    }
}

/// Fetch the raw content of a single skill-file from a GitHub source, for
/// preview before installing. `path` is an entry returned by
/// `list_repo_skills` — either a zip path or a raw URL.
#[tauri::command]
pub async fn preview_repo_skill(source: String, path: String) -> Result<String, String> {
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

/// Install a set of skills from a GitHub source. `paths` are the identifiers
/// returned by `list_repo_skills` (zip entries or raw URLs); `names`, if
/// provided, is a parallel array that overrides each skill's name.
#[tauri::command]
pub async fn install_repo_skills(
    source: String,
    paths: Vec<String>,
    names: Option<Vec<String>>,
) -> Result<Vec<SkillInfo>, String> {
    let name_override = |i: usize| -> Option<String> {
        names
            .as_ref()
            .and_then(|v| v.get(i))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };

    match parse_github_source(&source)? {
        GithubSource::RawFile { url } => {
            let text = download_text(&url).await?;
            let override_name = name_override(0).unwrap_or_else(|| filename_stem(&url));
            Ok(vec![install_skill_from_text(&text, Some(&override_name))?])
        }
        GithubSource::RepoZip { owner, repo, .. } => {
            let zip_bytes = download_repo_zip(&owner, &repo).await?;
            extract_skills_from_zip(&zip_bytes, &paths, names.as_deref())
        }
    }
}

// ─── helpers ────────────────────────────────────────────────────────────────

enum GithubSource {
    /// Download the whole repo as a zip and scan it (optionally filtered by subpath).
    RepoZip {
        owner: String,
        repo: String,
        subpath: Option<String>,
    },
    /// Fetch a single file over HTTP (raw.githubusercontent.com or converted from blob).
    RawFile { url: String },
}

/// Parse a GitHub source. Accepts:
///   - `owner/repo`
///   - `https://github.com/owner/repo[.git]`
///   - `https://github.com/owner/repo/blob/<branch>/path/to/SKILL.md`
///   - `https://raw.githubusercontent.com/owner/repo/<branch>/path/to/file.md`
///
/// Blob URLs pointing at a specific `.md` file are transparently rewritten
/// to their raw equivalent so the file is fetched directly instead of
/// downloading the whole repo zip.
fn parse_github_source(source: &str) -> Result<GithubSource, String> {
    let source = source.trim();

    // 1. Direct raw URL
    if source.starts_with("https://raw.githubusercontent.com/")
        || source.starts_with("http://raw.githubusercontent.com/")
    {
        return Ok(GithubSource::RawFile {
            url: source.to_string(),
        });
    }

    // 2. Strip github.com prefix (if present)
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

    // Blob URL: owner/repo/blob/<branch>/<path...>
    if segments.len() > 4 && segments[2] == "blob" {
        let branch = segments[3];
        let subpath = segments[4..].join("/");

        // Single-file blob URL → rewrite to raw and fetch directly
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

/// Install a skill from raw file text. If the text has valid SKILL.md
/// frontmatter it is used as-is (with optional name override). Otherwise the
/// entire text is treated as the body, `override_name` (or a fallback) as the
/// name, and the description is auto-summarized.
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

    // Always write with canonical frontmatter so agent-side discovery works.
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

/// Derive a friendly default name from a URL or file path — the filename
/// (minus extension), sanitized.
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

        // Strip the top-level "<repo>-<branch>/" prefix for user-facing filtering.
        let inside = strip_zip_top_dir(&name);

        if let Some(filter) = subpath_filter {
            // Match either the exact path or the SKILL.md at the end of the filter.
            let filter = filter.trim_end_matches('/');
            if inside != filter
                && !inside.ends_with(&format!("/{}", filter))
                && !filter.ends_with(inside)
            {
                continue;
            }
        }

        let mut content = String::new();
        file.read_to_string(&mut content)
            .map_err(|e| e.to_string())?;
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
        // Read the SKILL.md
        let mut file = archive.by_name(skill_md_path).map_err(|e| e.to_string())?;
        let mut content = String::new();
        file.read_to_string(&mut content)
            .map_err(|e| e.to_string())?;
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

        // Copy all top-level files from that directory
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

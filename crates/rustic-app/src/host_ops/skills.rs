//! Global skills (`~/.rustic/skills/<name>/SKILL.md`) — shared by both hosts.

use std::io::Read;
use std::path::PathBuf;

use rustic_agent::{
    discover_global_skills, global_skills_dir, skill_body, skills::parse_skill_frontmatter,
    SkillDef, SkillScope,
};
use serde::Serialize;

use super::library::{
    download_repo_zip, download_text, filename_stem, matches_subpath, name_override, open_zip,
    parse_github_source, sanitize_name, strip_zip_top_dir, summarize, with_frontmatter,
    GithubSource,
};
use crate::path_scope::validate_simple_name;

#[derive(Clone, Serialize)]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    pub scope: String,
    pub allowed_tools: Option<Vec<String>>,
}

/// A skill discovered inside a GitHub source.
#[derive(Clone, Serialize)]
pub struct RepoSkillInfo {
    pub name: String,
    pub description: String,
    /// Zip entry of the SKILL.md or a raw URL (used as the install key).
    pub path: String,
}

fn to_skill_info(s: &SkillDef) -> SkillInfo {
    SkillInfo {
        name: s.name.clone(),
        description: s.description.clone(),
        scope: match s.scope {
            SkillScope::Project => "project",
            SkillScope::Global => "global",
            SkillScope::Builtin => "builtin",
        }
        .to_string(),
        allowed_tools: s.allowed_tools.clone(),
    }
}

fn global_info(name: String, description: String, allowed_tools: Option<Vec<String>>) -> SkillInfo {
    SkillInfo {
        name,
        description,
        scope: "global".to_string(),
        allowed_tools,
    }
}

fn skills_root() -> Result<PathBuf, String> {
    let dir = global_skills_dir().ok_or_else(|| "Could not resolve home directory".to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

pub fn list_skills() -> Vec<SkillInfo> {
    discover_global_skills().iter().map(to_skill_info).collect()
}

pub fn get_skill_body(name: &str) -> Result<String, String> {
    validate_simple_name(name)?;
    let skill = discover_global_skills()
        .into_iter()
        .find(|s| s.name == name)
        .ok_or_else(|| format!("Skill not found: {}", name))?;
    let content = std::fs::read_to_string(&skill.path).map_err(|e| e.to_string())?;
    Ok(skill_body(&content).to_string())
}

pub fn create_skill(name: &str, body: &str) -> Result<SkillInfo, String> {
    let root = skills_root()?;
    let safe_name = sanitize_name(name);
    if safe_name.is_empty() {
        return Err("Invalid skill name".to_string());
    }
    let skill_dir = root.join(&safe_name);
    std::fs::create_dir_all(&skill_dir).map_err(|e| e.to_string())?;
    let skill_md_path = skill_dir.join("SKILL.md");
    if skill_md_path.exists() {
        return Err(format!("Skill already exists: {}", safe_name));
    }
    let description = summarize(body);
    rustic_core::io_util::atomic_write(
        &skill_md_path,
        with_frontmatter(&safe_name, &description, body).as_bytes(),
    )
    .map_err(|e| e.to_string())?;
    Ok(global_info(safe_name, description, None))
}

/// Renames the skill folder if the name changes, then rewrites SKILL.md.
pub fn update_skill(original_name: &str, name: &str, body: &str) -> Result<SkillInfo, String> {
    validate_simple_name(original_name)?;
    let root = skills_root()?;
    let original_dir = root.join(original_name);
    if !original_dir.exists() {
        return Err(format!("Skill not found: {}", original_name));
    }
    let new_safe_name = sanitize_name(name);
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
    let description = summarize(body);
    rustic_core::io_util::atomic_write(
        &final_dir.join("SKILL.md"),
        with_frontmatter(&new_safe_name, &description, body).as_bytes(),
    )
    .map_err(|e| e.to_string())?;
    Ok(global_info(new_safe_name, description, None))
}

pub fn delete_skill(name: &str) -> Result<(), String> {
    validate_simple_name(name)?;
    let root = skills_root()?;
    let skill_dir = root.join(name);
    if !skill_dir.exists() {
        return Err(format!("Skill not found: {}", name));
    }
    // Defense in depth: the resolved directory must stay inside `root`.
    let canon_root = root.canonicalize().map_err(|e| e.to_string())?;
    let canon_dir = skill_dir.canonicalize().map_err(|e| e.to_string())?;
    if !canon_dir.starts_with(&canon_root) {
        return Err("Refusing to delete path outside skills root".to_string());
    }
    std::fs::remove_dir_all(&canon_dir).map_err(|e| e.to_string())
}

// ─── GitHub install (two-step: list → install selected) ────────────────────

/// Repo sources scan the zip for every SKILL.md; single-file URLs without
/// frontmatter fall back to the filename stem + a summarized description.
pub async fn list_repo_skills(source: &str) -> Result<Vec<RepoSkillInfo>, String> {
    match parse_github_source(source)? {
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

pub async fn preview_repo_skill(source: &str, path: &str) -> Result<String, String> {
    super::library::preview_entry(source, path).await
}

/// `paths` are identifiers from `list_repo_skills`; `names` optionally
/// overrides each skill's name (parallel array).
pub async fn install_repo_skills(
    source: &str,
    paths: &[String],
    names: Option<&[String]>,
) -> Result<Vec<SkillInfo>, String> {
    match parse_github_source(source)? {
        GithubSource::RawFile { url } => {
            let text = download_text(&url).await?;
            let override_name = name_override(names, 0).unwrap_or_else(|| filename_stem(&url));
            Ok(vec![install_skill_from_text(&text, Some(&override_name))?])
        }
        GithubSource::RepoZip { owner, repo, .. } => {
            let zip_bytes = download_repo_zip(&owner, &repo).await?;
            extract_skills_from_zip(&zip_bytes, paths, names)
        }
    }
}

/// Valid frontmatter is used as-is (with optional name override); otherwise
/// the whole text is the body and the description is auto-summarized.
fn install_skill_from_text(text: &str, override_name: Option<&str>) -> Result<SkillInfo, String> {
    let (raw_name, description, allowed_tools, body) = match parse_skill_frontmatter(text) {
        Some((n, d, at)) => (n, d, at, skill_body(text).to_string()),
        None => (
            override_name.unwrap_or("skill").to_string(),
            summarize(text),
            None,
            text.to_string(),
        ),
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
    // Always canonical frontmatter so agent-side discovery works.
    rustic_core::io_util::atomic_write(
        &skill_dir.join("SKILL.md"),
        with_frontmatter(&safe_name, &description, &body).as_bytes(),
    )
    .map_err(|e| e.to_string())?;
    Ok(global_info(safe_name, description, allowed_tools))
}

pub fn scan_skills_in_zip(
    zip_bytes: &[u8],
    subpath_filter: Option<&str>,
) -> Result<Vec<RepoSkillInfo>, String> {
    let mut results = Vec::new();
    scan_skills_in_zip_each(zip_bytes, subpath_filter, &mut |s| results.push(s))?;
    if results.is_empty() {
        return Err("No SKILL.md files found in the archive".to_string());
    }
    Ok(results)
}

/// Walk the archive and hand every valid SKILL.md to `on_found` as soon as it is parsed.
pub fn scan_skills_in_zip_each(
    zip_bytes: &[u8],
    subpath_filter: Option<&str>,
    on_found: &mut dyn FnMut(RepoSkillInfo),
) -> Result<(), String> {
    let mut archive = open_zip(zip_bytes)?;
    for i in 0..archive.len() {
        let mut file = archive.by_index(i).map_err(|e| e.to_string())?;
        let name = file.name().map_err(|e| e.to_string())?.to_string();
        if !(name.ends_with("/SKILL.md") || name == "SKILL.md") {
            continue;
        }
        if let Some(filter) = subpath_filter {
            if !matches_subpath(strip_zip_top_dir(&name), filter) {
                continue;
            }
        }
        let mut content = String::new();
        file.read_to_string(&mut content)
            .map_err(|e| e.to_string())?;
        let Some((skill_name, description, _)) = parse_skill_frontmatter(&content) else {
            continue;
        };
        on_found(RepoSkillInfo {
            name: skill_name,
            description,
            path: name,
        });
    }
    Ok(())
}

/// Entry name relative to `dir_prefix` if it is a direct (top-level) file
/// of that directory and safe to write; `None` otherwise.
fn top_level_relative<'a>(entry_name: &'a str, dir_prefix: &str) -> Option<&'a str> {
    let relative = entry_name.strip_prefix(dir_prefix)?;
    if relative.is_empty()
        || relative.contains(['/', '\\', ':'])
        || relative == "."
        || relative == ".."
    {
        return None;
    }
    Some(relative)
}

fn extract_skills_from_zip(
    zip_bytes: &[u8],
    paths: &[String],
    names: Option<&[String]>,
) -> Result<Vec<SkillInfo>, String> {
    let mut archive = open_zip(zip_bytes)?;
    let skills_dir = skills_root()?;
    let mut installed = Vec::new();

    for (i, skill_md_path) in paths.iter().enumerate() {
        let mut content = String::new();
        archive
            .by_name(skill_md_path)
            .map_err(|e| e.to_string())?
            .read_to_string(&mut content)
            .map_err(|e| e.to_string())?;
        let Some((parsed_name, description, allowed_tools)) = parse_skill_frontmatter(&content)
        else {
            continue;
        };
        let dir_prefix = skill_md_path
            .rfind('/')
            .map(|i| &skill_md_path[..i + 1])
            .unwrap_or("");
        let safe_name = sanitize_name(&name_override(names, i).unwrap_or(parsed_name));
        let skill_out_dir = skills_dir.join(&safe_name);
        std::fs::create_dir_all(&skill_out_dir).map_err(|e| e.to_string())?;

        // Copy all top-level files from the skill's directory.
        for j in 0..archive.len() {
            let mut entry = archive.by_index(j).map_err(|e| e.to_string())?;
            if entry.is_dir() {
                continue;
            }
            let entry_name = entry.name().map_err(|e| e.to_string())?.to_string();
            let Some(relative) = top_level_relative(&entry_name, dir_prefix) else {
                continue;
            };
            let out_path = skill_out_dir.join(relative);
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf).map_err(|e| e.to_string())?;
            rustic_core::io_util::atomic_write(&out_path, &buf).map_err(|e| e.to_string())?;
        }
        installed.push(global_info(safe_name, description, allowed_tools));
    }
    Ok(installed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn zip_with(entries: &[(&str, &str)]) -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            let opts = zip::write::SimpleFileOptions::default();
            for (name, body) in entries {
                w.start_file(*name, opts).unwrap();
                w.write_all(body.as_bytes()).unwrap();
            }
            w.finish().unwrap();
        }
        buf.into_inner()
    }

    #[test]
    fn scans_skill_md_with_frontmatter_and_filter() {
        let fm = "---\nname: alpha\ndescription: does a\n---\n\nbody";
        let zip = zip_with(&[
            ("repo-main/skills/alpha/SKILL.md", fm),
            ("repo-main/skills/beta/SKILL.md", "no frontmatter"),
            ("repo-main/README.md", fm),
        ]);
        let all = scan_skills_in_zip(&zip, None).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "alpha");
        assert_eq!(all[0].path, "repo-main/skills/alpha/SKILL.md");
        assert!(scan_skills_in_zip(&zip, Some("skills/beta/SKILL.md")).is_err());
    }

    #[test]
    fn top_level_relative_rejects_nested_and_traversal() {
        assert_eq!(top_level_relative("r/s/a/run.sh", "r/s/a/"), Some("run.sh"));
        assert_eq!(top_level_relative("r/s/a/sub/x", "r/s/a/"), None);
        assert_eq!(top_level_relative("r/s/b/x", "r/s/a/"), None);
        assert_eq!(top_level_relative("SKILL.md", ""), Some("SKILL.md"));
        assert_eq!(top_level_relative("r/s/a/..", "r/s/a/"), None);
    }
}

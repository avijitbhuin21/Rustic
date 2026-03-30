use crate::state::AppState;
use rustic_agent::{SkillDef, SkillScope, discover_skills, skill_body, skills::parse_skill_frontmatter};
use serde::Serialize;
use std::io::Read;
use std::path::PathBuf;
use tauri::State;

/// Serializable skill info returned to the frontend.
#[derive(Clone, Serialize)]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    pub scope: String,
    pub allowed_tools: Option<Vec<String>>,
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

fn project_root(state: &AppState, project_id: &str) -> Result<PathBuf, String> {
    let workspace = state.workspace.lock().unwrap();
    workspace
        .list_projects()
        .into_iter()
        .find(|p| p.id.to_string() == project_id)
        .map(|p| p.root_path.clone())
        .ok_or_else(|| "Project not found".to_string())
}

#[tauri::command]
pub fn list_skills(state: State<'_, AppState>, project_id: String) -> Result<Vec<SkillInfo>, String> {
    let root = project_root(&state, &project_id)?;
    let skills = discover_skills(&root);
    Ok(skills.iter().map(to_skill_info).collect())
}

#[tauri::command]
pub fn get_skill_body(
    state: State<'_, AppState>,
    project_id: String,
    name: String,
) -> Result<String, String> {
    let root = project_root(&state, &project_id)?;
    let skills = discover_skills(&root);
    let skill = skills
        .iter()
        .find(|s| s.name == name)
        .ok_or_else(|| format!("Skill not found: {}", name))?;
    let content = std::fs::read_to_string(&skill.path).map_err(|e| e.to_string())?;
    Ok(skill_body(&content).to_string())
}

#[tauri::command]
pub fn create_skill(
    state: State<'_, AppState>,
    project_id: String,
    name: String,
    description: String,
    body: String,
) -> Result<SkillInfo, String> {
    let root = project_root(&state, &project_id)?;

    // Sanitize name: lowercase, alphanumeric + hyphens
    let safe_name: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    if safe_name.is_empty() {
        return Err("Invalid skill name".to_string());
    }

    let skill_dir = root.join(".rustic/skills").join(&safe_name);
    std::fs::create_dir_all(&skill_dir).map_err(|e| e.to_string())?;

    let skill_md_path = skill_dir.join("SKILL.md");
    if skill_md_path.exists() {
        return Err(format!("Skill already exists: {}", safe_name));
    }

    let content = format!("---\nname: {}\ndescription: {}\n---\n\n{}", safe_name, description, body);
    std::fs::write(&skill_md_path, &content).map_err(|e| e.to_string())?;

    Ok(SkillInfo {
        name: safe_name,
        description,
        scope: "project".to_string(),
        allowed_tools: None,
    })
}

#[tauri::command]
pub fn delete_skill(
    state: State<'_, AppState>,
    project_id: String,
    name: String,
) -> Result<(), String> {
    let root = project_root(&state, &project_id)?;
    let skills = discover_skills(&root);
    let skill = skills
        .iter()
        .find(|s| s.name == name)
        .ok_or_else(|| format!("Skill not found: {}", name))?;

    // Remove the parent directory of SKILL.md
    if let Some(dir) = skill.path.parent() {
        std::fs::remove_dir_all(dir).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Install a skill from a GitHub repository (`owner/repo` or full URL).
///
/// Downloads the repo as a ZIP archive, finds all SKILL.md files inside it,
/// and copies each skill directory to `<project>/.rustic/skills/<name>/`.
/// Tries `main` branch first, falls back to `master`.
#[tauri::command]
pub async fn install_skill(
    state: State<'_, AppState>,
    project_id: String,
    source: String,
) -> Result<Vec<SkillInfo>, String> {
    let root = project_root(&state, &project_id)?;
    let (owner, repo) = parse_github_source(&source)?;

    // Try main, then master
    let zip_bytes = {
        let main_url = format!(
            "https://codeload.github.com/{}/{}/zip/refs/heads/main",
            owner, repo
        );
        match download_bytes(&main_url).await {
            Ok(b) => b,
            Err(_) => {
                let master_url = format!(
                    "https://codeload.github.com/{}/{}/zip/refs/heads/master",
                    owner, repo
                );
                download_bytes(&master_url).await?
            }
        }
    };

    extract_and_install_skills(&zip_bytes, &root)
}

// ─── helpers ────────────────────────────────────────────────────────────────

fn parse_github_source(source: &str) -> Result<(String, String), String> {
    let source = source.trim();

    // Strip protocol and host if it's a full URL
    let path = if source.starts_with("https://github.com/") {
        source.trim_start_matches("https://github.com/")
    } else if source.starts_with("github.com/") {
        source.trim_start_matches("github.com/")
    } else {
        source
    };

    // Remove .git suffix
    let path = path.trim_end_matches(".git");

    let parts: Vec<&str> = path.splitn(2, '/').collect();
    if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
        return Err(format!(
            "Invalid GitHub source: \"{}\" — expected owner/repo or https://github.com/owner/repo",
            source
        ));
    }
    Ok((parts[0].to_string(), parts[1].to_string()))
}

async fn download_bytes(url: &str) -> Result<Vec<u8>, String> {
    let client = reqwest::Client::builder()
        .user_agent("Rustic-Agent/0.1")
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .map_err(|e| e.to_string())?;

    let resp = client.get(url).send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!(
            "HTTP {} downloading skill archive from {}",
            resp.status(),
            url
        ));
    }
    resp.bytes().await.map(|b| b.to_vec()).map_err(|e| e.to_string())
}

fn extract_and_install_skills(zip_bytes: &[u8], project_root: &PathBuf) -> Result<Vec<SkillInfo>, String> {
    use std::io::Cursor;

    let cursor = Cursor::new(zip_bytes);
    let mut archive = zip::ZipArchive::new(cursor).map_err(|e| e.to_string())?;

    let skills_dir = project_root.join(".rustic/skills");
    std::fs::create_dir_all(&skills_dir).map_err(|e| e.to_string())?;

    // Find all SKILL.md entries and their parent directories
    let mut skill_entries: Vec<(String, String)> = Vec::new(); // (zip_dir_prefix, content)
    for i in 0..archive.len() {
        let mut file = archive.by_index(i).map_err(|e| e.to_string())?;
        let name = file.name().to_string();
        if name.ends_with("/SKILL.md") || name == "SKILL.md" {
            let mut content = String::new();
            file.read_to_string(&mut content).map_err(|e| e.to_string())?;
            skill_entries.push((name, content));
        }
    }

    if skill_entries.is_empty() {
        return Err("No SKILL.md files found in the archive".to_string());
    }

    let mut installed: Vec<SkillInfo> = Vec::new();
    let mut cursor = Cursor::new(zip_bytes);
    let mut archive2 = zip::ZipArchive::new(&mut cursor).map_err(|e| e.to_string())?;

    for (skill_md_path, skill_md_content) in &skill_entries {
        let Some((name, description, allowed_tools)) = parse_skill_frontmatter(skill_md_content) else {
            continue;
        };

        // Determine the directory prefix in the zip that contains this SKILL.md
        let dir_prefix = if let Some(idx) = skill_md_path.rfind('/') {
            &skill_md_path[..idx + 1]
        } else {
            ""
        };

        let skill_out_dir = skills_dir.join(&name);
        std::fs::create_dir_all(&skill_out_dir).map_err(|e| e.to_string())?;

        // Copy all files from that directory
        for i in 0..archive2.len() {
            let mut file = archive2.by_index(i).map_err(|e| e.to_string())?;
            let entry_name = file.name().to_string();
            if !dir_prefix.is_empty() && !entry_name.starts_with(dir_prefix) {
                continue;
            }
            if file.is_dir() {
                continue;
            }

            let relative = if dir_prefix.is_empty() {
                &entry_name
            } else {
                &entry_name[dir_prefix.len()..]
            };

            // Skip nested directories
            if relative.contains('/') {
                continue;
            }

            let out_path = skill_out_dir.join(relative);
            let mut content = Vec::new();
            file.read_to_end(&mut content).map_err(|e| e.to_string())?;
            std::fs::write(&out_path, content).map_err(|e| e.to_string())?;
        }

        installed.push(SkillInfo {
            name,
            description,
            scope: "project".to_string(),
            allowed_tools,
        });
    }

    // Write skills-lock.json
    let lock_path = project_root.join(".rustic/skills-lock.json");
    if let Ok(existing) = std::fs::read_to_string(&lock_path) {
        if let Ok(mut lock) = serde_json::from_str::<serde_json::Value>(&existing) {
            for skill in &installed {
                lock[&skill.name] = serde_json::json!({ "installed_at": chrono::Utc::now().to_rfc3339() });
            }
            let _ = std::fs::write(&lock_path, serde_json::to_string_pretty(&lock).unwrap_or_default());
        }
    } else {
        let mut lock = serde_json::json!({});
        for skill in &installed {
            lock[&skill.name] = serde_json::json!({ "installed_at": chrono::Utc::now().to_rfc3339() });
        }
        let _ = std::fs::write(&lock_path, serde_json::to_string_pretty(&lock).unwrap_or_default());
    }

    Ok(installed)
}

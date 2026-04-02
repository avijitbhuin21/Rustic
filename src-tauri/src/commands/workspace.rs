use crate::state::AppState;
use rustic_core::workspace::project::Project;
use rustic_db::models::ProjectRow;
use std::path::PathBuf;
use tauri::State;

/// Ensure `.rustic/` directory exists with an initial `memory.md` and add `.rustic` to `.gitignore`.
fn init_rustic_dir(project_root: &std::path::Path) {
    let rustic_dir = project_root.join(".rustic");

    // Create .rustic/ if it doesn't exist
    let _ = std::fs::create_dir_all(&rustic_dir);

    // Create memory.md with initial project context
    let memory_path = rustic_dir.join("memory.md");
    if !memory_path.exists() {
        let os_info = if cfg!(target_os = "windows") {
            "Windows"
        } else if cfg!(target_os = "macos") {
            "macOS"
        } else {
            "Linux"
        };

        // List immediate files and directories
        let mut entries: Vec<String> = Vec::new();
        if let Ok(dir) = std::fs::read_dir(project_root) {
            for entry in dir.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                // Skip hidden dirs/files and .rustic itself
                if name.starts_with('.') {
                    continue;
                }
                if entry.path().is_dir() {
                    entries.push(format!("  - {}/", name));
                } else {
                    entries.push(format!("  - {}", name));
                }
            }
        }
        entries.sort();
        let tree = if entries.is_empty() {
            "  (empty project)".to_string()
        } else {
            entries.join("\n")
        };

        let content = format!(
            "# Project Memory\n\n\
             ## Environment\n\
             - OS: {}\n\
             - Project path: {}\n\n\
             ## Project root structure\n\
             {}\n",
            os_info,
            project_root.display(),
            tree,
        );

        let _ = std::fs::write(&memory_path, content);
    }

    // Add .rustic to .gitignore if not already present
    let gitignore_path = project_root.join(".gitignore");
    let should_add = if gitignore_path.exists() {
        match std::fs::read_to_string(&gitignore_path) {
            Ok(content) => !content.lines().any(|line| {
                let trimmed = line.trim();
                trimmed == ".rustic" || trimmed == ".rustic/" || trimmed == "/.rustic"
            }),
            Err(_) => false,
        }
    } else {
        true // .gitignore doesn't exist, we'll create it
    };

    if should_add {
        use std::io::Write;
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&gitignore_path)
        {
            // Add a newline before our entry if the file doesn't end with one
            if gitignore_path.exists() {
                if let Ok(content) = std::fs::read_to_string(&gitignore_path) {
                    if !content.is_empty() && !content.ends_with('\n') {
                        let _ = writeln!(file);
                    }
                }
            }
            let _ = writeln!(file, ".rustic");
        }
    }
}

#[tauri::command]
pub async fn add_project(
    state: State<'_, AppState>,
    path: String,
) -> Result<Project, String> {
    let path = PathBuf::from(&path);
    if !path.exists() || !path.is_dir() {
        return Err(format!("Directory does not exist: {}", path.display()));
    }

    // Return early if already in workspace memory
    {
        let workspace = state.workspace.lock().map_err(|e| e.to_string())?;
        if let Some(existing) = workspace.projects.iter().find(|p| p.root_path == path) {
            return Ok(existing.clone());
        }
    }

    // Initialize .rustic/ directory with memory.md and update .gitignore
    init_rustic_dir(&path);

    // Reuse the stable project ID from DB (keyed by root_path) so the FK
    // constraint never breaks after an app restart that re-generates UUIDs.
    let existing_id = {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        db.get_project_by_path(&path.to_string_lossy())
            .ok()
            .flatten()
            .map(|p| p.id)
    };

    let mut project = Project::new(path);
    if let Some(id) = existing_id {
        project.id = id;
    }

    {
        let mut workspace = state.workspace.lock().map_err(|e| e.to_string())?;
        // Double-check in case another thread added it concurrently
        if let Some(existing) = workspace.projects.iter().find(|p| p.root_path == project.root_path) {
            return Ok(existing.clone());
        }
        workspace.projects.push(project.clone());
    }

    // Persist to DB so tasks can reference project_id via foreign key
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let _ = db.insert_project(&ProjectRow {
        id: project.id.clone(),
        name: project.name.clone(),
        root_path: project.root_path.to_string_lossy().to_string(),
        created_at: now,
        settings_json: None,
    });

    Ok(project)
}

#[tauri::command]
pub async fn remove_project(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<(), String> {
    let mut workspace = state.workspace.lock().map_err(|e| e.to_string())?;
    workspace.remove_project(&project_id);
    Ok(())
}

#[tauri::command]
pub async fn list_projects(
    state: State<'_, AppState>,
) -> Result<Vec<Project>, String> {
    let workspace = state.workspace.lock().map_err(|e| e.to_string())?;
    Ok(workspace.list_projects())
}

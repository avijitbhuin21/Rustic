use crate::state::AppState;
use rustic_core::workspace::project::Project;
use rustic_db::models::ProjectRow;
use std::path::PathBuf;
use tauri::{AppHandle, State};

/// Ensure `.rustic/` directory exists with an initial `memory.md` and add `.rustic` to `.gitignore`.
fn init_rustic_dir(project_root: &std::path::Path) {
    let rustic_dir = project_root.join(".rustic");

    // Create .rustic/ if it doesn't exist
    let _ = std::fs::create_dir_all(&rustic_dir);

    // Create memory.md with minimal initial content (project structure is
    // now generated dynamically in the system prompt, not stored here).
    let memory_path = rustic_dir.join("memory.md");
    if !memory_path.exists() {
        let content = "# Project Memory\n";
        let _ = rustic_core::io_util::atomic_write(&memory_path, content.as_bytes());
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
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
) -> Result<Project, String> {
    let path = PathBuf::from(&path);
    if !path.exists() || !path.is_dir() {
        return Err(format!("Directory does not exist: {}", path.display()));
    }

    // F-08: refuse to add a project rooted under a system / credentials path.
    // `init_rustic_dir` would otherwise try to write `.rustic/memory.md` and
    // append to `.gitignore` at that location. ACLs usually block writes
    // under C:\Windows / /etc, but a non-admin location like /var/log or
    // /tmp/shared could still be polluted.
    crate::path_scope::validate_writable_path(&path)
        .map_err(|e| format!("Cannot add project at this location: {}", e))?;

    // Return early if already in workspace memory
    {
        let workspace = state.workspace.lock().map_err(|e| e.to_string())?;
        if let Some(existing) = workspace.projects.iter().find(|p| p.root_path == path) {
            return Ok(existing.clone());
        }
    }

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

    // Start file system watcher for this project
    {
        let mut watcher = state.file_watcher.lock().map_err(|e| e.to_string())?;
        watcher.watch_project(
            &project.root_path.to_string_lossy(),
            app.clone(),
            Some(state.workspace_services.clone()),
        );
    }

    // M2: kick off the symbol-index build in the background as soon as
    // the project opens. Without this the first code-intel tool call
    // (find_symbol / outline / etc.) pays the full 30-90s warm-up tax
    // and returns partial results during it. Pre-building during the
    // project-open window means the user is busy navigating files
    // while we silently warm the index. `ensure_index_build_started`
    // is idempotent — a second project-open or a tool-call-time fallback
    // both no-op once the build has been claimed.
    {
        let services = state
            .workspace_services
            .get_or_create(&project.root_path);
        services.ensure_index_build_started();

        // M2.2: spawn a polling task that emits `workspace-index-status`
        // Tauri events as the build transitions between states. The
        // build runs on a std::thread; we can't subscribe to it directly,
        // but polling status() every 500ms gives the frontend a useful
        // signal without coupling the agent crate to Tauri. The task
        // self-terminates once the build reaches Ready or Failed.
        emit_index_status_for(app.clone(), project.id.clone(), services);
    }

    Ok(project)
}

/// M2.2 helper: spin up a tokio task that polls the symbol-index status
/// for `services` and emits a `workspace-index-status` Tauri event each
/// time it changes. Self-terminates when the status reaches Ready or
/// Failed (terminal states). Idempotent — calling twice for the same
/// project results in two pollers but their events are identical, so
/// the worst case is duplicate events the frontend dedupes on identity.
fn emit_index_status_for(
    app: tauri::AppHandle,
    project_id: String,
    services: std::sync::Arc<rustic_agent::WorkspaceServices>,
) {
    use rustic_agent::IndexStatus;
    use tauri::Emitter;

    tokio::spawn(async move {
        let mut last_emitted: Option<IndexStatus> = None;
        // Cap polling at ~5 minutes; if the build hasn't finished by
        // then something's deeply wrong and we'd rather stop emitting.
        for _ in 0..600 {
            let status = services.symbol_index().status();
            if Some(status) != last_emitted {
                let label = match status {
                    IndexStatus::NotStarted => "not_started",
                    IndexStatus::Building => "building",
                    IndexStatus::Ready => "ready",
                    IndexStatus::Failed => "failed",
                };
                let _ = app.emit(
                    "workspace-index-status",
                    WorkspaceIndexStatusEvent {
                        project_id: project_id.clone(),
                        status: label.to_string(),
                    },
                );
                last_emitted = Some(status);
            }
            if matches!(status, IndexStatus::Ready | IndexStatus::Failed) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
    });
}

#[derive(serde::Serialize, Clone, Debug)]
struct WorkspaceIndexStatusEvent {
    project_id: String,
    status: String,
}

#[tauri::command]
pub async fn remove_project(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<(), String> {
    // Find the project path before removing so we can stop its watcher
    let project_path = {
        let workspace = state.workspace.lock().map_err(|e| e.to_string())?;
        workspace
            .projects
            .iter()
            .find(|p| p.id == project_id)
            .map(|p| p.root_path.to_string_lossy().to_string())
    };

    {
        let mut workspace = state.workspace.lock().map_err(|e| e.to_string())?;
        workspace.remove_project(&project_id);
    }

    // Persist the removal — without this the project reappears on next app
    // start because startup rehydrates the workspace from `db.list_projects()`.
    {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        db.delete_project(&project_id).map_err(|e| e.to_string())?;
    }

    // Stop file system watcher for this project
    if let Some(path) = project_path {
        let mut watcher = state.file_watcher.lock().map_err(|e| e.to_string())?;
        watcher.unwatch_project(&path);
    }

    Ok(())
}

#[tauri::command]
pub async fn list_projects(
    state: State<'_, AppState>,
) -> Result<Vec<Project>, String> {
    let workspace = state.workspace.lock().map_err(|e| e.to_string())?;
    Ok(workspace.list_projects())
}

/// C3.7: list the git worktrees attached to a project. Returned in the
/// same order the libgit2 worktree-name iterator yields, with absolute
/// on-disk paths. Returns an empty Vec for non-git projects or when no
/// worktrees are attached — never an error in those cases.
#[derive(serde::Serialize, Clone, Debug)]
pub struct WorktreeInfo {
    pub name: String,
    pub path: String,
}

#[tauri::command]
pub async fn list_project_worktrees(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<Vec<WorktreeInfo>, String> {
    let project_root: std::path::PathBuf = {
        let workspace = state.workspace.lock().map_err(|e| e.to_string())?;
        workspace
            .list_projects()
            .into_iter()
            .find(|p| p.id.to_string() == project_id)
            .map(|p| p.root_path)
            .ok_or_else(|| format!("Project not found: {}", project_id))?
    };

    let repo = match rustic_git::GitRepo::open(&project_root) {
        Ok(r) => r,
        Err(_) => return Ok(Vec::new()),
    };
    let names = repo.worktrees().map_err(|e| e.to_string())?;
    let mut out = Vec::with_capacity(names.len());
    for n in names {
        if let Some(path) = repo.worktree_path(&n) {
            out.push(WorktreeInfo {
                name: n,
                path: path.to_string_lossy().into_owned(),
            });
        }
    }
    Ok(out)
}

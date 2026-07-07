//! Workspace commands: list / add / remove projects (+ worktrees).

use std::path::Path;

use serde_json::{json, Value};

use rustic_app::context::AppContext;
use rustic_app::path_scope::validate_writable_path;
use rustic_app::sync_ext::MutexExt;

use crate::api::{ok, parse, project_root, ApiError, PathArg, ProjectArg};
use crate::context::ServerContext;

pub async fn dispatch(
    ctx: &ServerContext,
    command: &str,
    args: &Value,
) -> Option<Result<Value, ApiError>> {
    Some(match command {
        "list_projects" => {
            let ws = ctx.state().workspace.lock_safe();
            ok(ws.list_projects())
        }
        "add_project" => match parse::<PathArg>(args) {
            Ok(a) => add_project(ctx, a.path).map_err(Into::into).and_then(ok),
            Err(e) => Err(e),
        },
        "remove_project" => match parse::<ProjectArg>(args) {
            Ok(a) => remove_project(ctx, &a.project_id).map(|_| json!(null)),
            Err(e) => Err(e),
        },
        "reorder_projects" => match parse::<ReorderProjectsArg>(args) {
            Ok(a) => reorder_projects(ctx, a.project_ids).map(|_| json!(null)),
            Err(e) => Err(e),
        },
        "list_project_worktrees" => match parse::<ProjectArg>(args) {
            Ok(a) => list_project_worktrees(ctx, &a.project_id),
            Err(e) => Err(e),
        },
        _ => return None,
    })
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct WorktreeInfo {
    name: String,
    path: String,
}

fn list_project_worktrees(ctx: &ServerContext, project_id: &str) -> Result<Value, ApiError> {
    let root = project_root(ctx, project_id)?;
    let project_root = std::path::PathBuf::from(root);

    let repo = match rustic_git::GitRepo::open(&project_root) {
        Ok(r) => r,
        Err(_) => return ok(Vec::<WorktreeInfo>::new()),
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
    ok(out)
}

// ---- helpers that touch state (mirror the desktop command bodies) ----

fn add_project(
    ctx: &ServerContext,
    path: String,
) -> Result<rustic_core::workspace::project::Project, String> {
    use rustic_core::workspace::project::Project;
    use rustic_db::models::ProjectRow;

    let path = std::path::PathBuf::from(&path);
    if !path.exists() || !path.is_dir() {
        return Err(format!("Directory does not exist: {}", path.display()));
    }
    validate_writable_path(&path)
        .map_err(|e| format!("Cannot add project at this location: {e}"))?;

    // Already present?
    {
        let ws = ctx.state().workspace.lock_safe();
        if let Some(existing) = ws.projects.iter().find(|p| p.root_path == path) {
            return Ok(existing.clone());
        }
    }

    init_rustic_dir(&path);

    let existing_id = {
        let db = ctx.state().db.lock_safe();
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
        let mut ws = ctx.state().workspace.lock_safe();
        if let Some(existing) = ws
            .projects
            .iter()
            .find(|p| p.root_path == project.root_path)
        {
            return Ok(existing.clone());
        }
        ws.projects.push(project.clone());
    }

    {
        let db = ctx.state().db.lock_safe();
        let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let sort_order = db.next_project_sort_order().unwrap_or(0);
        let _ = db.insert_project(&ProjectRow {
            id: project.id.clone(),
            name: project.name.clone(),
            root_path: project.root_path.to_string_lossy().to_string(),
            created_at: now,
            settings_json: None,
            sort_order,
        });
        let _ = db.set_project_archived(&project.id, false);
    }

    // Start the filesystem watcher, emitting through the WS hub.
    {
        let mut watcher = ctx.state().file_watcher.lock_safe();
        let emitter: std::sync::Arc<dyn rustic_app::EventEmitter> =
            std::sync::Arc::new(ctx.clone());
        watcher.watch_project(
            &project.root_path.to_string_lossy(),
            emitter,
            Some(ctx.state().workspace_services.clone()),
        );
    }

    Ok(project)
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReorderProjectsArg {
    project_ids: Vec<String>,
}

fn reorder_projects(ctx: &ServerContext, project_ids: Vec<String>) -> Result<(), ApiError> {
    {
        let mut ws = ctx.state().workspace.lock_safe();
        ws.reorder_projects(&project_ids);
    }
    {
        let db = ctx.state().db.lock_safe();
        db.reorder_projects(&project_ids)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn remove_project(ctx: &ServerContext, project_id: &str) -> Result<(), ApiError> {
    let project_path = {
        let ws = ctx.state().workspace.lock_safe();
        ws.projects
            .iter()
            .find(|p| p.id == project_id)
            .map(|p| p.root_path.to_string_lossy().to_string())
    };
    {
        let mut ws = ctx.state().workspace.lock_safe();
        ws.remove_project(project_id);
    }
    {
        let db = ctx.state().db.lock_safe();
        db.set_project_archived(project_id, true)
            .map_err(|e| e.to_string())?;
    }
    if let Some(path) = project_path {
        let mut watcher = ctx.state().file_watcher.lock_safe();
        watcher.unwatch_project(&path);
    }
    Ok(())
}

/// Minimal `.rustic/` seeding (memory folder + index + .gitignore line).
/// Mirrors `commands::workspace::init_rustic_dir`.
fn init_rustic_dir(project_root: &Path) {
    let rustic_dir = project_root.join(".rustic");
    let _ = std::fs::create_dir_all(&rustic_dir);
    let memory_dir = rustic_dir.join("memory");
    let _ = std::fs::create_dir_all(&memory_dir);
    let index_path = memory_dir.join("MEMORY.md");
    if !index_path.exists() {
        let _ = rustic_core::io_util::atomic_write(&index_path, b"# Memory Index\n");
    }
    let gitignore_path = project_root.join(".gitignore");
    let needs = match std::fs::read_to_string(&gitignore_path) {
        Ok(content) => !content.lines().any(|l| {
            let t = l.trim();
            t == ".rustic" || t == ".rustic/" || t == "/.rustic"
        }),
        Err(_) => true,
    };
    if needs {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&gitignore_path)
        {
            let _ = writeln!(f, ".rustic/");
        }
    }
}

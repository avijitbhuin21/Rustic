use ignore::WalkBuilder;
use rustic_core::workspace::file_tree::{self, FileNode};
use std::path::Path;

#[tauri::command]
pub async fn read_dir(path: String) -> Result<Vec<FileNode>, String> {
    let path = Path::new(&path);
    if !path.exists() || !path.is_dir() {
        return Err(format!("Directory does not exist: {}", path.display()));
    }

    file_tree::read_directory(path, 0).map_err(|e| e.to_string())
}

/// List every file under `root_path` as forward-slash relative paths, honoring
/// `.gitignore` and skipping a hardcoded set of heavy directories. Used by the
/// chat input's `@` mention picker to offer file references.
///
/// The walk stops early once `max_files` entries are collected so that huge
/// monorepos don't freeze the UI — callers should pick a cap around 5000.
#[tauri::command]
pub async fn list_project_files(
    root_path: String,
    max_files: Option<usize>,
) -> Result<Vec<String>, String> {
    let root = Path::new(&root_path);
    if !root.exists() || !root.is_dir() {
        return Err(format!("Directory does not exist: {}", root.display()));
    }

    // Belt-and-suspenders on top of .gitignore: these directories are rarely
    // useful in a file picker and often huge even when not gitignored.
    const HARD_SKIP: &[&str] = &[
        ".git", "node_modules", "target", "dist", "build", ".next",
        ".venv", "venv", "__pycache__", ".cache", ".turbo", ".parcel-cache",
    ];

    let cap = max_files.unwrap_or(5000);
    let mut out: Vec<String> = Vec::with_capacity(cap.min(1024));

    let walker = WalkBuilder::new(root)
        .hidden(false) // allow dotfiles like .env.example — .gitignore still applies
        .git_ignore(true)
        .git_exclude(true)
        .filter_entry(|entry| {
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                let name = entry.file_name().to_string_lossy();
                return !HARD_SKIP.iter().any(|s| *s == name);
            }
            true
        })
        .build();

    for entry in walker.flatten() {
        if out.len() >= cap {
            break;
        }
        let ft = match entry.file_type() {
            Some(t) => t,
            None => continue,
        };
        if !ft.is_file() {
            continue;
        }
        let rel = match entry.path().strip_prefix(root) {
            Ok(p) => p,
            Err(_) => continue,
        };
        // Normalize to forward slashes for display / filtering consistency.
        let s = rel.to_string_lossy().replace('\\', "/");
        if !s.is_empty() {
            out.push(s);
        }
    }

    Ok(out)
}

#[tauri::command]
pub async fn read_file_content(path: String) -> Result<String, String> {
    let path = Path::new(&path);
    if !path.exists() || !path.is_file() {
        return Err(format!("File does not exist: {}", path.display()));
    }

    std::fs::read_to_string(path).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn create_file(dir_path: String, name: String) -> Result<String, String> {
    let full_path = Path::new(&dir_path).join(&name);
    if full_path.exists() {
        return Err(format!("File already exists: {}", full_path.display()));
    }
    std::fs::write(&full_path, "").map_err(|e| e.to_string())?;
    Ok(full_path.to_string_lossy().to_string())
}

#[tauri::command]
pub async fn create_folder(dir_path: String, name: String) -> Result<String, String> {
    let full_path = Path::new(&dir_path).join(&name);
    if full_path.exists() {
        return Err(format!("Folder already exists: {}", full_path.display()));
    }
    std::fs::create_dir_all(&full_path).map_err(|e| e.to_string())?;
    Ok(full_path.to_string_lossy().to_string())
}

#[tauri::command]
pub async fn rename_entry(old_path: String, new_name: String) -> Result<String, String> {
    let old = Path::new(&old_path);
    if !old.exists() {
        return Err(format!("Path does not exist: {}", old.display()));
    }
    let new_path = old
        .parent()
        .ok_or_else(|| "Cannot determine parent directory".to_string())?
        .join(&new_name);
    if new_path.exists() {
        return Err(format!("Already exists: {}", new_path.display()));
    }
    std::fs::rename(old, &new_path).map_err(|e| e.to_string())?;
    Ok(new_path.to_string_lossy().to_string())
}

#[tauri::command]
pub async fn delete_entry(path: String) -> Result<(), String> {
    let p = Path::new(&path);
    if !p.exists() {
        return Err(format!("Path does not exist: {}", p.display()));
    }
    if p.is_dir() {
        std::fs::remove_dir_all(p).map_err(|e| e.to_string())
    } else {
        std::fs::remove_file(p).map_err(|e| e.to_string())
    }
}

#[tauri::command]
pub async fn reveal_in_file_manager(path: String) -> Result<(), String> {
    let p = Path::new(&path);
    if !p.exists() {
        return Err(format!("Path does not exist: {}", p.display()));
    }

    #[cfg(target_os = "windows")]
    {
        if p.is_dir() {
            std::process::Command::new("explorer")
                .arg(&path)
                .spawn()
                .map_err(|e| e.to_string())?;
        } else {
            std::process::Command::new("explorer")
                .arg(format!("/select,{}", path))
                .spawn()
                .map_err(|e| e.to_string())?;
        }
    }

    #[cfg(target_os = "macos")]
    {
        if p.is_dir() {
            std::process::Command::new("open")
                .arg(&path)
                .spawn()
                .map_err(|e| e.to_string())?;
        } else {
            std::process::Command::new("open")
                .args(["-R", &path])
                .spawn()
                .map_err(|e| e.to_string())?;
        }
    }

    #[cfg(target_os = "linux")]
    {
        let dir = if p.is_dir() {
            path.clone()
        } else {
            p.parent()
                .map(|pp| pp.to_string_lossy().to_string())
                .unwrap_or(path.clone())
        };
        std::process::Command::new("xdg-open")
            .arg(&dir)
            .spawn()
            .map_err(|e| e.to_string())?;
    }

    Ok(())
}

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

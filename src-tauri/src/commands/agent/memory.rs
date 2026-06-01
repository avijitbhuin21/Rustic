//! Per-project memory. The agent persists project-specific context to a
//! `.rustic/memory/` folder inside the project root, kept in git so it travels
//! with the project. Memory is *fragmented*: one fact per `.md` file, with a
//! one-line-per-entry index at `.rustic/memory/MEMORY.md` that is preloaded at
//! task start. The agent reads individual fragment files on demand instead of
//! loading the whole memory into context.
//!
//! A legacy single `.rustic/memory.md` file (the pre-folder layout) is still
//! read for backward compatibility.

use crate::state::AppState;
use crate::sync_ext::MutexExt;
use std::path::{Path, PathBuf};
use tauri::State;

/// Folder holding the fragmented memory files.
pub fn memory_dir(project_root: &Path) -> PathBuf {
    project_root.join(".rustic").join("memory")
}

/// Index file: one line per memory fragment. This is what gets preloaded.
pub fn memory_index_path(project_root: &Path) -> PathBuf {
    memory_dir(project_root).join("MEMORY.md")
}

/// Legacy single-file memory location (pre-folder layout).
pub fn legacy_memory_path(project_root: &Path) -> PathBuf {
    project_root.join(".rustic").join("memory.md")
}

/// Build the text injected as `[Project Memory]` on the first turn. Prefers the
/// folder index; falls back to the legacy single file. Returns `None` when
/// there's no non-empty memory to inject.
pub fn load_memory_preload(project_root: &Path) -> Option<String> {
    // Prefer the fragmented-memory index.
    if let Ok(idx) = std::fs::read_to_string(memory_index_path(project_root)) {
        let trimmed = idx.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    // Back-compat: an older project may still have the single memory.md.
    if let Ok(legacy) = std::fs::read_to_string(legacy_memory_path(project_root)) {
        let trimmed = legacy.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    None
}

fn project_root_for(
    state: &State<'_, AppState>,
    project_id: &str,
) -> Result<PathBuf, String> {
    let workspace = state.workspace.lock_safe();
    let project = workspace
        .list_projects()
        .into_iter()
        .find(|p| p.id.to_string() == project_id)
        .ok_or_else(|| "Project not found".to_string())?;
    Ok(project.root_path.clone())
}

/// Returns a human-readable view of the project's memory: the index followed by
/// each fragment file's contents. Falls back to the legacy single file. Used by
/// the memory viewer UI.
#[tauri::command]
pub fn get_memory(state: State<'_, AppState>, project_id: String) -> Result<String, String> {
    let root = project_root_for(&state, &project_id)?;
    let dir = memory_dir(&root);

    // Ensure the folder and an empty index exist so the agent always has a
    // place to write.
    if !dir.exists() {
        let _ = std::fs::create_dir_all(&dir);
    }
    let index_path = memory_index_path(&root);
    if !index_path.exists() {
        // Seed the index from a legacy memory.md if one exists, otherwise an
        // empty header.
        let seed = std::fs::read_to_string(legacy_memory_path(&root)).unwrap_or_default();
        let body = if seed.trim().is_empty() {
            "# Memory Index\n".to_string()
        } else {
            format!("# Memory Index\n\n{}\n", seed.trim())
        };
        let _ = rustic_core::io_util::atomic_write(&index_path, body.as_bytes());
    }

    let mut out = String::new();
    if let Ok(idx) = std::fs::read_to_string(&index_path) {
        out.push_str(idx.trim_end());
    }

    // Append each fragment file (everything except the index itself), sorted
    // for stable display.
    let mut fragments: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| {
                    p.extension().and_then(|e| e.to_str()) == Some("md")
                        && p.file_name().and_then(|n| n.to_str()) != Some("MEMORY.md")
                })
                .collect()
        })
        .unwrap_or_default();
    fragments.sort();
    for frag in fragments {
        if let Ok(content) = std::fs::read_to_string(&frag) {
            let name = frag
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("fragment.md");
            out.push_str(&format!("\n\n---\n## {}\n\n{}", name, content.trim()));
        }
    }
    Ok(out)
}

/// Wipe the project's memory: removes every fragment and resets the index.
/// Also clears the legacy single file if present.
#[tauri::command]
pub fn clear_memory(state: State<'_, AppState>, project_id: String) -> Result<(), String> {
    let root = project_root_for(&state, &project_id)?;
    let dir = memory_dir(&root);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
    }
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    rustic_core::io_util::atomic_write(memory_index_path(&root).as_path(), b"# Memory Index\n")
        .map_err(|e| e.to_string())?;
    // Drop the legacy file too so it doesn't resurrect on next preload.
    let legacy = legacy_memory_path(&root);
    if legacy.exists() {
        let _ = std::fs::remove_file(&legacy);
    }
    Ok(())
}

// ProjectDefaults extracted to ./project_defaults.rs

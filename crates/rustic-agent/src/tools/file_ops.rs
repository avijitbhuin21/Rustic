use super::{ToolContext, ToolOutput};
use crate::provider::ToolDef;
use crate::task::permissions::Action;
use anyhow::Result;
use serde_json::{json, Value};

pub fn definitions() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "read_file".into(),
            description: "Read the contents of a file at the given path.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Relative path from project root" }
                },
                "required": ["path"]
            }),
        },
        ToolDef {
            name: "write_file".into(),
            description: "Write content to a file, replacing its contents. Creates the file if it doesn't exist.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Relative path from project root" },
                    "content": { "type": "string", "description": "The full file content to write" }
                },
                "required": ["path", "content"]
            }),
        },
        ToolDef {
            name: "create_file".into(),
            description: "Create a new file with the given content. Fails if the file already exists.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Relative path from project root" },
                    "content": { "type": "string", "description": "The file content" }
                },
                "required": ["path", "content"]
            }),
        },
        ToolDef {
            name: "list_directory".into(),
            description: "List the contents of a directory.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Relative path from project root (empty or '.' for root)" }
                },
                "required": ["path"]
            }),
        },
    ]
}

pub async fn execute(name: &str, params: Value, context: &ToolContext) -> Result<ToolOutput> {
    match name {
        "read_file" => {
            if !context.check_permission(&Action::Read) {
                return Ok(ToolOutput { content: "Permission denied: read not allowed".into(), is_error: true });
            }
            let path = params["path"].as_str().unwrap_or("");
            let full_path = context.project_root.join(path);
            match std::fs::read_to_string(&full_path) {
                Ok(content) => Ok(ToolOutput { content, is_error: false }),
                Err(e) => Ok(ToolOutput { content: format!("Error reading file: {}", e), is_error: true }),
            }
        }
        "write_file" => {
            if !context.check_permission(&Action::Write) {
                return Ok(ToolOutput { content: "Permission denied: write not allowed".into(), is_error: true });
            }
            let path = params["path"].as_str().unwrap_or("");
            let content = params["content"].as_str().unwrap_or("");
            let full_path = context.project_root.join(path);
            // Snapshot before modifying
            if let Some(ref snapshot) = context.snapshot_fn {
                snapshot(&full_path);
            }
            if let Some(parent) = full_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            match std::fs::write(&full_path, content) {
                Ok(()) => Ok(ToolOutput { content: format!("Written to {}", path), is_error: false }),
                Err(e) => Ok(ToolOutput { content: format!("Error writing file: {}", e), is_error: true }),
            }
        }
        "create_file" => {
            if !context.check_permission(&Action::Write) {
                return Ok(ToolOutput { content: "Permission denied: write not allowed".into(), is_error: true });
            }
            let path = params["path"].as_str().unwrap_or("");
            let content = params["content"].as_str().unwrap_or("");
            let full_path = context.project_root.join(path);
            if full_path.exists() {
                return Ok(ToolOutput { content: format!("File already exists: {}", path), is_error: true });
            }
            // Snapshot before creating (will record as was_new=true)
            if let Some(ref snapshot) = context.snapshot_fn {
                snapshot(&full_path);
            }
            if let Some(parent) = full_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            match std::fs::write(&full_path, content) {
                Ok(()) => Ok(ToolOutput { content: format!("Created {}", path), is_error: false }),
                Err(e) => Ok(ToolOutput { content: format!("Error creating file: {}", e), is_error: true }),
            }
        }
        "list_directory" => {
            if !context.check_permission(&Action::Read) {
                return Ok(ToolOutput { content: "Permission denied: read not allowed".into(), is_error: true });
            }
            let path = params["path"].as_str().unwrap_or(".");
            let full_path = if path.is_empty() || path == "." {
                context.project_root.clone()
            } else {
                context.project_root.join(path)
            };
            match std::fs::read_dir(&full_path) {
                Ok(entries) => {
                    let mut items: Vec<String> = entries
                        .filter_map(|e| e.ok())
                        .map(|e| {
                            let name = e.file_name().to_string_lossy().to_string();
                            if e.path().is_dir() { format!("{}/", name) } else { name }
                        })
                        .collect();
                    items.sort();
                    Ok(ToolOutput { content: items.join("\n"), is_error: false })
                }
                Err(e) => Ok(ToolOutput { content: format!("Error listing directory: {}", e), is_error: true }),
            }
        }
        _ => Ok(ToolOutput { content: format!("Unknown file tool: {}", name), is_error: true }),
    }
}

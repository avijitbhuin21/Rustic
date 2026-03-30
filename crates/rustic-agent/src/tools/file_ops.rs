use super::{ToolContext, ToolOutput};
use crate::provider::ToolDef;
use crate::task::file_lock::LOCK_TIMEOUT_SECS;
use crate::task::permissions::{Action, PermissionLevel};
use crate::task::{PermissionOp, TaskEvent};
use anyhow::Result;
use serde_json::{json, Value};
use std::time::Duration;

/// Check whether a file path is sensitive. Returns Some(ToolOutput) to block/prompt, None to allow.
/// `full_path` is the absolute path; `rel_path` is the relative path string from the tool input.
async fn check_sensitive_path(
    rel_path: &str,
    full_path: &std::path::Path,
    context: &crate::tools::ToolContext,
) -> Option<ToolOutput> {
    let filename = full_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    let path_str = full_path.to_string_lossy().to_lowercase();
    let filename_lower = filename.to_lowercase();

    // ── Tier 1: always block ─────────────────────────────────────────────────
    let tier1_names = ["id_rsa", "id_ed25519", "id_ecdsa", "id_dsa"];
    let tier1_extensions = [".pem", ".p12", ".pfx"];

    if tier1_names.contains(&filename_lower.as_str()) {
        return Some(ToolOutput {
            content: format!(
                "SENSITIVE_FILE_BLOCKED: Access to '{}' is permanently denied. \
                 Private key files cannot be read or modified by the agent.",
                rel_path
            ),
            is_error: true,
        });
    }
    if tier1_extensions.iter().any(|ext| filename_lower.ends_with(ext)) {
        return Some(ToolOutput {
            content: format!(
                "SENSITIVE_FILE_BLOCKED: Access to '{}' is permanently denied. \
                 Certificate/key files cannot be read or modified by the agent.",
                rel_path
            ),
            is_error: true,
        });
    }
    // Standalone .key files (not build artifacts — check it's not something like keymap.key)
    if filename_lower == ".key" || (filename_lower.ends_with(".key") && !filename_lower.contains("map") && !filename_lower.contains("board")) {
        return Some(ToolOutput {
            content: format!(
                "SENSITIVE_FILE_BLOCKED: Access to '{}' is permanently denied. Key files cannot be accessed.",
                rel_path
            ),
            is_error: true,
        });
    }
    // AWS credentials
    if path_str.contains(".aws") && filename_lower == "credentials" {
        return Some(ToolOutput {
            content: "SENSITIVE_FILE_BLOCKED: Access to AWS credentials file is permanently denied.".to_string(),
            is_error: true,
        });
    }
    // Service account JSON
    if filename_lower.starts_with("service-account") && filename_lower.ends_with(".json") {
        return Some(ToolOutput {
            content: format!(
                "SENSITIVE_FILE_BLOCKED: Access to service account key '{}' is permanently denied.",
                rel_path
            ),
            is_error: true,
        });
    }

    // ── Check allowlist ──────────────────────────────────────────────────────
    // Paths in .rustic/allowed-files.txt skip tier-2/3
    let normalized = rel_path.replace('\\', "/");
    if context.allowed_paths.iter().any(|p| p.trim() == normalized.as_str()) {
        return None;
    }

    // ── Tier 2: sensitive patterns (require confirmation unless sensitive_files_allowed) ──
    let is_tier2 = {
        let n = &filename_lower;
        n == ".env"
            || n.starts_with(".env.")
            || n.starts_with("credentials")
            || n == "credentials"
            || n.starts_with("secrets")
            || n.ends_with(".secret")
            || n.ends_with(".token")
    };

    if is_tier2 {
        if context.sensitive_files_allowed {
            return None; // FullAuto allow-all mode
        }
        let approved = context
            .permission_broker
            .request(
                &context.event_tx,
                &context.task_id,
                crate::task::PermissionOp::SensitiveFile {
                    path: rel_path.to_string(),
                    tier: 2,
                    reason: "This file may contain secrets or credentials.".to_string(),
                },
            )
            .await;
        if !approved {
            return Some(ToolOutput {
                content: format!(
                    "PERMISSION_DENIED: Access to sensitive file '{}' was denied.",
                    rel_path
                ),
                is_error: true,
            });
        }
        return None;
    }

    // ── Tier 3: gitignored files ─────────────────────────────────────────────
    if context.sensitive_files_allowed {
        return None; // FullAuto allow-all skips tier-3 too
    }

    // Build gitignore matcher from project root
    let gitignore_path = context.project_root.join(".gitignore");
    if gitignore_path.exists() {
        use ignore::gitignore::GitignoreBuilder;
        let mut builder = GitignoreBuilder::new(&context.project_root);
        let _ = builder.add(&gitignore_path);
        if let Ok(gi) = builder.build() {
            let match_result = gi.matched_path_or_any_parents(full_path, full_path.is_dir());
            if match_result.is_ignore() {
                let approved = context
                    .permission_broker
                    .request(
                        &context.event_tx,
                        &context.task_id,
                        crate::task::PermissionOp::SensitiveFile {
                            path: rel_path.to_string(),
                            tier: 3,
                            reason: "This file is listed in .gitignore.".to_string(),
                        },
                    )
                    .await;
                if !approved {
                    return Some(ToolOutput {
                        content: format!(
                            "PERMISSION_DENIED: Access to gitignored file '{}' was denied.",
                            rel_path
                        ),
                        is_error: true,
                    });
                }
            }
        }
    }

    None // allow
}

// Context bounds for error responses
const CONTEXT_LINES: usize = 150;
const MAX_CONTEXT_LINES: usize = 300;
const MAX_CONTEXT_BYTES: usize = 8 * 1024;

pub fn definitions() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "read_file".into(),
            description: "Read a file's contents. Use start_line/end_line to read a specific \
                          section (1-indexed, inclusive). Never read more than 300 lines at once — \
                          use grep or run_command to locate content first.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Relative path from project root" },
                    "start_line": {
                        "type": "integer",
                        "description": "First line to read (1-indexed). Omit to read from the beginning."
                    },
                    "end_line": {
                        "type": "integer",
                        "description": "Last line to read (1-indexed, inclusive). Omit to read to the end."
                    }
                },
                "required": ["path"]
            }),
        },
        ToolDef {
            name: "create_file".into(),
            description: "Create a new file with the given content. \
                          Fails with FILE_HAS_CONTENT if the file already exists — \
                          use edit_file or apply_patch to modify existing files.".into(),
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
            name: "edit_file".into(),
            description: "Edit a file by replacing the first occurrence of old_string with \
                          new_string. The match is exact — whitespace and indentation must match. \
                          Returns STALE_READ with file context if old_string is not found. \
                          Returns ALREADY_APPLIED if the replacement is already in place.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Relative path from project root" },
                    "old_string": {
                        "type": "string",
                        "description": "The exact text to replace (must match exactly, including whitespace and indentation)"
                    },
                    "new_string": {
                        "type": "string",
                        "description": "The replacement text"
                    },
                    "hint_line": {
                        "type": "integer",
                        "description": "Approximate line number of old_string (1-indexed). \
                                       Improves STALE_READ context when the match fails."
                    }
                },
                "required": ["path", "old_string", "new_string"]
            }),
        },
        ToolDef {
            name: "apply_patch".into(),
            description: "Apply multiple find-and-replace hunks to a file atomically. \
                          All hunks must succeed or none are applied (rollback on failure). \
                          Each hunk uses exact string matching.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Relative path from project root" },
                    "hunks": {
                        "type": "array",
                        "description": "List of [{old_string, new_string}] hunks applied in order",
                        "items": {
                            "type": "object",
                            "properties": {
                                "old_string": { "type": "string", "description": "Exact text to replace" },
                                "new_string": { "type": "string", "description": "Replacement text" }
                            },
                            "required": ["old_string", "new_string"]
                        }
                    }
                },
                "required": ["path", "hunks"]
            }),
        },
        ToolDef {
            name: "insert_lines".into(),
            description: "Insert content after a specific line number in a file. \
                          Use after_line=0 to insert before the first line.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Relative path from project root" },
                    "after_line": {
                        "type": "integer",
                        "description": "Insert after this line (1-indexed). Use 0 to insert before line 1."
                    },
                    "content": {
                        "type": "string",
                        "description": "Content to insert (may include newlines)"
                    }
                },
                "required": ["path", "after_line", "content"]
            }),
        },
        ToolDef {
            name: "delete_lines".into(),
            description: "Delete a range of lines from a file (1-indexed, inclusive).".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Relative path from project root" },
                    "start_line": { "type": "integer", "description": "First line to delete (1-indexed)" },
                    "end_line": { "type": "integer", "description": "Last line to delete (1-indexed, inclusive)" }
                },
                "required": ["path", "start_line", "end_line"]
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
        "read_file" => execute_read_file(params, context).await,
        "create_file" => execute_create_file(params, context).await,
        "edit_file" => execute_edit_file(params, context).await,
        "apply_patch" => execute_apply_patch(params, context).await,
        "insert_lines" => execute_insert_lines(params, context).await,
        "delete_lines" => execute_delete_lines(params, context).await,
        "list_directory" => execute_list_directory(params, context).await,
        _ => Ok(ToolOutput { content: format!("Unknown file tool: {}", name), is_error: true }),
    }
}

// ─── read_file ────────────────────────────────────────────────────────────────

async fn execute_read_file(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    if !context.check_permission(&Action::Read) {
        return Ok(ToolOutput {
            content: "PERMISSION_DENIED: Read not allowed in current permission mode.".into(),
            is_error: true,
        });
    }
    let path = params["path"].as_str().unwrap_or("");
    let start_line = params["start_line"].as_u64().map(|n| n as usize);
    let end_line = params["end_line"].as_u64().map(|n| n as usize);
    let full_path = context.project_root.join(path);

    if let Some(blocked) = check_sensitive_path(path, &full_path, context).await {
        return Ok(blocked);
    }

    match std::fs::read_to_string(&full_path) {
        Ok(content) => {
            if start_line.is_none() && end_line.is_none() {
                Ok(ToolOutput { content, is_error: false })
            } else {
                let lines: Vec<&str> = content.lines().collect();
                let start = start_line.map(|n| n.saturating_sub(1)).unwrap_or(0).min(lines.len());
                let end = end_line.map(|n| n.min(lines.len())).unwrap_or(lines.len());
                let end = end.max(start);
                let selected: Vec<String> = lines[start..end]
                    .iter()
                    .enumerate()
                    .map(|(i, line)| format!("{}: {}", start + i + 1, *line))
                    .collect();
                Ok(ToolOutput { content: selected.join("\n"), is_error: false })
            }
        }
        Err(e) => Ok(ToolOutput { content: format!("Error reading file: {}", e), is_error: true }),
    }
}

// ─── create_file ──────────────────────────────────────────────────────────────

async fn execute_create_file(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    let path = params["path"].as_str().unwrap_or("");
    if context.permissions == PermissionLevel::Chat {
        return Ok(ToolOutput {
            content: "PERMISSION_DENIED: File writes are not allowed in Chat mode.".into(),
            is_error: true,
        });
    }
    let full_path = context.project_root.join(path);

    if let Some(blocked) = check_sensitive_path(path, &full_path, context).await {
        return Ok(blocked);
    }

    if context.needs_write_approval() {
        let approved = context
            .permission_broker
            .request(&context.event_tx, &context.task_id, PermissionOp::CreateFile(path.to_string()))
            .await;
        if !approved {
            return Ok(ToolOutput {
                content: "PERMISSION_DENIED: User denied file creation.".into(),
                is_error: true,
            });
        }
    }
    let content = params["content"].as_str().unwrap_or("");
    if full_path.exists() {
        return Ok(ToolOutput {
            content: format!(
                "FILE_HAS_CONTENT: File '{}' already exists. Use edit_file or apply_patch to modify it.",
                path
            ),
            is_error: true,
        });
    }
    if let Some(ref snapshot) = context.snapshot_fn {
        snapshot(&full_path);
    }
    if let Some(parent) = full_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::write(&full_path, content) {
        Ok(()) => {
            maybe_emit_memory_updated(path, context);
            Ok(ToolOutput { content: format!("Created {}", path), is_error: false })
        }
        Err(e) => Ok(ToolOutput { content: format!("Error creating file: {}", e), is_error: true }),
    }
}

// ─── edit_file ────────────────────────────────────────────────────────────────

async fn execute_edit_file(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    let path = params["path"].as_str().unwrap_or("");
    if context.permissions == PermissionLevel::Chat {
        return Ok(ToolOutput {
            content: "PERMISSION_DENIED: File writes are not allowed in Chat mode.".into(),
            is_error: true,
        });
    }
    let full_path_for_check = context.project_root.join(path);
    if let Some(blocked) = check_sensitive_path(path, &full_path_for_check, context).await {
        return Ok(blocked);
    }

    if context.needs_write_approval() {
        let approved = context
            .permission_broker
            .request(&context.event_tx, &context.task_id, PermissionOp::WriteFile(path.to_string()))
            .await;
        if !approved {
            return Ok(ToolOutput {
                content: "PERMISSION_DENIED: User denied file edit.".into(),
                is_error: true,
            });
        }
    }

    let old_string = match params["old_string"].as_str() {
        Some(s) => s.to_string(),
        None => return Ok(ToolOutput { content: "old_string is required".into(), is_error: true }),
    };
    let new_string = params["new_string"].as_str().unwrap_or("").to_string();
    let hint_line = params["hint_line"].as_u64().map(|n| n as usize);

    let full_path = context.project_root.join(path);

    // Acquire per-file lock before any I/O
    let file_lock = context.file_lock.get_lock(&full_path);
    let _guard = match tokio::time::timeout(Duration::from_secs(LOCK_TIMEOUT_SECS), file_lock.lock()).await {
        Ok(guard) => guard,
        Err(_) => return Ok(ToolOutput {
            content: format!(
                "LOCK_TIMEOUT: Could not acquire lock on '{}' within {} seconds. Retry after a moment.",
                path, LOCK_TIMEOUT_SECS
            ),
            is_error: true,
        }),
    };

    // Read file
    let content = match std::fs::read_to_string(&full_path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ToolOutput {
                content: format!(
                    "CONTENT_DELETED: File '{}' does not exist. It may have been deleted.",
                    path
                ),
                is_error: true,
            });
        }
        Err(e) => return Ok(ToolOutput { content: format!("Error reading file: {}", e), is_error: true }),
    };

    // Check if old_string is present
    if !content.contains(old_string.as_str()) {
        // Idempotency: new_string already in file and old_string gone → edit was already applied
        if content.contains(new_string.as_str()) && !new_string.is_empty() {
            return Ok(ToolOutput {
                content: format!(
                    "ALREADY_APPLIED: The replacement text is already present in '{}'. No changes made.",
                    path
                ),
                is_error: false,
            });
        }
        // STALE_READ: provide ±150 lines of context around hint_line
        let ctx = build_stale_read_context(&content, hint_line);
        return Ok(ToolOutput {
            content: format!(
                "STALE_READ: old_string not found in '{}'. The file has changed since you last read it.\n\
                 Use the context below to find the correct text and retry.\n\n{}",
                path, ctx
            ),
            is_error: true,
        });
    }

    // Apply replacement (first occurrence only)
    let new_content = content.replacen(old_string.as_str(), new_string.as_str(), 1);

    if let Some(ref snapshot_fn) = context.snapshot_fn {
        snapshot_fn(&full_path);
    }

    match std::fs::write(&full_path, &new_content) {
        Ok(()) => {
            maybe_emit_memory_updated(path, context);
            Ok(ToolOutput { content: format!("Edited {}", path), is_error: false })
        }
        Err(e) => Ok(ToolOutput { content: format!("Error writing file: {}", e), is_error: true }),
    }
}

// ─── apply_patch ──────────────────────────────────────────────────────────────

async fn execute_apply_patch(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    let path = params["path"].as_str().unwrap_or("");
    if context.permissions == PermissionLevel::Chat {
        return Ok(ToolOutput {
            content: "PERMISSION_DENIED: File writes are not allowed in Chat mode.".into(),
            is_error: true,
        });
    }
    let full_path_for_check = context.project_root.join(path);
    if let Some(blocked) = check_sensitive_path(path, &full_path_for_check, context).await {
        return Ok(blocked);
    }

    if context.needs_write_approval() {
        let approved = context
            .permission_broker
            .request(&context.event_tx, &context.task_id, PermissionOp::WriteFile(path.to_string()))
            .await;
        if !approved {
            return Ok(ToolOutput {
                content: "PERMISSION_DENIED: User denied file patch.".into(),
                is_error: true,
            });
        }
    }

    let hunks = match params["hunks"].as_array() {
        Some(h) => h.clone(),
        None => return Ok(ToolOutput { content: "hunks array is required".into(), is_error: true }),
    };
    if hunks.is_empty() {
        return Ok(ToolOutput { content: "No hunks provided".into(), is_error: true });
    }

    let full_path = context.project_root.join(path);

    // Acquire per-file lock before any I/O
    let file_lock = context.file_lock.get_lock(&full_path);
    let _guard = match tokio::time::timeout(Duration::from_secs(LOCK_TIMEOUT_SECS), file_lock.lock()).await {
        Ok(guard) => guard,
        Err(_) => return Ok(ToolOutput {
            content: format!(
                "LOCK_TIMEOUT: Could not acquire lock on '{}' within {} seconds. Retry after a moment.",
                path, LOCK_TIMEOUT_SECS
            ),
            is_error: true,
        }),
    };

    // Read file
    let original = match std::fs::read_to_string(&full_path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ToolOutput {
                content: format!("CONTENT_DELETED: File '{}' does not exist.", path),
                is_error: true,
            });
        }
        Err(e) => return Ok(ToolOutput { content: format!("Error reading file: {}", e), is_error: true }),
    };

    // Apply all hunks to in-memory copy — no writes until all succeed
    let mut current = original.clone();
    for (i, hunk) in hunks.iter().enumerate() {
        let old = match hunk["old_string"].as_str() {
            Some(s) => s,
            None => return Ok(ToolOutput {
                content: format!("Hunk {} is missing old_string", i),
                is_error: true,
            }),
        };
        let new = hunk["new_string"].as_str().unwrap_or("");

        if !current.contains(old) {
            if current.contains(new) && !new.is_empty() {
                return Ok(ToolOutput {
                    content: format!(
                        "ALREADY_APPLIED: Hunk {} replacement is already present in '{}'. \
                         No changes applied.",
                        i, path
                    ),
                    is_error: false,
                });
            }
            return Ok(ToolOutput {
                content: format!(
                    "STALE_READ: Hunk {} old_string not found in '{}'. \
                     No changes applied (all hunks rolled back).",
                    i, path
                ),
                is_error: true,
            });
        }
        current = current.replacen(old, new, 1);
    }

    // All hunks applied in memory — snapshot and write atomically
    if let Some(ref snapshot_fn) = context.snapshot_fn {
        snapshot_fn(&full_path);
    }

    match std::fs::write(&full_path, &current) {
        Ok(()) => {
            maybe_emit_memory_updated(path, context);
            Ok(ToolOutput {
                content: format!("Patched {} ({} hunk(s) applied)", path, hunks.len()),
                is_error: false,
            })
        }
        Err(e) => Ok(ToolOutput { content: format!("Error writing file: {}", e), is_error: true }),
    }
}

// ─── insert_lines ─────────────────────────────────────────────────────────────

async fn execute_insert_lines(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    let path = params["path"].as_str().unwrap_or("");
    if context.permissions == PermissionLevel::Chat {
        return Ok(ToolOutput {
            content: "PERMISSION_DENIED: File writes are not allowed in Chat mode.".into(),
            is_error: true,
        });
    }
    let full_path = context.project_root.join(path);

    if let Some(blocked) = check_sensitive_path(path, &full_path, context).await {
        return Ok(blocked);
    }

    if context.needs_write_approval() {
        let approved = context
            .permission_broker
            .request(&context.event_tx, &context.task_id, PermissionOp::WriteFile(path.to_string()))
            .await;
        if !approved {
            return Ok(ToolOutput {
                content: "PERMISSION_DENIED: User denied line insertion.".into(),
                is_error: true,
            });
        }
    }

    let after_line = params["after_line"].as_u64().unwrap_or(0) as usize;
    let insert_content = params["content"].as_str().unwrap_or("");

    let file_lock = context.file_lock.get_lock(&full_path);
    let _guard = match tokio::time::timeout(Duration::from_secs(LOCK_TIMEOUT_SECS), file_lock.lock()).await {
        Ok(guard) => guard,
        Err(_) => return Ok(ToolOutput {
            content: format!(
                "LOCK_TIMEOUT: Could not acquire lock on '{}' within {} seconds.",
                path, LOCK_TIMEOUT_SECS
            ),
            is_error: true,
        }),
    };

    let content = match std::fs::read_to_string(&full_path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ToolOutput {
                content: format!("CONTENT_DELETED: File '{}' does not exist.", path),
                is_error: true,
            });
        }
        Err(e) => return Ok(ToolOutput { content: format!("Error reading file: {}", e), is_error: true }),
    };

    let trailing_newline = content.ends_with('\n');
    let mut lines: Vec<&str> = content.lines().collect();
    let insert_at = after_line.min(lines.len());

    // Split insert_content into lines and splice in
    let insert_lines: Vec<&str> = insert_content.lines().collect();
    let insert_count = insert_lines.len();
    for (i, line) in insert_lines.into_iter().enumerate() {
        lines.insert(insert_at + i, line);
    }

    let mut new_content = lines.join("\n");
    if trailing_newline {
        new_content.push('\n');
    }

    if let Some(ref snapshot_fn) = context.snapshot_fn {
        snapshot_fn(&full_path);
    }

    match std::fs::write(&full_path, &new_content) {
        Ok(()) => {
            maybe_emit_memory_updated(path, context);
            Ok(ToolOutput {
                content: format!(
                    "Inserted {} line(s) after line {} in {}",
                    insert_count, after_line, path
                ),
                is_error: false,
            })
        }
        Err(e) => Ok(ToolOutput { content: format!("Error writing file: {}", e), is_error: true }),
    }
}

// ─── delete_lines ─────────────────────────────────────────────────────────────

async fn execute_delete_lines(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    let path = params["path"].as_str().unwrap_or("");
    if context.permissions == PermissionLevel::Chat {
        return Ok(ToolOutput {
            content: "PERMISSION_DENIED: File writes are not allowed in Chat mode.".into(),
            is_error: true,
        });
    }
    let full_path = context.project_root.join(path);

    if let Some(blocked) = check_sensitive_path(path, &full_path, context).await {
        return Ok(blocked);
    }

    if context.needs_write_approval() {
        let approved = context
            .permission_broker
            .request(&context.event_tx, &context.task_id, PermissionOp::WriteFile(path.to_string()))
            .await;
        if !approved {
            return Ok(ToolOutput {
                content: "PERMISSION_DENIED: User denied line deletion.".into(),
                is_error: true,
            });
        }
    }

    let start_line = params["start_line"].as_u64().unwrap_or(1) as usize;
    let end_line = params["end_line"].as_u64().unwrap_or(start_line as u64) as usize;

    if start_line == 0 || start_line > end_line {
        return Ok(ToolOutput {
            content: format!(
                "Invalid line range: {}-{} (start_line must be ≥ 1 and ≤ end_line)",
                start_line, end_line
            ),
            is_error: true,
        });
    }

    let file_lock = context.file_lock.get_lock(&full_path);
    let _guard = match tokio::time::timeout(Duration::from_secs(LOCK_TIMEOUT_SECS), file_lock.lock()).await {
        Ok(guard) => guard,
        Err(_) => return Ok(ToolOutput {
            content: format!(
                "LOCK_TIMEOUT: Could not acquire lock on '{}' within {} seconds.",
                path, LOCK_TIMEOUT_SECS
            ),
            is_error: true,
        }),
    };

    let content = match std::fs::read_to_string(&full_path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ToolOutput {
                content: format!("CONTENT_DELETED: File '{}' does not exist.", path),
                is_error: true,
            });
        }
        Err(e) => return Ok(ToolOutput { content: format!("Error reading file: {}", e), is_error: true }),
    };

    let trailing_newline = content.ends_with('\n');
    let lines: Vec<&str> = content.lines().collect();

    if start_line > lines.len() {
        return Ok(ToolOutput {
            content: format!(
                "Line {} is beyond end of file ({} lines total in {})",
                start_line,
                lines.len(),
                path
            ),
            is_error: true,
        });
    }

    let actual_end = end_line.min(lines.len());
    let deleted_count = actual_end - start_line + 1;

    // Collect lines before and after the deleted range
    let new_lines: Vec<&str> = lines[..start_line - 1]
        .iter()
        .chain(lines[actual_end..].iter())
        .copied()
        .collect();

    let mut new_content = new_lines.join("\n");
    if trailing_newline && !new_content.is_empty() {
        new_content.push('\n');
    }

    if let Some(ref snapshot_fn) = context.snapshot_fn {
        snapshot_fn(&full_path);
    }

    match std::fs::write(&full_path, &new_content) {
        Ok(()) => {
            maybe_emit_memory_updated(path, context);
            Ok(ToolOutput {
                content: format!(
                    "Deleted {} line(s) (lines {}-{}) from {}",
                    deleted_count, start_line, actual_end, path
                ),
                is_error: false,
            })
        }
        Err(e) => Ok(ToolOutput { content: format!("Error writing file: {}", e), is_error: true }),
    }
}

// ─── list_directory ───────────────────────────────────────────────────────────

async fn execute_list_directory(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    if !context.check_permission(&Action::Read) {
        return Ok(ToolOutput {
            content: "PERMISSION_DENIED: Read not allowed in current permission mode.".into(),
            is_error: true,
        });
    }
    let path = params["path"].as_str().unwrap_or(".");
    let full_path = if path.is_empty() || path == "." {
        context.project_root.clone()
    } else {
        context.project_root.join(path)
    };

    if let Some(blocked) = check_sensitive_path(path, &full_path, context).await {
        return Ok(blocked);
    }
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

// ─── helpers ──────────────────────────────────────────────────────────────────

/// Emits MemoryUpdated if the path targets .rustic/memory.md.
fn maybe_emit_memory_updated(path: &str, ctx: &ToolContext) {
    let normalized = path.replace('\\', "/");
    if normalized.ends_with(".rustic/memory.md") {
        let _ = ctx.event_tx.send(TaskEvent::MemoryUpdated { task_id: ctx.task_id.clone() });
    }
}

/// Build a context block for STALE_READ errors.
/// Returns ±CONTEXT_LINES lines around `hint_line` (1-indexed), or the first MAX_CONTEXT_LINES
/// lines if no hint is available. Capped at MAX_CONTEXT_LINES lines and MAX_CONTEXT_BYTES bytes.
fn build_stale_read_context(content: &str, hint_line: Option<usize>) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();

    let (start, end) = if let Some(hl) = hint_line {
        let center = hl.saturating_sub(1); // convert to 0-indexed
        let start = center.saturating_sub(CONTEXT_LINES);
        let end = (center + CONTEXT_LINES + 1).min(total);
        (start, end)
    } else {
        (0, MAX_CONTEXT_LINES.min(total))
    };

    let end = end.min(start + MAX_CONTEXT_LINES);

    let mut result = String::new();
    let mut byte_count = 0usize;

    for (i, line) in lines[start..end].iter().enumerate() {
        let formatted = format!("{}: {}\n", start + i + 1, line);
        if byte_count + formatted.len() > MAX_CONTEXT_BYTES {
            result.push_str(&format!("[... truncated at {}KB]\n", MAX_CONTEXT_BYTES / 1024));
            break;
        }
        result.push_str(&formatted);
        byte_count += formatted.len();
    }

    // Annotation showing position within the file
    if start > 0 || end < total {
        format!(
            "(showing lines {}-{} of {} total)\n{}",
            start + 1,
            end,
            total,
            result
        )
    } else {
        result
    }
}

use super::{ToolContext, ToolOutput};
use crate::provider::ToolDef;
use crate::task::permissions::{Action, PermissionLevel};
use crate::task::{PermissionOp, TaskEvent};
use anyhow::Result;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// Process-wide cache of `Path::canonicalize` results. Resolving an allowed
/// root used to stat every project_root on every tool invocation; in Global
/// orchestrator scope with N projects that was N+1 syscalls per call. Project
/// roots are stable directories — once we've resolved one, the answer is
/// good for the rest of the session. Bounded only by the number of distinct
/// project roots ever opened (rarely >100).
fn canonicalize_cache() -> &'static Mutex<HashMap<PathBuf, PathBuf>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, PathBuf>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn canonicalize_cached(p: &Path) -> Option<PathBuf> {
    {
        if let Ok(map) = canonicalize_cache().lock() {
            if let Some(hit) = map.get(p) {
                return Some(hit.clone());
            }
        }
    }
    match p.canonicalize() {
        Ok(canon) => {
            if let Ok(mut map) = canonicalize_cache().lock() {
                map.insert(p.to_path_buf(), canon.clone());
            }
            Some(canon)
        }
        Err(_) => None,
    }
}

/// Resolve `rel_path` against the active scope, then verify the result is
/// contained within an allowed root. Returns the joined (un-canonicalized,
/// since the file may not exist yet) path on success.
///
/// In Global orchestrator scope (`context.is_global`), the model is talking
/// about projects collectively, so we widen the allowed-roots set to *every*
/// registered workspace project — otherwise the orchestrator can't read its
/// own projects' files and falls back to `cat` via run_command, which is
/// uglier and bypasses the sensitive-file check.
///
/// Outside Global scope, only the active project root is allowed. Path
/// traversal (`../../etc/passwd`) and absolute paths into unrelated trees
/// are still rejected.
fn resolve_with_scope(
    context: &ToolContext,
    rel_path: &str,
) -> std::result::Result<std::path::PathBuf, ToolOutput> {
    // Build the candidate joined path. Absolute paths replace the base on
    // both Windows and Unix (Path::join semantics), so this also works for
    // the Global orchestrator passing `D:\Projects\foo\bar.py`.
    let joined = context.project_root.join(rel_path);

    // Walk up to find the deepest existing ancestor — canonicalize fails on
    // not-yet-existing paths (e.g. for create_file).
    let mut probe = joined.clone();
    let canon_existing = loop {
        if let Ok(c) = probe.canonicalize() {
            break c;
        }
        if !probe.pop() {
            return Err(ToolOutput {
                content: format!(
                    "PATH_SCOPE_VIOLATION: '{}' could not be resolved to a path inside an allowed project.",
                    rel_path
                ),
                is_error: true,
            });
        }
    };

    // Build the allowed-roots list. Always include the active project_root.
    // In Global scope, also include every workspace project the orchestrator
    // can see. Project roots are canonicalized once and cached process-wide
    // so back-to-back tool calls don't repeatedly stat the same directories.
    let mut allowed_roots: Vec<std::path::PathBuf> = Vec::new();
    let canon_active =
        canonicalize_cached(&context.project_root).unwrap_or_else(|| context.project_root.to_path_buf());
    allowed_roots.push(canon_active);

    if context.is_global {
        if let Some(host) = &context.orchestrator_host {
            if let Ok(projects) = host.list_projects() {
                for p in projects {
                    let raw = std::path::PathBuf::from(&p.root_path);
                    let canon = canonicalize_cached(&raw).unwrap_or(raw);
                    allowed_roots.push(canon);
                }
            }
        }
    }

    if !allowed_roots.iter().any(|root| canon_existing.starts_with(root)) {
        return Err(ToolOutput {
            content: format!(
                "PATH_SCOPE_VIOLATION: '{}' resolves outside the {}.",
                rel_path,
                if context.is_global {
                    "set of registered workspace projects"
                } else {
                    "project root"
                }
            ),
            is_error: true,
        });
    }

    Ok(joined)
}

/// Back-compat wrapper for callers that don't yet have a ToolContext to hand.
/// New code should call `resolve_with_scope` directly.
fn resolve_within_project(
    project_root: &std::path::Path,
    rel_path: &str,
) -> std::result::Result<std::path::PathBuf, ToolOutput> {
    let joined = project_root.join(rel_path);
    let mut probe = joined.clone();
    let canon_existing = loop {
        if let Ok(c) = probe.canonicalize() {
            break c;
        }
        if !probe.pop() {
            return Err(ToolOutput {
                content: format!(
                    "PATH_SCOPE_VIOLATION: '{}' could not be resolved to a path inside the project.",
                    rel_path
                ),
                is_error: true,
            });
        }
    };
    let canon_root = canonicalize_cached(project_root).unwrap_or_else(|| project_root.to_path_buf());
    if !canon_existing.starts_with(&canon_root) {
        return Err(ToolOutput {
            content: format!(
                "PATH_SCOPE_VIOLATION: '{}' resolves outside the project root.",
                rel_path
            ),
            is_error: true,
        });
    }
    Ok(joined)
}

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

    // ── .rustic/ directory is always allowed ───────────────────────────────
    // The .rustic folder is project configuration, not sensitive data.
    let normalized = rel_path.replace('\\', "/");
    if normalized.starts_with(".rustic/") || normalized == ".rustic" {
        return None;
    }

    // ── .gitignore is always allowed ────────────────────────────────────────
    // It is project configuration, never credentials.
    if filename_lower == ".gitignore" {
        return None;
    }

    // ── Check allowlist ──────────────────────────────────────────────────────
    // Paths in .rustic/allowed-files.txt skip tier-2/3
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
        // Bypass the prompt when either the explicit "sensitive files allowed"
        // toggle is on, OR the task is running in FullAuto. The FullAuto enum
        // doc reads "no approval prompts" — a tier-2 prompt here violated that
        // contract and was the reason FullAuto sub-agents (which inherit
        // FullAuto from the parent) still stalled on `.env` reads.
        if context.sensitive_files_allowed()
            || context.permissions() == PermissionLevel::FullAuto
        {
            return None;
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
    if context.sensitive_files_allowed()
        || context.permissions() == PermissionLevel::FullAuto
    {
        return None;
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

/// Enforce the sub-agent's declared write scope. Returns `Some(ToolOutput)` when
/// the path is outside scope and the write must be rejected. `None` means the
/// write is in scope (or the agent is unrestricted, i.e. the main agent).
///
/// Runs before the sensitive-file / broker checks because a scope violation is
/// a harder failure: it means the orchestrator did not authorize this sub-agent
/// to touch that file at all, regardless of whether the file is sensitive.
fn check_write_scope(context: &ToolContext, rel_path: &str) -> Option<ToolOutput> {
    let scope = match &context.write_scope {
        None => return None, // main agent: unrestricted
        Some(s) => s,
    };

    let normalized = rel_path.replace('\\', "/");
    let in_scope = scope
        .iter()
        .any(|allowed| crate::task::subagent::paths_overlap(allowed, &normalized));

    if in_scope {
        return None;
    }

    let scope_display = if scope.is_empty() {
        "[] (read-only)".to_string()
    } else {
        format!("[{}]", scope.join(", "))
    };

    Some(ToolOutput {
        content: format!(
            "WRITE_SCOPE_VIOLATION: This sub-agent's declared writes are {}.\n\
             The path '{}' is outside that scope. Either:\n  \
             1. If you can finish without writing this file, skip it and call \
             `report_blocked_write` with the path and reason so the orchestrator \
             can handle it afterward, then end your turn with a plain-text summary \
             of what you did finish.\n  \
             2. If this write is critical and you cannot continue without it, you \
             must still stop — call `report_blocked_write` with a clear reason and \
             end your turn with a plain-text summary. The orchestrator will \
             re-dispatch with expanded scope.\n\
             Do not retry this write.",
            scope_display, rel_path
        ),
        is_error: true,
    })
}

// Hard line limit for reads with no explicit start_line/end_line.
// Protects context window from accidentally large files.
const DEFAULT_READ_LIMIT: usize = 500;

// Context bounds for STALE_READ error responses (kept for the legacy
// stale-read path; the new EDIT_NO_MATCH path uses top-N candidate lines
// instead and is tighter).
const CONTEXT_LINES: usize = 150;
const MAX_CONTEXT_LINES: usize = 300;
const MAX_CONTEXT_BYTES: usize = 8 * 1024;

// EDIT_NO_MATCH error context: top-N candidate lines + small surrounding window.
// Picked to keep error bodies focused (no more ±150-line dumps) — see plan P0.5
// and R.2 F1 for why the old format misled the agent.
const NO_MATCH_TOP_N: usize = 3;
const NO_MATCH_CTX_LINES: usize = 2; // lines above + below each candidate

pub fn definitions() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "read_file".into(),
            description: "Read a file's contents. Every read is billed against the context \
                          window — be intentional.\n\
                          • If you already know WHICH lines you need (from a prior grep_search \
                            hit, an edit_file EDIT_NO_MATCH, or a compiler error), pass \
                            start_line/end_line (1-indexed, inclusive) and read only that range.\n\
                          • If you need to survey a file you've never opened, omit the range — \
                            output is capped at 500 lines and you'll get a TRUNCATED notice with \
                            the total line count so you can follow up with a targeted range.\n\
                          • Do NOT re-read a file you've already read in this task unless it \
                            was modified since. If you try anyway, the tool will return a \
                            FILE_UNCHANGED stub instead of the bytes — refer to the earlier \
                            read_file tool_result in the conversation for the content. To \
                            force a fresh read after the file was modified, no action is needed: \
                            the stub only triggers when the mtime matches the earlier read.\n\
                          • To LOCATE files, use `glob` (by filename pattern) or `grep_search` \
                            (by content). Never read many files just to find one — that burns \
                            tokens fast.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Relative path from project root" },
                    "start_line": {
                        "type": "integer",
                        "description": "First line to read (1-indexed). Omit to read from the beginning (capped at 500 lines)."
                    },
                    "end_line": {
                        "type": "integer",
                        "description": "Last line to read (1-indexed, inclusive). Omit to read to the end of the file (or the 500-line cap)."
                    }
                },
                "required": ["path"]
            }),
        },
        ToolDef {
            name: "create_file".into(),
            description: "Create a new file with the given content, or create an empty directory. \
                          Parent directories are created automatically. If the file already exists, \
                          use edit_file or apply_patch to modify it instead.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative path from project root for the file or directory to create"
                    },
                    "content": {
                        "type": "string",
                        "description": "The file content to write. Omit or leave empty to create an empty file. \
                                        Set is_directory to true to create a directory instead."
                    },
                    "is_directory": {
                        "type": "boolean",
                        "description": "If true, create an empty directory instead of a file. Default: false."
                    }
                },
                "required": ["path"]
            }),
        },
        ToolDef {
            name: "edit_file".into(),
            description: "Edit a file by replacing the first occurrence of old_string with \
                          new_string. Matching is byte-exact first; if that fails, a \
                          whitespace-tolerant fallback (strip per-line trailing whitespace, \
                          normalize CRLF/LF) is attempted. \
                          To DELETE content, pass new_string as an empty string \"\". \
                          To REPLACE a large section, match the entire block as old_string and \
                          provide the new content as new_string. \
                          Returns EDIT_NO_MATCH with top candidate lines if old_string cannot \
                          be located (this is a string-matching failure, not a file-changed \
                          error — fix your old_string rather than re-reading). \
                          Returns ALREADY_APPLIED if the replacement is already in place.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Relative path from project root" },
                    "old_string": {
                        "type": "string",
                        "description": "The text to replace. Byte-exact match is preferred; \
                                        whitespace-only differences will fall back gracefully \
                                        but still emit a warning so you can tighten the match."
                    },
                    "new_string": {
                        "type": "string",
                        "description": "The replacement text"
                    },
                    "hint_line": {
                        "type": "integer",
                        "description": "Approximate line number of old_string (1-indexed). \
                                       Improves EDIT_NO_MATCH candidate ranking when the match fails."
                    }
                },
                "required": ["path", "old_string", "new_string"]
            }),
        },
        ToolDef {
            name: "apply_patch".into(),
            description: "Apply multiple find-and-replace hunks to a file atomically. \
                          All hunks must succeed or none are applied (rollback on failure). \
                          Each hunk uses byte-exact matching with a whitespace-tolerant fallback \
                          (same rules as edit_file). EDIT_NO_MATCH on any hunk rolls everything back.".into(),
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
    // Global orchestrator is read-only over the filesystem — it can inspect
    // code and run commands (for surveying) but cannot modify anything. To
    // change files it must `spawn_subtask` into a specific project.
    if context.is_global && matches!(name, "create_file" | "edit_file" | "apply_patch") {
        return Ok(ToolOutput {
            content: format!(
                "PERMISSION_DENIED: `{}` is blocked in the Global scope. \
                 Global is read-only — use `spawn_subtask` to delegate file \
                 changes to a specific project.",
                name
            ),
            is_error: true,
        });
    }

    match name {
        "read_file" => execute_read_file(params, context).await,
        "create_file" => execute_create_file(params, context).await,
        "edit_file" => execute_edit_file(params, context).await,
        "apply_patch" => execute_apply_patch(params, context).await,
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
    // Use scope-aware resolution so the Global orchestrator can read across
    // every registered workspace project, not just the empty global_scope dir.
    let full_path = match resolve_with_scope(context, path) {
        Ok(p) => p,
        Err(violation) => return Ok(violation),
    };

    if let Some(blocked) = check_sensitive_path(path, &full_path, context).await {
        return Ok(blocked);
    }

    // Acquire per-file lock — wait silently if another task holds it
    let file_lock = context.file_lock.get_lock(&full_path);
    let _guard = file_lock.lock().await;

    // ── Unchanged-file short-circuit ──────────────────────────────────────
    // If this file was already read earlier in the task AND its mtime hasn't
    // changed AND the requested range is covered by what the model has seen,
    // return a stub pointing at the prior tool_result instead of re-billing
    // the bytes. Matches Claude Code's FILE_UNCHANGED behaviour.
    //
    // Bounds used for coverage check — we normalize the range to compare against
    // what the model was actually shown:
    //   - No range given → covers lines 1..min(total, DEFAULT_READ_LIMIT)
    //   - Range given    → covers [start..end] clamped to the file size
    let mtime_now = match std::fs::metadata(&full_path).and_then(|m| m.modified()) {
        Ok(t) => Some(t),
        Err(_) => None,
    };

    if let Some(current_mtime) = mtime_now {
        // We need the file length to normalize ranges consistently with how
        // the model saw them. A cheap stat-sized shortcut isn't correct here
        // (sizes are in bytes, not lines), so we fall through and read if the
        // pre-check path doesn't hit.
        if let Ok(metadata) = std::fs::metadata(&full_path) {
            if metadata.is_file() {
                // Compute normalized bounds WITHOUT reading the file. We use
                // a conservative lower bound: end = end_line OR DEFAULT_READ_LIMIT
                // when no range was requested. This is slightly pessimistic
                // (we might miss a stub opportunity when the file is tiny)
                // but never falsely stubs content the model hasn't seen.
                let norm_start = start_line.unwrap_or(1).max(1);
                let norm_end = end_line.unwrap_or(DEFAULT_READ_LIMIT).max(norm_start);
                if context
                    .file_read_registry
                    .already_covered(&full_path, norm_start, norm_end, current_mtime)
                {
                    return Ok(ToolOutput {
                        content: format!(
                            "FILE_UNCHANGED: '{}' was already read earlier in this \
                             conversation and has not been modified since (same mtime). \
                             The requested range (lines {}-{}) is already covered by an \
                             earlier read_file tool_result in this thread — refer to that \
                             result instead of re-reading. If you need a different range \
                             of the same file, pass start_line/end_line that falls outside \
                             what you've already seen.",
                            path, norm_start, norm_end
                        ),
                        is_error: false,
                    });
                }
            }
        }
    }

    match std::fs::read_to_string(&full_path) {
        Ok(content) => {
            let lines: Vec<&str> = content.lines().collect();
            let total = lines.len();

            // Compute the actual (1-indexed, inclusive) range the model is about
            // to see so we can record it for future stub checks.
            let (recorded_start, recorded_end, output) = if start_line.is_none() && end_line.is_none() {
                let end = total.min(DEFAULT_READ_LIMIT);
                let body = lines[..end].join("\n");
                let text = if total > DEFAULT_READ_LIMIT {
                    format!(
                        "{}\n\n[TRUNCATED: showing lines 1-{} of {} total. \
                         Pass start_line/end_line to read beyond line {}.]",
                        body, end, total, end
                    )
                } else {
                    body
                };
                (1usize, end.max(1), text)
            } else {
                let start = start_line.map(|n| n.saturating_sub(1)).unwrap_or(0).min(total);
                let end = end_line.map(|n| n.min(total)).unwrap_or(total);
                let end = end.max(start);
                let selected: Vec<String> = lines[start..end]
                    .iter()
                    .enumerate()
                    .map(|(i, line)| format!("{}: {}", start + i + 1, *line))
                    .collect();
                (start + 1, end.max(start + 1), selected.join("\n"))
            };

            if let Some(mtime) = mtime_now {
                context
                    .file_read_registry
                    .record(full_path.clone(), mtime, recorded_start, recorded_end);
            }

            Ok(ToolOutput { content: output, is_error: false })
        }
        Err(e) => Ok(ToolOutput { content: format!("Error reading file: {}", e), is_error: true }),
    }
}

// ─── create_file ─────────────────────────────────────────────────────────────

async fn execute_create_file(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    let path = params["path"].as_str().unwrap_or("");
    if path.is_empty() {
        return Ok(ToolOutput { content: "path is required".into(), is_error: true });
    }

    if context.permissions() == PermissionLevel::Chat {
        return Ok(ToolOutput {
            content: "PERMISSION_DENIED: File writes are not allowed in Chat mode.".into(),
            is_error: true,
        });
    }

    if let Some(scope_violation) = check_write_scope(context, path) {
        return Ok(scope_violation);
    }

    let full_path = match resolve_within_project(&context.project_root, path) {
        Ok(p) => p,
        Err(violation) => return Ok(violation),
    };
    let is_directory = params["is_directory"].as_bool().unwrap_or(false);

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

    if is_directory {
        match std::fs::create_dir_all(&full_path) {
            Ok(()) => Ok(ToolOutput { content: format!("Created directory {}", path), is_error: false }),
            Err(e) => Ok(ToolOutput { content: format!("Error creating directory: {}", e), is_error: true }),
        }
    } else {
        // Acquire per-file lock — wait silently if another task holds it
        let file_lock = context.file_lock.get_lock(&full_path);
        let _guard = file_lock.lock().await;

        if full_path.exists() {
            return Ok(ToolOutput {
                content: format!(
                    "FILE_EXISTS: '{}' already exists. Use edit_file or apply_patch to modify it.",
                    path
                ),
                is_error: true,
            });
        }
        // Auto-create parent directories
        if let Some(parent) = full_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        // Capture pre-mutation state into the snapshot for this user message.
        // For create_file the file does not yet exist, so capture records a
        // null backup that revert will later use to delete the new file.
        track_before_write(context, &full_path);
        let content = params["content"].as_str().unwrap_or("");
        match crate::io_util::atomic_write(&full_path, content.as_bytes()) {
            Ok(()) => {
                maybe_emit_memory_updated(path, context);
                Ok(ToolOutput { content: format!("Created {}", path), is_error: false })
            }
            Err(e) => Ok(ToolOutput { content: format!("Error creating file: {}", e), is_error: true }),
        }
    }
}

// ─── edit_file ────────────────────────────────────────────────────────────────

async fn execute_edit_file(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    let path = params["path"].as_str().unwrap_or("");
    if context.permissions() == PermissionLevel::Chat {
        return Ok(ToolOutput {
            content: "PERMISSION_DENIED: File writes are not allowed in Chat mode.".into(),
            is_error: true,
        });
    }
    if let Some(scope_violation) = check_write_scope(context, path) {
        return Ok(scope_violation);
    }
    let full_path_for_check = match resolve_within_project(&context.project_root, path) {
        Ok(p) => p,
        Err(violation) => return Ok(violation),
    };
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

    let full_path = match resolve_within_project(&context.project_root, path) {
        Ok(p) => p,
        Err(violation) => return Ok(violation),
    };

    // Acquire per-file lock before any I/O — wait silently for contention
    let file_lock = context.file_lock.get_lock(&full_path);
    let _guard = file_lock.lock().await;

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

    // Locate old_string in the file (exact match, with whitespace-tolerant fallback).
    let matched = match find_edit_match(&content, &old_string) {
        Some(m) => m,
        None => {
            // Idempotency check: if new_string is already in the file, the edit was already applied.
            if !new_string.is_empty() && content.contains(new_string.as_str()) {
                return Ok(ToolOutput {
                    content: format!(
                        "ALREADY_APPLIED: The replacement text is already present in '{}'. No changes made.",
                        path
                    ),
                    is_error: false,
                });
            }
            // EDIT_NO_MATCH: byte-mismatch on old_string. Surface the top-N
            // candidate lines so the agent can correct its match string
            // rather than misreading this as an external file change.
            let ctx = build_no_match_context(&content, &old_string, hint_line);
            return Ok(ToolOutput {
                content: format!(
                    "EDIT_NO_MATCH: old_string did not byte-match any text in '{}'. \
                     This is a string-matching failure, not a file-changed error — \
                     check your old_string for whitespace, indentation, or character \
                     differences from what's actually in the file.\n\n{}",
                    path, ctx
                ),
                is_error: true,
            });
        }
    };

    // Splice in new_string at the matched byte range (preserves original
    // formatting around the match even when whitespace fallback hit).
    let mut new_content = String::with_capacity(content.len() + new_string.len());
    new_content.push_str(&content[..matched.range.start]);
    new_content.push_str(&new_string);
    new_content.push_str(&content[matched.range.end..]);

    // Capture pre-edit content into the current user message's snapshot.
    // Idempotent within a snapshot — repeated edits to the same file in one
    // turn keep v1 (the original pre-turn state) as the revert target.
    track_before_write(context, &full_path);

    match crate::io_util::atomic_write(&full_path, new_content.as_bytes()) {
        Ok(()) => {
            maybe_emit_memory_updated(path, context);
            let msg = match matched.fallback {
                MatchFallback::Exact => format!("Edited {}", path),
                MatchFallback::Whitespace => format!(
                    "Edited {} (WHITESPACE_NORMALIZED: matched after stripping per-line \
                     trailing whitespace / normalizing line endings — your old_string had \
                     cosmetic whitespace differences from the file)",
                    path
                ),
            };
            Ok(ToolOutput { content: msg, is_error: false })
        }
        Err(e) => Ok(ToolOutput { content: format!("Error writing file: {}", e), is_error: true }),
    }
}

// ─── apply_patch ──────────────────────────────────────────────────────────────

async fn execute_apply_patch(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    let path = params["path"].as_str().unwrap_or("");
    if context.permissions() == PermissionLevel::Chat {
        return Ok(ToolOutput {
            content: "PERMISSION_DENIED: File writes are not allowed in Chat mode.".into(),
            is_error: true,
        });
    }
    if let Some(scope_violation) = check_write_scope(context, path) {
        return Ok(scope_violation);
    }
    let full_path_for_check = match resolve_within_project(&context.project_root, path) {
        Ok(p) => p,
        Err(violation) => return Ok(violation),
    };
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

    let full_path = match resolve_within_project(&context.project_root, path) {
        Ok(p) => p,
        Err(violation) => return Ok(violation),
    };

    // Acquire per-file lock before any I/O — wait silently for contention
    let file_lock = context.file_lock.get_lock(&full_path);
    let _guard = file_lock.lock().await;

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
    let mut whitespace_fallbacks: Vec<usize> = Vec::new();
    for (i, hunk) in hunks.iter().enumerate() {
        let old = match hunk["old_string"].as_str() {
            Some(s) => s,
            None => return Ok(ToolOutput {
                content: format!("Hunk {} is missing old_string", i),
                is_error: true,
            }),
        };
        let new = hunk["new_string"].as_str().unwrap_or("");

        match find_edit_match(&current, old) {
            Some(m) => {
                if m.fallback == MatchFallback::Whitespace {
                    whitespace_fallbacks.push(i);
                }
                let mut spliced = String::with_capacity(current.len() + new.len());
                spliced.push_str(&current[..m.range.start]);
                spliced.push_str(new);
                spliced.push_str(&current[m.range.end..]);
                current = spliced;
            }
            None => {
                if !new.is_empty() && current.contains(new) {
                    return Ok(ToolOutput {
                        content: format!(
                            "ALREADY_APPLIED: Hunk {} replacement is already present in '{}'. \
                             No changes applied.",
                            i, path
                        ),
                        is_error: false,
                    });
                }
                let ctx = build_no_match_context(&current, old, None);
                return Ok(ToolOutput {
                    content: format!(
                        "EDIT_NO_MATCH: Hunk {} old_string did not byte-match any text in '{}'. \
                         No changes applied (all hunks rolled back). This is a string-matching \
                         failure — check whitespace, indentation, or characters in your old_string.\n\n{}",
                        i, path, ctx
                    ),
                    is_error: true,
                });
            }
        }
    }

    // All hunks applied in memory; now capture pre-patch state into the
    // snapshot for this user message before flushing to disk.
    track_before_write(context, &full_path);

    match crate::io_util::atomic_write(&full_path, current.as_bytes()) {
        Ok(()) => {
            maybe_emit_memory_updated(path, context);
            let mut msg = format!("Patched {} ({} hunk(s) applied)", path, hunks.len());
            if !whitespace_fallbacks.is_empty() {
                msg.push_str(&format!(
                    " (WHITESPACE_NORMALIZED on hunk(s) {:?}: matched after stripping \
                     per-line trailing whitespace / normalizing line endings)",
                    whitespace_fallbacks
                ));
            }
            Ok(ToolOutput { content: msg, is_error: false })
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
        match resolve_within_project(&context.project_root, path) {
            Ok(p) => p,
            Err(violation) => return Ok(violation),
        }
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
        let _ = ctx.event_tx.try_send(TaskEvent::MemoryUpdated { task_id: ctx.task_id.clone() });
    }
}

/// Capture the pre-mutation state of `abs_path` into the current user message's
/// snapshot, then emit a `FileTracked` event so the UI's changed-files panel
/// can render the path immediately.
///
/// Failure is non-fatal — a tracker error must not block the actual edit
/// because the user's intent is the file change, not the bookkeeping. Errors
/// are logged via `tracing::warn`.
fn track_before_write(ctx: &ToolContext, abs_path: &std::path::Path) {
    let (Some(history), Some(message_id)) = (
        ctx.file_history.as_ref(),
        ctx.current_user_message_id.as_ref(),
    ) else {
        return;
    };
    match history.capture(message_id, abs_path) {
        Ok(outcome) => {
            use crate::file_history::CaptureOutcome;
            let rel = match outcome {
                CaptureOutcome::Captured { rel_path, .. } => rel_path,
                CaptureOutcome::DidNotExist { rel_path } => rel_path,
                CaptureOutcome::AlreadyTracked { .. } => return, // no event for repeats
                CaptureOutcome::TooLarge { rel_path, size } => {
                    tracing::info!(path = %rel_path, size, "file too large to track");
                    rel_path
                }
            };
            let _ = ctx.event_tx.try_send(TaskEvent::FileTracked {
                task_id: ctx.task_id.clone(),
                message_id: message_id.clone(),
                kind: crate::FileTrackedKind::EditTool,
                paths: vec![rel],
            });
        }
        Err(e) => {
            tracing::warn!(?e, path = %abs_path.display(), "tracker capture failed");
        }
    }
}

/// Outcome of `find_edit_match`. Carries the byte range in the *original*
/// content that should be replaced, plus which fallback matched (or `Exact`).
struct EditMatch {
    range: std::ops::Range<usize>,
    fallback: MatchFallback,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum MatchFallback {
    Exact,
    Whitespace,
}

/// Try to locate `old_string` inside `content`, falling back to
/// whitespace-tolerant matching if the exact match fails. The returned range
/// is always in the original `content`'s byte coordinates, so callers can
/// splice in `new_string` while preserving the file's actual formatting.
///
/// Whitespace fallback rules (only ASCII spaces/tabs/CR are touched; UTF-8
/// multibyte sequences are untouched):
///   - CRLF and CR line endings are normalized to LF.
///   - Trailing whitespace (space, tab) is stripped from each line.
/// Both sides are normalized, then `find` runs on the normalized strings.
/// A byte-offset map carries us back to original coordinates.
fn find_edit_match(content: &str, old_string: &str) -> Option<EditMatch> {
    if old_string.is_empty() {
        return None;
    }
    if let Some(idx) = content.find(old_string) {
        return Some(EditMatch {
            range: idx..idx + old_string.len(),
            fallback: MatchFallback::Exact,
        });
    }
    let (norm_content, content_offsets) = normalize_ws_with_offsets(content);
    let (norm_old, _) = normalize_ws_with_offsets(old_string);
    if norm_old.is_empty() {
        return None;
    }
    let idx = norm_content.find(&norm_old)?;
    let end = idx + norm_old.len();
    // content_offsets has len = norm_content.len() + 1, with the trailing
    // entry pointing one past the last consumed original byte. Both indices
    // are guaranteed in range by construction.
    let orig_start = *content_offsets.get(idx)?;
    let orig_end = *content_offsets.get(end)?;
    if orig_end < orig_start {
        return None;
    }
    Some(EditMatch {
        range: orig_start..orig_end,
        fallback: MatchFallback::Whitespace,
    })
}

/// Build a whitespace-normalized copy of `s` plus a byte-offset map.
/// `offsets[i]` is the byte index in `s` that produced byte `i` of the
/// normalized output, with one extra sentinel entry pointing one past the
/// end so callers can read both range endpoints from the map. Operates only
/// on ASCII whitespace bytes — multibyte UTF-8 sequences pass through
/// untouched, so the output is always valid UTF-8.
fn normalize_ws_with_offsets(s: &str) -> (String, Vec<usize>) {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut offsets: Vec<usize> = Vec::with_capacity(bytes.len() + 1);

    let mut i = 0;
    while i < bytes.len() {
        // Find end of current line (up to next '\n', or end of input).
        let mut line_end = i;
        while line_end < bytes.len() && bytes[line_end] != b'\n' {
            line_end += 1;
        }
        // Strip a trailing '\r' (CRLF → LF) and any trailing spaces/tabs.
        let mut trimmed_end = line_end;
        if trimmed_end > i && bytes[trimmed_end - 1] == b'\r' {
            trimmed_end -= 1;
        }
        while trimmed_end > i
            && (bytes[trimmed_end - 1] == b' ' || bytes[trimmed_end - 1] == b'\t')
        {
            trimmed_end -= 1;
        }
        // Emit the trimmed line content.
        for k in i..trimmed_end {
            offsets.push(k);
            out.push(bytes[k]);
        }
        // Emit the line terminator (LF) if present in the source. The offset
        // points at the '\n' byte itself; for a CRLF source line the '\r'
        // bytes have already been dropped above.
        if line_end < bytes.len() {
            offsets.push(line_end);
            out.push(b'\n');
            i = line_end + 1;
        } else {
            i = line_end;
        }
    }
    offsets.push(bytes.len());
    // Safety: we only ever dropped ASCII bytes (space, tab, CR) outside any
    // multibyte sequence, so the result is still valid UTF-8.
    let out_str = String::from_utf8(out).expect("ws normalization preserves utf-8");
    (out_str, offsets)
}

/// Token-set Jaccard similarity between two lines after collapsing whitespace
/// and lower-casing. Cheap, deterministic, and matches the failure mode we
/// actually see (indentation differences, trailing whitespace) better than
/// raw Levenshtein.
fn line_similarity(a: &str, b: &str) -> f32 {
    let toks_a: std::collections::HashSet<String> = a
        .split_whitespace()
        .map(|t| t.to_ascii_lowercase())
        .collect();
    let toks_b: std::collections::HashSet<String> = b
        .split_whitespace()
        .map(|t| t.to_ascii_lowercase())
        .collect();
    if toks_a.is_empty() && toks_b.is_empty() {
        return 1.0;
    }
    let inter = toks_a.intersection(&toks_b).count();
    let union = toks_a.union(&toks_b).count();
    if union == 0 {
        0.0
    } else {
        inter as f32 / union as f32
    }
}

/// Build the EDIT_NO_MATCH context block: top N candidate lines (by token
/// similarity against the first non-empty line of `old_string`), each shown
/// with ±NO_MATCH_CTX_LINES of surrounding context. Falls back to a brief
/// head-of-file slice if no meaningful comparison can be made.
fn build_no_match_context(content: &str, old_string: &str, hint_line: Option<usize>) -> String {
    let file_lines: Vec<&str> = content.lines().collect();
    let total = file_lines.len();
    if total == 0 {
        return "(file is empty)\n".to_string();
    }

    // Pick the first non-empty / non-whitespace-only line of old_string as
    // the probe. If old_string is entirely blank lines, fall back to a hint-
    // line window if we have one, otherwise the first MAX_CONTEXT_LINES.
    let probe = old_string
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("");
    if probe.trim().is_empty() {
        return build_stale_read_context(content, hint_line);
    }

    // Score every line, keep top N. Hint line gets a small similarity boost
    // so that ties near the hint win.
    let mut scored: Vec<(f32, usize)> = file_lines
        .iter()
        .enumerate()
        .map(|(idx, line)| {
            let mut score = line_similarity(probe, line);
            if let Some(hl) = hint_line {
                let line_no = idx + 1;
                let dist = (line_no as isize - hl as isize).unsigned_abs() as usize;
                if dist <= 5 {
                    score += 0.05 * (6.0 - dist as f32) / 6.0;
                }
            }
            (score, idx)
        })
        .filter(|(score, _)| *score > 0.0)
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    if scored.is_empty() {
        return build_stale_read_context(content, hint_line);
    }

    // Take top N, then dedupe overlapping context windows (if two candidates
    // are within NO_MATCH_CTX_LINES of each other, merge them).
    let mut picks: Vec<(f32, usize)> = Vec::new();
    for (score, idx) in scored.into_iter().take(NO_MATCH_TOP_N * 3) {
        if picks.len() >= NO_MATCH_TOP_N {
            break;
        }
        let too_close = picks
            .iter()
            .any(|(_, p)| idx.abs_diff(*p) <= NO_MATCH_CTX_LINES);
        if too_close {
            continue;
        }
        picks.push((score, idx));
    }
    picks.sort_by_key(|(_, idx)| *idx);

    let mut out = String::new();
    out.push_str(&format!(
        "Top {} candidate location(s) (by token similarity to your old_string's first line):\n\n",
        picks.len()
    ));
    let mut byte_count = out.len();
    for (score, idx) in picks {
        let line_no = idx + 1;
        let start = idx.saturating_sub(NO_MATCH_CTX_LINES);
        let end = (idx + NO_MATCH_CTX_LINES + 1).min(total);
        let header = format!("— line {} (similarity {:.2}) —\n", line_no, score);
        if byte_count + header.len() > MAX_CONTEXT_BYTES {
            out.push_str(&format!(
                "[... truncated at {}KB]\n",
                MAX_CONTEXT_BYTES / 1024
            ));
            break;
        }
        out.push_str(&header);
        byte_count += header.len();
        for (j, line) in file_lines[start..end].iter().enumerate() {
            let n = start + j + 1;
            let marker = if n == line_no { ">" } else { " " };
            let formatted = format!("{} {}: {}\n", marker, n, line);
            if byte_count + formatted.len() > MAX_CONTEXT_BYTES {
                out.push_str(&format!(
                    "[... truncated at {}KB]\n",
                    MAX_CONTEXT_BYTES / 1024
                ));
                return out;
            }
            out.push_str(&formatted);
            byte_count += formatted.len();
        }
        out.push('\n');
        byte_count += 1;
    }
    out
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

#[cfg(test)]
mod p0_5_match_tests {
    use super::*;

    #[test]
    fn exact_match_is_exact() {
        let content = "hello\nworld\n";
        let m = find_edit_match(content, "world").expect("should match");
        assert_eq!(m.fallback, MatchFallback::Exact);
        assert_eq!(&content[m.range], "world");
    }

    #[test]
    fn trailing_whitespace_difference_falls_back() {
        // File has trailing spaces on line 1; agent's old_string does not.
        let content = "/// doc comment   \nfn foo() {}\n";
        let old = "/// doc comment\nfn foo() {}\n";
        let m = find_edit_match(content, old).expect("ws fallback should match");
        assert_eq!(m.fallback, MatchFallback::Whitespace);
        // Replacement range is in original coordinates and spans the
        // trailing whitespace on line 1.
        assert_eq!(&content[m.range], content);
    }

    #[test]
    fn crlf_vs_lf_falls_back() {
        let content = "line one\r\nline two\r\n";
        let old = "line one\nline two\n";
        let m = find_edit_match(content, old).expect("crlf fallback should match");
        assert_eq!(m.fallback, MatchFallback::Whitespace);
        // Range covers the whole CRLF content
        assert_eq!(m.range.start, 0);
        assert_eq!(m.range.end, content.len());
    }

    #[test]
    fn no_match_returns_none() {
        let content = "hello\nworld\n";
        assert!(find_edit_match(content, "goodbye").is_none());
    }

    #[test]
    fn empty_old_string_returns_none() {
        assert!(find_edit_match("anything", "").is_none());
    }

    #[test]
    fn utf8_multibyte_is_preserved() {
        // "héllo" contains a multibyte é. Ensure the matcher doesn't mangle
        // byte offsets across the multibyte boundary when whitespace fallback
        // runs.
        let content = "héllo  \nwörld\n";
        let old = "héllo\nwörld\n";
        let m = find_edit_match(content, old).expect("utf-8 fallback should match");
        assert_eq!(m.fallback, MatchFallback::Whitespace);
        // Splicing must produce valid UTF-8.
        let replacement = "replaced";
        let mut out = String::new();
        out.push_str(&content[..m.range.start]);
        out.push_str(replacement);
        out.push_str(&content[m.range.end..]);
        assert_eq!(out, "replaced");
    }

    #[test]
    fn whitespace_inside_lines_is_not_collapsed() {
        // We only strip trailing whitespace, not collapse internal whitespace.
        // "fn foo()" with 2 spaces between tokens should NOT match a file with
        // 1 space (those are semantically different in code formatting).
        let content = "fn  foo() {}\n";
        let old = "fn foo() {}\n";
        assert!(find_edit_match(content, old).is_none(),
            "internal whitespace differences must NOT be normalized away");
    }
}

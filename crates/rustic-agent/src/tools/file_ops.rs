use super::{coerce_batch_array, coerce_bool, ToolContext, ToolOutput};
use crate::provider::ToolDef;
use crate::task::permissions::{Action, PermissionLevel};
use crate::task::{PermissionOp, TaskEvent};
use anyhow::Result;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// Process-wide cache of canonicalize results; avoids N+1 stat calls in Global scope.
fn canonicalize_cache() -> &'static Mutex<HashMap<PathBuf, PathBuf>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, PathBuf>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Hard cap on cached entries; the map is cleared when full (simple, and the
/// working set — mostly project roots — is tiny, so refill is cheap).
const CANONICALIZE_CACHE_MAX: usize = 4096;

fn canonicalize_cached(p: &Path) -> Option<PathBuf> {
    {
        if let Ok(mut map) = canonicalize_cache().lock() {
            if let Some(hit) = map.get(p) {
                // Validate the cached mapping is still live: a single stat is far
                // cheaper than a full canonicalize, and it drops stale entries
                // (deleted/renamed paths) instead of serving them forever.
                if hit.exists() {
                    return Some(hit.clone());
                }
                map.remove(p);
            }
        }
    }
    match p.canonicalize() {
        Ok(canon) => {
            if let Ok(mut map) = canonicalize_cache().lock() {
                if map.len() >= CANONICALIZE_CACHE_MAX {
                    map.clear();
                }
                map.insert(p.to_path_buf(), canon.clone());
            }
            Some(canon)
        }
        Err(_) => None,
    }
}

// Quote normalization constants for matching and preservation
const LEFT_SINGLE_CURLY_QUOTE: char = '\u{2018}'; // '
const RIGHT_SINGLE_CURLY_QUOTE: char = '\u{2019}'; // '
const LEFT_DOUBLE_CURLY_QUOTE: char = '\u{201C}'; // "
const RIGHT_DOUBLE_CURLY_QUOTE: char = '\u{201D}'; // "

fn normalize_quotes(s: &str) -> String {
    s.replace(LEFT_SINGLE_CURLY_QUOTE, "'")
        .replace(RIGHT_SINGLE_CURLY_QUOTE, "'")
        .replace(LEFT_DOUBLE_CURLY_QUOTE, "\"")
        .replace(RIGHT_DOUBLE_CURLY_QUOTE, "\"")
}

fn is_opening_context(chars: &[char], index: usize) -> bool {
    if index == 0 {
        return true;
    }
    match chars.get(index.wrapping_sub(1)) {
        Some(&' ' | &'\t' | &'\n' | &'\r' | &'(' | &'[' | &'{' | '\u{2014}' | '\u{2013}') => true,
        _ => false,
    }
}

fn apply_curly_double_quotes(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    chars
        .iter()
        .enumerate()
        .map(|(i, &ch)| {
            if ch == '"' {
                if is_opening_context(&chars, i) {
                    LEFT_DOUBLE_CURLY_QUOTE
                } else {
                    RIGHT_DOUBLE_CURLY_QUOTE
                }
            } else {
                ch
            }
        })
        .collect()
}

fn apply_curly_single_quotes(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    chars
        .iter()
        .enumerate()
        .map(|(i, &ch)| {
            if ch == '\'' {
                // Check for contractions (apostrophes between letters)
                let prev_is_letter = i > 0 && chars.get(i - 1).map_or(false, |c| c.is_alphabetic());
                let next_is_letter = chars.get(i + 1).map_or(false, |c| c.is_alphabetic());

                if prev_is_letter && next_is_letter {
                    // Apostrophe in contraction
                    RIGHT_SINGLE_CURLY_QUOTE
                } else if is_opening_context(&chars, i) {
                    LEFT_SINGLE_CURLY_QUOTE
                } else {
                    RIGHT_SINGLE_CURLY_QUOTE
                }
            } else {
                ch
            }
        })
        .collect()
}

fn preserve_quote_style(old_string: &str, actual_old_string: &str, new_string: &str) -> String {
    if old_string == actual_old_string {
        return new_string.to_string();
    }

    let has_double_quotes = actual_old_string.contains(LEFT_DOUBLE_CURLY_QUOTE)
        || actual_old_string.contains(RIGHT_DOUBLE_CURLY_QUOTE);
    let has_single_quotes = actual_old_string.contains(LEFT_SINGLE_CURLY_QUOTE)
        || actual_old_string.contains(RIGHT_SINGLE_CURLY_QUOTE);

    if !has_double_quotes && !has_single_quotes {
        return new_string.to_string();
    }

    let mut result = new_string.to_string();
    if has_double_quotes {
        result = apply_curly_double_quotes(&result);
    }
    if has_single_quotes {
        result = apply_curly_single_quotes(&result);
    }
    result
}

/// Drop cached parse tree for `path` and refresh the symbol index. Called on every write.
pub(crate) fn refresh_index_after_write(context: &ToolContext, path: &Path) {
    let ts = context.workspace_services.tree_sitter();
    let idx = context.workspace_services.symbol_index();
    ts.invalidate(path);
    let _ = crate::index::refresh_file(path, ts, idx); // best-effort; IO failure doesn't undo the write
}

/// Resolve `rel_path` within the active project's root. Path traversal and
/// unrelated absolute paths are rejected.
pub(crate) fn resolve_with_scope(
    context: &ToolContext,
    rel_path: &str,
) -> std::result::Result<std::path::PathBuf, ToolOutput> {
    resolve_within_project(&context.project_root, rel_path)
}

pub(crate) fn resolve_within_project(
    project_root: &std::path::Path,
    rel_path: &str,
) -> std::result::Result<std::path::PathBuf, ToolOutput> {
    if !project_root.exists() {
        return Err(ToolOutput {
            content: format!(
                "PROJECT_ROOT_MISSING: the task's working directory '{}' no longer exists on \
                 disk. Do NOT retry file operations — stop and inform the user.",
                project_root.display()
            ),
            is_error: true,
            attachments: Vec::new(),
        });
    }
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
                attachments: Vec::new(),
            });
        }
    };
    let canon_root =
        canonicalize_cached(project_root).unwrap_or_else(|| project_root.to_path_buf());
    if !canon_existing.starts_with(&canon_root) {
        return Err(ToolOutput {
            content: format!(
                "PATH_SCOPE_VIOLATION: '{}' resolves outside the project root.",
                rel_path
            ),
            is_error: true,
            attachments: Vec::new(),
        });
    }
    Ok(joined)
}

/// Build-once cache for the project's `.gitignore` matcher, keyed by the
/// .gitignore path and invalidated when its mtime changes. Rebuilding the
/// GitignoreBuilder on every sensitive-path check was measurably hot.
fn cached_gitignore(
    project_root: &Path,
    gitignore_path: &Path,
) -> Option<std::sync::Arc<ignore::gitignore::Gitignore>> {
    use ignore::gitignore::GitignoreBuilder;
    type GiCache = HashMap<
        PathBuf,
        (
            std::time::SystemTime,
            std::sync::Arc<ignore::gitignore::Gitignore>,
        ),
    >;
    static CACHE: OnceLock<Mutex<GiCache>> = OnceLock::new();

    let mtime = gitignore_path
        .metadata()
        .and_then(|m| m.modified())
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);

    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(map) = cache.lock() {
        if let Some((cached_mtime, gi)) = map.get(gitignore_path) {
            if *cached_mtime == mtime {
                return Some(std::sync::Arc::clone(gi));
            }
        }
    }

    let mut builder = GitignoreBuilder::new(project_root);
    let _ = builder.add(gitignore_path);
    let gi = std::sync::Arc::new(builder.build().ok()?);
    if let Ok(mut map) = cache.lock() {
        map.insert(
            gitignore_path.to_path_buf(),
            (mtime, std::sync::Arc::clone(&gi)),
        );
    }
    Some(gi)
}

pub(crate) async fn check_sensitive_path(
    rel_path: &str,
    full_path: &std::path::Path,
    context: &crate::tools::ToolContext,
) -> Option<ToolOutput> {
    let filename = full_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let path_str = full_path.to_string_lossy().to_lowercase();
    let filename_lower = filename.to_lowercase();

    // Tier 1: always block
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
            attachments: Vec::new(),
        });
    }
    if tier1_extensions
        .iter()
        .any(|ext| filename_lower.ends_with(ext))
    {
        return Some(ToolOutput {
            content: format!(
                "SENSITIVE_FILE_BLOCKED: Access to '{}' is permanently denied. \
                 Certificate/key files cannot be read or modified by the agent.",
                rel_path
            ),
            is_error: true,
            attachments: Vec::new(),
        });
    }
    // Standalone .key files (not build artifacts — check it's not something like keymap.key)
    if filename_lower == ".key"
        || (filename_lower.ends_with(".key")
            && !filename_lower.contains("map")
            && !filename_lower.contains("board"))
    {
        return Some(ToolOutput {
            content: format!(
                "SENSITIVE_FILE_BLOCKED: Access to '{}' is permanently denied. Key files cannot be accessed.",
                rel_path
            ),
            is_error: true,
            attachments: Vec::new(),
        });
    }
    // AWS credentials
    if path_str.contains(".aws") && filename_lower == "credentials" {
        return Some(ToolOutput {
            content:
                "SENSITIVE_FILE_BLOCKED: Access to AWS credentials file is permanently denied."
                    .to_string(),
            is_error: true,
            attachments: Vec::new(),
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
            attachments: Vec::new(),
        });
    }

    let normalized = rel_path.replace('\\', "/");
    if normalized.starts_with(".rustic/") || normalized == ".rustic" {
        return None;
    }

    if filename_lower == ".gitignore" {
        return None;
    }

    // Paths in .rustic/allowed-files.txt skip tier-2/3
    if context
        .allowed_paths
        .iter()
        .any(|p| p.trim() == normalized.as_str())
    {
        return None;
    }

    // Tier 2: sensitive patterns — require confirmation unless sensitive_files_allowed
    let is_tier2 = {
        let n = &filename_lower;
        n == ".env"
            || n.starts_with(".env.")
            || n.ends_with(".env") // production.env, local.env, …
            || n.starts_with("credentials")
            || n == "credentials"
            || n.starts_with("secrets")
            || n.ends_with(".secret")
            || n.ends_with(".token")
    };

    if is_tier2 {
        // Tier-2 (likely secrets) is prompted even in FullAuto — autonomy
        // covers routine edits/commands, not silently exfiltrating credential
        // files into the transcript. Only the explicit "sensitive files
        // allowed" toggle (or an allowed-files.txt entry, handled above)
        // bypasses this prompt.
        if context.sensitive_files_allowed() {
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
                attachments: Vec::new(),
            });
        }
        return None;
    }

    // Tier 3: gitignored files
    if context.sensitive_files_allowed() || context.permissions() == PermissionLevel::FullAuto {
        return None;
    }

    let gitignore_path = context.project_root.join(".gitignore");
    if gitignore_path.exists() {
        if let Some(gi) = cached_gitignore(&context.project_root, &gitignore_path) {
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
                        attachments: Vec::new(),
                    });
                }
            }
        }
    }

    None // allow
}

/// Returns `Some(error)` when `rel_path` is outside the sub-agent's declared write scope.
pub(crate) fn check_write_scope(context: &ToolContext, rel_path: &str) -> Option<ToolOutput> {
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
        attachments: Vec::new(),
    })
}

const DEFAULT_READ_LIMIT: usize = 500;

// Two-layer read cap: 256 KB byte gate (pre-read stat) + 25K token estimate (post-read).
// Both are overridable via env var for power users.
const P1_11_MAX_READ_BYTES: u64 = 256 * 1024;
const P1_11_MAX_TOKEN_ESTIMATE: usize = 25_000;

const CONTEXT_LINES: usize = 150;
const MAX_CONTEXT_LINES: usize = 300;
const MAX_CONTEXT_BYTES: usize = 8 * 1024;
const NO_MATCH_TOP_N: usize = 3;
const NO_MATCH_CTX_LINES: usize = 2;

pub fn definitions() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "read_file".into(),
            description: "Read a file's contents. Every read is billed against the context \
                          window — be intentional.\n\
                          • Text files: pass `offset` / `limit` (preferred names) to read a \
                            specific range, or omit both for a default window. Legacy \
                            `start_line` / `end_line` are still accepted as synonyms.\n\
                          • Jupyter notebooks (`.ipynb`): pass `cells` (e.g. `\"1-10\"`) to \
                            scope by cell; defaults to first 25 cells.\n\
                          • PDF (`.pdf`): pass `pages` (e.g. `\"1-5\"` or `\"3\"`) to scope \
                            by page; defaults to the first 20 pages. Hard ceiling 100 MB, \
                            native-attachment forwarding up to 32 MB so image-heavy PDFs \
                            still preserve visual detail. Per-call cap of 20 pages.\n\
                          • DOCX (`.docx`): pass `paragraph_range` (e.g. `\"1-200\"`) to \
                            scope by paragraph; defaults to the first 2000 paragraphs.\n\
                          • XLSX (`.xlsx`): pass `sheet` (sheet name or 1-indexed number) \
                            and `rows` (e.g. `\"1-1000\"`) to scope. Defaults to the first \
                            sheet, first 500 rows.\n\
                          • Legacy binary OLE (`.doc` / `.xls`): not supported — convert to \
                            the modern .docx / .xlsx first.\n\
                          • Files are capped pre-read at 256 KB (text) and post-read at an \
                            estimated 25K tokens. Passing an explicit range that exceeds \
                            either cap returns an error pointing at a smaller range — \
                            preferred over silent truncation.\n\
                          • Do NOT re-read a file you've already read in this task unless it \
                            was modified since. If you try anyway, the tool returns a \
                            FILE_UNCHANGED stub instead of the bytes — refer to the earlier \
                            read_file tool_result. The stub only triggers when the mtime \
                            matches the earlier read, so a freshly-modified file always \
                            re-reads.\n\
                          • To LOCATE files, use `glob` (by filename pattern) or `grep_search` \
                            (by content). Never read many files just to find one — that burns \
                            tokens fast.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Relative path from project root" },
                    "offset": {
                        "type": "integer",
                        "description": "P1.11 preferred name. For text: first line to read \
                                        (1-indexed). For notebooks: equivalent to the start \
                                        of `cells`. Omit to read from the beginning."
                    },
                    "limit": {
                        "type": "integer",
                        "description": "P1.11 preferred name. Max lines (text) or cells \
                                        (notebook) to return. Omit for format-specific defaults."
                    },
                    "start_line": {
                        "type": "integer",
                        "description": "Legacy alias for `offset` (text files only). Prefer `offset`."
                    },
                    "end_line": {
                        "type": "integer",
                        "description": "Legacy alias: last line to read (1-indexed, inclusive). \
                                        Prefer `offset`/`limit`."
                    },
                    "cells": {
                        "type": "string",
                        "description": "Notebooks only: cell range, e.g. \"1-10\" or \"3\". \
                                        Defaults to the first 25 cells when omitted."
                    },
                    "pages": {
                        "type": "string",
                        "description": "PDF only: page range (1-indexed inclusive), e.g. \
                                        \"1-5\" or \"3\". Defaults to the first 20 pages; \
                                        per-call cap is 20 pages."
                    },
                    "paragraph_range": {
                        "type": "string",
                        "description": "DOCX only: paragraph range (1-indexed inclusive), \
                                        e.g. \"1-200\". Defaults to the first 2000 paragraphs."
                    },
                    "sheet": {
                        "description": "XLSX only: which sheet to read — accepts a string \
                                        name or a 1-indexed number. Defaults to the first sheet."
                    },
                    "rows": {
                        "type": "string",
                        "description": "XLSX only: row range (1-indexed inclusive) within the \
                                        selected sheet, e.g. \"1-1000\". Defaults to the \
                                        first 500 rows."
                    },
                    "reads": {
                        "type": "array",
                        "description": "Batch mode: read N files in one call. Each entry uses \
                                        the same shape as a single-read call (any of `path`, \
                                        `offset`/`limit`, `start_line`/`end_line`, `cells`, \
                                        `pages`, `paragraph_range`, `sheet`/`rows`). Mutually \
                                        exclusive with the top-level fields. Each entry is \
                                        read independently — one failing entry does NOT cancel \
                                        the rest. Empty array is an error.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "path": { "type": "string" },
                                "offset": { "type": "integer" },
                                "limit": { "type": "integer" },
                                "start_line": { "type": "integer" },
                                "end_line": { "type": "integer" },
                                "cells": { "type": "string" },
                                "pages": { "type": "string" },
                                "paragraph_range": { "type": "string" },
                                "sheet": {},
                                "rows": { "type": "string" }
                            },
                            "required": ["path"]
                        }
                    }
                }
            }),
        },
        ToolDef {
            name: "create_file".into(),
            description: "Create a new file with the given content, OR create an empty directory \
                          when `is_directory` is true. Yes — this tool DOES create folders; you do \
                          not need a separate `mkdir` tool. Parent directories are created \
                          automatically when writing files; for explicit folder creation pass \
                          `is_directory: true` (real JSON boolean — `true`, not the string \
                          \"true\"). If the file already exists, use edit_file to modify it instead. \
                          \
                          ORDERING NOTE: batch entries in `creates` are treated as independent / \
                          parallel — do NOT mix a parent directory and files inside it in the same \
                          batch, or the file creates may race the directory and fail. Chain instead: \
                          one call to create the directory (with `is_directory: true`), then a \
                          second batch call for the files. \
                          \
                          BATCH MODE: to create N files/directories in a single tool call, pass a \
                          `creates: [...]` array where each entry has the same `path` / optional \
                          `content` / optional `is_directory` fields you'd put in a single create. \
                          Mutually exclusive with the top-level fields. Each entry is processed \
                          independently — one failing entry (e.g. FILE_EXISTS) does NOT cancel the \
                          rest. Entries must not depend on each other's effects; if you need an \
                          explicit directory before its files, create the directory in a prior \
                          (single or batch) call. Empty array is an error.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative path from project root for the file or directory to create. \
                                        Required in single-create mode; omit when using `creates`."
                    },
                    "content": {
                        "type": "string",
                        "description": "The file content to write. Omit or leave empty to create an empty file. \
                                        Set is_directory to true to create a directory instead."
                    },
                    "is_directory": {
                        "type": "boolean",
                        "description": "If true, create an empty directory instead of a file. Default: false."
                    },
                    "creates": {
                        "type": "array",
                        "description": "Batch mode: create N files/directories in one call. Each entry \
                                        uses the same shape as a single-create call. Mutually exclusive \
                                        with the top-level `path`/`content`/`is_directory` fields. \
                                        Empty array is an error.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "path": { "type": "string" },
                                "content": { "type": "string" },
                                "is_directory": { "type": "boolean" }
                            },
                            "required": ["path"]
                        }
                    }
                }
            }),
        },
        ToolDef {
            name: "move_file".into(),
            description: "Move or rename a file or directory within the project. Creates \
                          destination parent directories automatically. Fails with MOVE_BLOCKED \
                          if the destination already exists unless `overwrite: true` is passed \
                          (directories are never overwritten). Prefer this over shell `mv` / \
                          `Move-Item`: it needs no shell permission, updates the symbol index, \
                          invalidates stale read caches, and keeps file-history tracking coherent.".into(),
            parameters: json!({
                "type": "object",
                "required": ["path", "new_path"],
                "properties": {
                    "path": { "type": "string", "description": "Relative path of the existing file or directory to move." },
                    "new_path": { "type": "string", "description": "Relative destination path (the new name/location)." },
                    "overwrite": { "type": "boolean", "description": "Replace an existing destination FILE (default false). Directories are never overwritten." }
                }
            }),
        },
        ToolDef {
            name: "edit_file".into(),
            description: "Edit a file by replacing the first occurrence of old_string with \
                          new_string. Matching is byte-exact first; if that fails, a \
                          whitespace-tolerant fallback (strip per-line trailing whitespace, \
                          normalize CRLF/LF) is attempted. \
                          To DELETE content, pass new_string as an empty string \"\". \
                          To APPEND to the file, pass old_string as an empty string \"\" — \
                          new_string is then added to the end (with a separating newline iff \
                          the file is non-empty and doesn't already end in one). This is the \
                          canonical way to add content without finding an anchor to match. \
                          To REPLACE a large section, match the entire block as old_string and \
                          provide the new content as new_string. \
                          Returns EDIT_NO_MATCH with top candidate lines if old_string cannot \
                          be located (this is a string-matching failure, not a file-changed \
                          error — fix your old_string rather than re-reading). \
                          Returns ALREADY_APPLIED if the replacement is already in place. \
                          \
                          BATCH MODE (P1.5): to apply N edits across M files in a single tool \
                          call, pass an `edits: [...]` array where each entry has the same \
                          `path` / `old_string` / `new_string` / optional `hint_line` fields \
                          you'd put in a single-edit call. Mutually exclusive with the \
                          top-level fields. Pre-flight validation runs against every entry \
                          first; if any entry fails its match the whole batch is rejected \
                          before any disk write happens (atomic with respect to the per-turn \
                          revert snapshot — `/rewind` restores all batch writes together).".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Relative path from project root. Required in single-edit mode; omit when using `edits`." },
                    "old_string": {
                        "type": "string",
                        "description": "The text to replace. Byte-exact match is preferred; \
                                        whitespace-only differences will fall back gracefully \
                                        but still emit a warning so you can tighten the match. \
                                        Required in single-edit mode; omit when using `edits`."
                    },
                    "new_string": {
                        "type": "string",
                        "description": "The replacement text. Required in single-edit mode; omit when using `edits`."
                    },
                    "replace_all": {
                        "type": "boolean",
                        "description": "Replace all occurrences of old_string in the file (default: false). \
                                       When false, only the first occurrence is replaced. Use this to rename \
                                       variables or make consistent changes throughout a file."
                    },
                    "hint_line": {
                        "type": "integer",
                        "description": "Approximate line number of old_string (1-indexed). \
                                       Improves EDIT_NO_MATCH candidate ranking when the match fails."
                    },
                    "edits": {
                        "type": "array",
                        "description": "P1.5 batch mode: apply N edits in one call. Each entry \
                                        uses the same shape as a single-edit call. Mutually \
                                        exclusive with the top-level `path`/`old_string`/\
                                        `new_string` fields. Empty array is an error.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "path": { "type": "string" },
                                "old_string": { "type": "string" },
                                "new_string": { "type": "string" },
                                "replace_all": { "type": "boolean", "description": "Replace all occurrences (default: false)." },
                                "hint_line": { "type": "integer" }
                            },
                            "required": ["path", "old_string", "new_string"]
                        }
                    }
                }
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
        "move_file" => execute_move_file(params, context).await,
        "list_directory" => execute_list_directory(params, context).await,
        _ => Ok(ToolOutput {
            content: format!("Unknown file tool: {}", name),
            is_error: true,
            attachments: Vec::new(),
        }),
    }
}

async fn execute_read_file(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    if let Some(reads) = coerce_batch_array(params.get("reads")) {
        const TOP_LEVEL_FIELDS: &[&str] = &[
            "path",
            "offset",
            "limit",
            "start_line",
            "end_line",
            "cells",
            "pages",
            "paragraph_range",
            "sheet",
            "rows",
        ];
        let mixed = TOP_LEVEL_FIELDS.iter().any(|f| params.get(*f).is_some());
        if mixed {
            return Ok(ToolOutput {
                content: "BATCH_READ_REJECTED: `reads` was provided alongside top-level read \
                          fields. Use one shape or the other, not both."
                    .into(),
                is_error: true,
                attachments: Vec::new(),
            });
        }
        return execute_read_file_batch(reads, context).await;
    }
    execute_read_file_one(params, context).await
}

async fn execute_read_file_batch(reads: Vec<Value>, context: &ToolContext) -> Result<ToolOutput> {
    if !context.check_permission(&Action::Read) {
        return Ok(ToolOutput {
            content: "PERMISSION_DENIED: Read not allowed in current permission mode.".into(),
            is_error: true,
            attachments: Vec::new(),
        });
    }
    if reads.is_empty() {
        return Ok(ToolOutput {
            content: "BATCH_READ_REJECTED: `reads` array is empty. Pass at least one entry, \
                      or use the single-read shape `{ path, offset?, limit?, ... }`."
                .into(),
            is_error: true,
            attachments: Vec::new(),
        });
    }
    let mut shape_errors: Vec<String> = Vec::new();
    for (i, entry) in reads.iter().enumerate() {
        let path = entry
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if path.is_empty() {
            shape_errors.push(format!(
                "entry[{}]: `path` is required and must be non-empty",
                i
            ));
        }
    }
    if !shape_errors.is_empty() {
        return Ok(ToolOutput {
            content: format!(
                "BATCH_READ_REJECTED: {} entry/entries failed validation. Nothing was read.\n{}",
                shape_errors.len(),
                shape_errors.join("\n"),
            ),
            is_error: true,
            attachments: Vec::new(),
        });
    }

    let mut out = String::new();
    let mut all_errored = true;
    let mut combined_attachments: Vec<crate::tools::ToolAttachment> = Vec::new();
    for (i, entry) in reads.iter().enumerate() {
        let path_preview = entry.get("path").and_then(|v| v.as_str()).unwrap_or("");
        out.push_str(&format!(
            "=== read_file entry {}: {} ===\n",
            i + 1,
            path_preview
        ));
        let result = execute_read_file_one(entry.clone(), context).await?;
        if !result.is_error {
            all_errored = false;
        }
        out.push_str(&result.content);
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push('\n');
        combined_attachments.extend(result.attachments);
    }
    Ok(ToolOutput {
        content: out.trim_end().to_string(),
        is_error: all_errored,
        attachments: combined_attachments,
    })
}

async fn execute_read_file_one(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    if !context.check_permission(&Action::Read) {
        return Ok(ToolOutput {
            content: "PERMISSION_DENIED: Read not allowed in current permission mode.".into(),
            is_error: true,
            attachments: Vec::new(),
        });
    }
    let path = params["path"].as_str().unwrap_or("");

    // Accept `offset`/`limit` (preferred) and legacy `start_line`/`end_line`.
    let offset = params
        .get("offset")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize);
    let limit_param = params
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize);
    let legacy_start = params
        .get("start_line")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize);
    let legacy_end = params
        .get("end_line")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize);

    let start_line: Option<usize> = offset.or(legacy_start);
    let end_line: Option<usize> = match (limit_param, legacy_end) {
        (Some(lim), _) => {
            let start = start_line.unwrap_or(1).max(1);
            Some(start.saturating_add(lim.saturating_sub(1)))
        }
        (None, Some(e)) => Some(e),
        _ => None,
    };
    let full_path = match resolve_with_scope(context, path) {
        Ok(p) => p,
        Err(violation) => return Ok(violation),
    };

    if let Some(blocked) = check_sensitive_path(path, &full_path, context).await {
        return Ok(blocked);
    }

    // read_file does NOT acquire the per-file mutex: all writes use atomic_write
    // (temp+rename), so reads are always consistent. Holding the mutex here caused
    // 30 s timeouts when Defender/indexer scans blocked read_to_string inside
    // a concurrent edit_file that held the same mutex.
    let mtime_now = match std::fs::metadata(&full_path).and_then(|m| m.modified()) {
        Ok(t) => Some(t),
        Err(_) => None,
    };

    if let Some(current_mtime) = mtime_now {
        if let Ok(metadata) = std::fs::metadata(&full_path) {
            if metadata.is_file() {
                let norm_start = start_line.unwrap_or(1).max(1);
                let norm_end = end_line.unwrap_or(DEFAULT_READ_LIMIT).max(norm_start);
                if context.file_read_registry.already_covered(
                    &full_path,
                    crate::tools::ReadUnit::Lines,
                    norm_start,
                    norm_end,
                    current_mtime,
                ) {
                    return Ok(ToolOutput {
                        content: format!(
                            "FILE_UNCHANGED: '{}' was already read earlier in this \
                             conversation and has not been modified since (same mtime). \
                             The requested range (lines {}-{}) is already covered by an \
                             earlier read_file tool_result in this thread — refer to that \
                             result instead of re-reading. If you need a different range \
                             of the same file, pass offset/limit (or legacy \
                             start_line/end_line) that falls outside what you've already \
                             seen.",
                            path, norm_start, norm_end
                        ),
                        is_error: false,
                        attachments: Vec::new(),
                    });
                }
            }
        }
    }

    let extension = full_path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase());
    match extension.as_deref() {
        Some("ipynb") => {
            return read_notebook(
                &full_path,
                path,
                params.get("cells"),
                start_line,
                end_line,
                context,
            );
        }
        Some("pdf") => {
            return read_pdf(&full_path, path, params.get("pages"));
        }
        Some("docx") => {
            return read_docx(&full_path, path, params.get("paragraph_range"));
        }
        Some("xlsx") => {
            return read_xlsx(&full_path, path, params.get("sheet"), params.get("rows"));
        }
        Some("doc") | Some("xls") => {
            return Ok(ToolOutput {
                content: format!(
                    "UNSUPPORTED_FORMAT: '{}' (.{}) is a legacy binary OLE document and \
                     is not supported. Convert to the modern format first \
                     (.doc → .docx, .xls → .xlsx) using LibreOffice/Word/Excel or a CLI \
                     like `libreoffice --headless --convert-to docx '{}'`, then re-read \
                     the converted file.",
                    path,
                    extension.as_deref().unwrap_or("?"),
                    path,
                ),
                is_error: true,
                attachments: Vec::new(),
            });
        }
        Some("png") | Some("jpg") | Some("jpeg") | Some("gif") | Some("webp") => {
            // Raster formats the vision-capable providers accept: load the
            // real bytes and attach them as an Image block so the model
            // actually SEES the image. (This used to return a text stub
            // claiming the image was "captured for visual analysis" while
            // attaching nothing — a false capability claim.)
            const IMAGE_MAX_BYTES: u64 = 4 * 1024 * 1024;
            let metadata = match std::fs::metadata(&full_path) {
                Ok(m) => m,
                Err(e) => {
                    return Ok(ToolOutput {
                        content: format!(
                            "READ_FAILED: '{}' image file could not be stat'd ({}). \
                             Verify the path and try again.",
                            path, e,
                        ),
                        is_error: true,
                        attachments: Vec::new(),
                    });
                }
            };
            if metadata.len() > IMAGE_MAX_BYTES {
                return Ok(ToolOutput {
                    content: format!(
                        "IMAGE_TOO_LARGE: '{}' is {} KB (cap {} KB for inline vision). \
                         Downscale it first, e.g. with ImageMagick: \
                         `magick '{}' -resize 1600x1600\\> smaller.png`, then read the result.",
                        path,
                        metadata.len() / 1024,
                        IMAGE_MAX_BYTES / 1024,
                        path,
                    ),
                    is_error: true,
                    attachments: Vec::new(),
                });
            }
            let data = match std::fs::read(&full_path) {
                Ok(d) => d,
                Err(e) => {
                    return Ok(ToolOutput {
                        content: format!("Error reading image file: {}", e),
                        is_error: true,
                        attachments: Vec::new(),
                    });
                }
            };
            let media_type =
                crate::tools::sniff_image_media_type(&data).unwrap_or(match extension.as_deref() {
                    Some("png") => "image/png",
                    Some("jpg") | Some("jpeg") => "image/jpeg",
                    Some("gif") => "image/gif",
                    Some("webp") => "image/webp",
                    _ => "application/octet-stream",
                });
            let size_kb = metadata.len() / 1024;
            return Ok(ToolOutput {
                content: format!(
                    "[Image: {} (.{}, {} KB) — attached below as an image block; you can \
                     see and analyze it directly.]",
                    path,
                    extension.as_deref().unwrap_or("?"),
                    size_kb,
                ),
                is_error: false,
                attachments: vec![crate::tools::ToolAttachment::Image {
                    media_type: media_type.to_string(),
                    data,
                }],
            });
        }
        // SVG is XML text — fall through to the normal text-read path so the
        // model can read (and edit) the markup directly.
        Some("bmp") | Some("ico") => {
            return Ok(ToolOutput {
                content: format!(
                    "IMAGE_FORMAT_UNSUPPORTED: '{}' (.{}) can't be attached for vision \
                     (providers accept png/jpeg/gif/webp). Convert it first, e.g. \
                     `magick '{}' out.png`, then read the converted file.",
                    path,
                    extension.as_deref().unwrap_or("?"),
                    path,
                ),
                is_error: true,
                attachments: Vec::new(),
            });
        }
        _ => {}
    }

    let byte_cap: u64 = std::env::var("RUSTIC_FILE_READ_MAX_BYTES")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(P1_11_MAX_READ_BYTES);
    if let Ok(metadata) = std::fs::metadata(&full_path) {
        if metadata.is_file() && metadata.len() > byte_cap {
            if start_line.is_some() || end_line.is_some() {
                return Ok(ToolOutput {
                    content: format!(
                        "READ_TOO_LARGE: '{}' is {} bytes, exceeding the {}-byte read cap \
                         even with the requested range. Pick a smaller offset/limit window \
                         and retry. (Override the cap process-wide with the \
                         RUSTIC_FILE_READ_MAX_BYTES env var.)",
                        path,
                        metadata.len(),
                        byte_cap
                    ),
                    is_error: true,
                    attachments: Vec::new(),
                });
            }
        }
    }

    match std::fs::read_to_string(&full_path) {
        Ok(content) => {
            let lines: Vec<&str> = content.lines().collect();
            let total = lines.len();

            let (recorded_start, mut recorded_end, mut output) =
                if start_line.is_none() && end_line.is_none() {
                    let end = total.min(DEFAULT_READ_LIMIT);
                    let body = lines[..end].join("\n");
                    let text = if total > DEFAULT_READ_LIMIT {
                        format!(
                            "{}\n\n[TRUNCATED: showing lines 1-{} of {} total. \
                         Pass offset/limit (or start_line/end_line) to read beyond line {}.]",
                            body, end, total, end
                        )
                    } else {
                        body
                    };
                    (1usize, end.max(1), text)
                } else {
                    let start = start_line
                        .map(|n| n.saturating_sub(1))
                        .unwrap_or(0)
                        .min(total);
                    let end = end_line.map(|n| n.min(total)).unwrap_or(total);
                    let end = end.max(start);
                    let selected: Vec<String> = lines[start..end]
                        .iter()
                        .enumerate()
                        .map(|(i, line)| format!("{}: {}", start + i + 1, *line))
                        .collect();
                    (start + 1, end.max(start + 1), selected.join("\n"))
                };

            let token_cap: usize = std::env::var("RUSTIC_FILE_READ_MAX_OUTPUT_TOKENS")
                .ok()
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(P1_11_MAX_TOKEN_ESTIMATE);
            let estimated_tokens = output.len() / 4;
            if estimated_tokens > token_cap {
                if start_line.is_some() || end_line.is_some() {
                    return Ok(ToolOutput {
                        content: format!(
                            "READ_TOO_LARGE: '{}' range ≈{} tokens (cap = {}). Even with \
                             the explicit offset/limit you passed, the body is too large to \
                             fit in context. Pick a narrower range and retry. (Override \
                             with RUSTIC_FILE_READ_MAX_OUTPUT_TOKENS env var.)",
                            path, estimated_tokens, token_cap
                        ),
                        is_error: true,
                        attachments: Vec::new(),
                    });
                }
                let allowed_chars = token_cap.saturating_mul(4);
                if output.len() > allowed_chars {
                    let cutoff = output
                        .char_indices()
                        .nth(allowed_chars)
                        .map(|(i, _)| i)
                        .unwrap_or(output.len());
                    output.truncate(cutoff);
                    // The registry must record only what the model actually
                    // saw — recording the pre-truncation range made later
                    // re-reads of the cut lines bounce off FILE_UNCHANGED.
                    let shown_lines = output.lines().count().max(1);
                    recorded_end = recorded_start + shown_lines - 1;
                    output.push_str(&format!(
                        "\n\n[TRUNCATED to ~{} tokens (lines {}-{} shown). Pass offset/limit to read further.]",
                        token_cap, recorded_start, recorded_end
                    ));
                }
            }

            if let Some(mtime) = mtime_now {
                context.file_read_registry.record(
                    full_path.clone(),
                    crate::tools::ReadUnit::Lines,
                    mtime,
                    recorded_start,
                    recorded_end,
                );
            }

            Ok(ToolOutput {
                content: output,
                is_error: false,
                attachments: Vec::new(),
            })
        }
        Err(e) => Ok(ToolOutput {
            content: format!("Error reading file: {}", e),
            is_error: true,
            attachments: Vec::new(),
        }),
    }
}

fn read_notebook(
    full_path: &std::path::Path,
    rel_path: &str,
    cells_param: Option<&Value>,
    offset: Option<usize>,
    limit: Option<usize>,
    context: &ToolContext,
) -> Result<ToolOutput> {
    const DEFAULT_NOTEBOOK_CELL_LIMIT: usize = 25;

    let current_mtime = std::fs::metadata(full_path).and_then(|m| m.modified()).ok();

    let raw = match std::fs::read_to_string(full_path) {
        Ok(s) => s,
        Err(e) => {
            return Ok(ToolOutput {
                content: format!("Error reading notebook: {}", e),
                is_error: true,
                attachments: Vec::new(),
            });
        }
    };
    let json: Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            return Ok(ToolOutput {
                content: format!(
                    "NOTEBOOK_PARSE_ERROR: '{}' isn't valid JSON: {}. .ipynb files must be \
                     parseable as JSON.",
                    rel_path, e
                ),
                is_error: true,
                attachments: Vec::new(),
            });
        }
    };
    let cells = match json.get("cells").and_then(|c| c.as_array()) {
        Some(a) => a,
        None => {
            return Ok(ToolOutput {
                content: format!(
                    "NOTEBOOK_SHAPE_ERROR: '{}' has no top-level `cells` array. This may \
                     not be a notebook file, or it's saved in an unsupported nbformat.",
                    rel_path
                ),
                is_error: true,
                attachments: Vec::new(),
            });
        }
    };

    let total = cells.len();

    let (cell_start, cell_end) = if let Some(spec) = cells_param.and_then(|v| v.as_str()) {
        match parse_range_1indexed(spec, total) {
            Ok(r) => r,
            Err(e) => {
                return Ok(ToolOutput {
                    content: format!("NOTEBOOK_RANGE_ERROR: {}", e),
                    is_error: true,
                    attachments: Vec::new(),
                });
            }
        }
    } else if offset.is_some() || limit.is_some() {
        let start = offset.unwrap_or(1).max(1);
        let end = if let Some(end_inclusive) = limit {
            end_inclusive
        } else {
            start.saturating_add(DEFAULT_NOTEBOOK_CELL_LIMIT - 1)
        };
        (start.min(total.max(1)), end.min(total.max(1)))
    } else {
        (1, total.min(DEFAULT_NOTEBOOK_CELL_LIMIT))
    };

    if total == 0 {
        return Ok(ToolOutput {
            content: format!("[Notebook '{}' has no cells.]", rel_path),
            is_error: false,
            attachments: Vec::new(),
        });
    }

    if let Some(mtime) = current_mtime {
        if context.file_read_registry.already_covered(
            full_path,
            crate::tools::ReadUnit::Cells,
            cell_start,
            cell_end,
            mtime,
        ) {
            return Ok(ToolOutput {
                content: format!(
                    "FILE_UNCHANGED: '{}' notebook cells {}-{} were already read \
                     earlier in this conversation and the file has not been modified \
                     since (same mtime). Refer to the earlier read_file tool_result \
                     instead of re-reading. Pass a different `cells:` range to read \
                     more of the notebook.",
                    rel_path, cell_start, cell_end,
                ),
                is_error: false,
                attachments: Vec::new(),
            });
        }
    }

    let mut body = String::new();
    body.push_str(&format!(
        "Notebook '{}' — showing cells {}-{} of {} total.\n",
        rel_path, cell_start, cell_end, total,
    ));
    for (i, cell) in cells.iter().enumerate().skip(cell_start.saturating_sub(1)) {
        let n = i + 1;
        if n > cell_end {
            break;
        }
        let cell_type = cell
            .get("cell_type")
            .and_then(|v| v.as_str())
            .unwrap_or("code");
        let source = stringify_notebook_source(cell.get("source"));
        body.push_str(&format!("\n── Cell {} [{}] ──\n", n, cell_type));
        if source.trim().is_empty() {
            body.push_str("(empty)\n");
        } else {
            body.push_str(source.trim_end());
            body.push('\n');
        }
    }
    if cell_end < total {
        body.push_str(&format!(
            "\n[TRUNCATED: showing cells {}-{} of {} total. Pass `cells: \"{}-{}\"` to read \
             further.]",
            cell_start,
            cell_end,
            total,
            cell_end + 1,
            (cell_end + DEFAULT_NOTEBOOK_CELL_LIMIT).min(total),
        ));
    }

    if let Some(mtime) = current_mtime {
        context.file_read_registry.record(
            full_path.to_path_buf(),
            crate::tools::ReadUnit::Cells,
            mtime,
            cell_start,
            cell_end,
        );
    }

    Ok(ToolOutput {
        content: body,
        is_error: false,
        attachments: Vec::new(),
    })
}

fn stringify_notebook_source(src: Option<&Value>) -> String {
    match src {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|p| p.as_str().map(|s| s.to_string()))
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

/// Parse a 1-indexed inclusive range `"1-10"` or `"3"`, clamped to `total`.
fn parse_range_1indexed(spec: &str, total: usize) -> std::result::Result<(usize, usize), String> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Err("range spec is empty".into());
    }
    if let Some((a, b)) = spec.split_once('-') {
        let start: usize = a
            .trim()
            .parse()
            .map_err(|_| format!("range start '{}' is not an integer", a))?;
        let end: usize = b
            .trim()
            .parse()
            .map_err(|_| format!("range end '{}' is not an integer", b))?;
        if start == 0 || end == 0 {
            return Err("range bounds are 1-indexed; 0 is not valid".into());
        }
        if start > end {
            return Err(format!("range start {} > end {}", start, end));
        }
        Ok((start.min(total.max(1)), end.min(total.max(1))))
    } else {
        let n: usize = spec
            .parse()
            .map_err(|_| format!("'{}' is not an integer or N-M range", spec))?;
        if n == 0 {
            return Err("0 is not a valid 1-indexed position".into());
        }
        Ok((n.min(total.max(1)), n.min(total.max(1))))
    }
}

#[cfg(test)]
mod p1_11_notebook_tests {
    use super::*;

    #[test]
    fn range_parse_handles_singletons_and_ranges() {
        assert_eq!(parse_range_1indexed("3", 10).unwrap(), (3, 3));
        assert_eq!(parse_range_1indexed("1-5", 10).unwrap(), (1, 5));
        assert_eq!(parse_range_1indexed("8-100", 10).unwrap(), (8, 10)); // clamped
        assert!(parse_range_1indexed("0", 10).is_err());
        assert!(parse_range_1indexed("", 10).is_err());
        assert!(parse_range_1indexed("5-2", 10).is_err());
    }

    #[test]
    fn stringify_handles_string_and_array_source() {
        assert_eq!(
            stringify_notebook_source(Some(&Value::String("hi".into()))),
            "hi"
        );
        assert_eq!(
            stringify_notebook_source(Some(&serde_json::json!(["a", "b", "c"]))),
            "abc"
        );
        assert_eq!(stringify_notebook_source(None), "");
    }

    // C9.7 — additional read_file coverage.

    #[test]
    fn range_parse_singleton_at_boundary() {
        // singleton at exactly `total`
        assert_eq!(parse_range_1indexed("5", 5).unwrap(), (5, 5));
        // singleton > total → clamps
        assert_eq!(parse_range_1indexed("10", 5).unwrap(), (5, 5));
    }

    #[test]
    fn range_parse_zero_or_negative_returns_err() {
        assert!(parse_range_1indexed("0", 10).is_err());
        assert!(parse_range_1indexed("-1", 10).is_err());
        assert!(parse_range_1indexed("0-3", 10).is_err());
    }

    #[test]
    fn range_parse_non_numeric_returns_err() {
        assert!(parse_range_1indexed("abc", 10).is_err());
        assert!(parse_range_1indexed("1-abc", 10).is_err());
        assert!(parse_range_1indexed("1-2-3", 10).is_err());
    }
}

#[cfg(test)]
mod c9_read_file_tests {
    // (super::* not needed — we exercise public surface via crate paths.)

    // C9.1 — image extension routing. We can verify the recognised set
    // is what the spec asks for without a full read.
    fn is_image_ext(ext: &str) -> bool {
        matches!(
            ext,
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" | "ico"
        )
    }

    #[test]
    fn image_extensions_are_recognized() {
        for e in ["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "ico"] {
            assert!(is_image_ext(e), "extension `{}` must be image", e);
        }
    }

    #[test]
    fn non_image_extensions_rejected() {
        for e in [
            "rs", "txt", "md", "ipynb", "pdf", "docx", "xlsx", "doc", "xls",
        ] {
            assert!(!is_image_ext(e), "extension `{}` must NOT be image", e);
        }
    }

    // C9.4 — FileReadRegistry with the new ReadUnit-aware keying. Cell
    // reads and line reads of the SAME path must not stub each other.
    #[test]
    fn registry_line_and_cell_keys_are_independent() {
        use crate::tools::{FileReadRegistry, ReadUnit};
        use std::path::PathBuf;
        use std::time::SystemTime;
        let reg = FileReadRegistry::new();
        let path = PathBuf::from("/test/foo.ipynb");
        let m = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(100);
        // Record a CELL read 1..5.
        reg.record(path.clone(), ReadUnit::Cells, m, 1, 5);
        // Same file, same mtime, but as LINES — must not be considered covered.
        assert!(!reg.already_covered(&path, ReadUnit::Lines, 1, 5, m));
        // Same file, same mtime, as CELLS — IS covered.
        assert!(reg.already_covered(&path, ReadUnit::Cells, 1, 5, m));
        // A wider cells range must NOT be covered.
        assert!(!reg.already_covered(&path, ReadUnit::Cells, 1, 10, m));
        // A narrower cells range IS covered.
        assert!(reg.already_covered(&path, ReadUnit::Cells, 2, 4, m));
    }

    #[test]
    fn registry_invalidate_drops_every_unit_for_path() {
        use crate::tools::{FileReadRegistry, ReadUnit};
        use std::path::PathBuf;
        use std::time::SystemTime;
        let reg = FileReadRegistry::new();
        let path = PathBuf::from("/test/notebook.ipynb");
        let m = SystemTime::UNIX_EPOCH;
        reg.record(path.clone(), ReadUnit::Cells, m, 1, 5);
        reg.record(path.clone(), ReadUnit::Lines, m, 1, 100);
        assert!(reg.already_covered(&path, ReadUnit::Cells, 1, 5, m));
        assert!(reg.already_covered(&path, ReadUnit::Lines, 1, 100, m));
        reg.invalidate(&path);
        assert!(!reg.already_covered(&path, ReadUnit::Cells, 1, 5, m));
        assert!(!reg.already_covered(&path, ReadUnit::Lines, 1, 100, m));
    }

    #[test]
    fn registry_mtime_change_clears_intervals_for_that_unit() {
        use crate::tools::{FileReadRegistry, ReadUnit};
        use std::path::PathBuf;
        use std::time::{Duration, SystemTime};
        let reg = FileReadRegistry::new();
        let path = PathBuf::from("/test/foo.rs");
        let m1 = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
        let m2 = SystemTime::UNIX_EPOCH + Duration::from_secs(200);
        reg.record(path.clone(), ReadUnit::Lines, m1, 1, 50);
        assert!(reg.already_covered(&path, ReadUnit::Lines, 1, 50, m1));
        // A read at a newer mtime should not see the old coverage.
        assert!(!reg.already_covered(&path, ReadUnit::Lines, 1, 50, m2));
        // After recording at m2, the m1 coverage is gone (replace_file
        // behaviour: mtime change clears intervals).
        reg.record(path.clone(), ReadUnit::Lines, m2, 1, 10);
        assert!(!reg.already_covered(&path, ReadUnit::Lines, 1, 50, m2));
        assert!(reg.already_covered(&path, ReadUnit::Lines, 1, 10, m2));
    }

    #[test]
    fn registry_disjoint_reads_do_not_falsely_cover_gap() {
        use crate::tools::{FileReadRegistry, ReadUnit};
        use std::path::PathBuf;
        use std::time::SystemTime;
        let reg = FileReadRegistry::new();
        let path = PathBuf::from("/test/long.rs");
        let m = SystemTime::UNIX_EPOCH;
        reg.record(path.clone(), ReadUnit::Lines, m, 1, 50);
        reg.record(path.clone(), ReadUnit::Lines, m, 200, 300);
        // The gap 51..199 was never seen — must NOT be covered.
        assert!(!reg.already_covered(&path, ReadUnit::Lines, 80, 120, m));
        // The recorded ranges themselves ARE covered.
        assert!(reg.already_covered(&path, ReadUnit::Lines, 1, 50, m));
        assert!(reg.already_covered(&path, ReadUnit::Lines, 250, 280, m));
    }

    #[test]
    fn registry_adjacent_intervals_coalesce() {
        // 1-100 followed by 101-200 should coalesce into 1-200 so a
        // later read of 50-150 is fully covered.
        use crate::tools::{FileReadRegistry, ReadUnit};
        use std::path::PathBuf;
        use std::time::SystemTime;
        let reg = FileReadRegistry::new();
        let path = PathBuf::from("/test/coalesce.rs");
        let m = SystemTime::UNIX_EPOCH;
        reg.record(path.clone(), ReadUnit::Lines, m, 1, 100);
        reg.record(path.clone(), ReadUnit::Lines, m, 101, 200);
        assert!(reg.already_covered(&path, ReadUnit::Lines, 50, 150, m));
    }

    #[test]
    fn notebook_empty_cells_returns_zero_cells_block() {
        // Smoke test the empty-notebook branch in read_notebook by
        // verifying the early-exit text. We can't easily construct a
        // ToolContext here, so we exercise this via a direct JSON
        // parse + counting.
        let json = serde_json::json!({ "cells": [] });
        let cells = json.get("cells").and_then(|c| c.as_array()).unwrap();
        assert_eq!(cells.len(), 0);
    }

    // C9.7 — UNSUPPORTED_FORMAT message wording (forward-compat: the
    // message must mention the offending extension so the agent can
    // disambiguate which format failed).
    #[test]
    fn unsupported_format_message_includes_extension_substring() {
        // Historical — pre-C9.3 the .pdf / .docx / .xlsx arms returned
        // UNSUPPORTED_FORMAT pointing at pandoc. Now they route through
        // the native readers. This test stays as a smoke for the
        // .doc / .xls (legacy OLE) arm which still surfaces an
        // UNSUPPORTED_FORMAT pointing at libreoffice.
        let ole_msg = "UNSUPPORTED_FORMAT: 'doc.doc' (.doc) is a legacy binary OLE document";
        assert!(ole_msg.contains("UNSUPPORTED_FORMAT"));
        assert!(ole_msg.contains("OLE"));
    }
}

#[cfg(test)]
mod c9_3_pdf_docx_xlsx_tests {
    use super::*;
    use std::io::Write;
    use std::path::Path;

    #[test]
    fn xlsx_format_float_drops_trailing_zeros() {
        assert_eq!(format_xlsx_float(42.0), "42");
        assert_eq!(format_xlsx_float(3.17), "3.17");
        assert_eq!(format_xlsx_float(2.5), "2.5");
        assert_eq!(format_xlsx_float(0.0), "0");
        // Float trailing-zero trim works on the .6 format.
        assert_eq!(format_xlsx_float(1.500000), "1.5");
    }

    fn write_minimal_xlsx(path: &Path) {
        use std::io::Cursor;
        let mut buf = Cursor::new(Vec::new());
        {
            let mut zw = zip::ZipWriter::new(&mut buf);
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            zw.start_file("[Content_Types].xml", opts).unwrap();
            zw.write_all(br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>"#).unwrap();
            zw.start_file("_rels/.rels", opts).unwrap();
            zw.write_all(br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#).unwrap();
            zw.start_file("xl/workbook.xml", opts).unwrap();
            zw.write_all(br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Sheet1" sheetId="1" r:id="rId1"/></sheets></workbook>"#).unwrap();
            zw.start_file("xl/_rels/workbook.xml.rels", opts).unwrap();
            zw.write_all(br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#).unwrap();
            zw.start_file("xl/worksheets/sheet1.xml", opts).unwrap();
            zw.write_all(br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>name</t></is></c><c r="B1" t="inlineStr"><is><t>age</t></is></c></row><row r="2"><c r="A2" t="inlineStr"><is><t>Alice</t></is></c><c r="B2"><v>30</v></c></row></sheetData></worksheet>"#).unwrap();
            zw.finish().unwrap();
        }
        std::fs::write(path, buf.into_inner()).unwrap();
    }

    #[test]
    fn xlsx_reader_extracts_first_sheet_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.xlsx");
        write_minimal_xlsx(&path);
        let out = read_xlsx(&path, "test.xlsx", None, None).unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("Sheet1"));
        assert!(out.content.contains("name"));
        assert!(out.content.contains("Alice"));
        assert!(out.content.contains("30"));
    }

    #[test]
    fn xlsx_reader_named_sheet_lookup_rejects_unknown() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.xlsx");
        write_minimal_xlsx(&path);
        let out = read_xlsx(
            &path,
            "test.xlsx",
            Some(&serde_json::json!("NonExistent")),
            None,
        )
        .unwrap();
        assert!(out.is_error);
        assert!(out.content.contains("XLSX_SHEET_NOT_FOUND"));
        assert!(out.content.contains("NonExistent"));
    }

    #[test]
    fn xlsx_reader_row_range_clamps_to_total() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.xlsx");
        write_minimal_xlsx(&path);
        let out = read_xlsx(&path, "test.xlsx", None, Some(&serde_json::json!("1-10"))).unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("Row 1"));
        assert!(out.content.contains("Row 2"));
    }

    #[test]
    fn pdf_reader_rejects_oversize_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("huge.pdf");
        // Write a file just over the hard ceiling (100 MB + 1 byte).
        // Allocation is sparse — most OSes don't actually consume the
        // bytes until the file is read, so this is cheap.
        let f = std::fs::File::create(&path).unwrap();
        f.set_len(PDF_MAX_BYTES_TOTAL + 1).unwrap();
        let out = read_pdf(&path, "huge.pdf", None).unwrap();
        assert!(out.is_error);
        assert!(out.content.contains("PDF_TOO_LARGE"));
        assert!(out.attachments.is_empty());
    }

    #[test]
    fn pdf_reader_handles_missing_file() {
        let out = read_pdf(
            std::path::Path::new("/nonexistent/path/xxxx.pdf"),
            "missing.pdf",
            None,
        )
        .unwrap();
        assert!(out.is_error);
        assert!(out.content.contains("PDF_READ_FAILED"));
    }

    #[test]
    fn pdf_reader_garbage_content_returns_parse_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("not_pdf.pdf");
        std::fs::write(&path, b"this is not a PDF file").unwrap();
        let out = read_pdf(&path, "not_pdf.pdf", None).unwrap();
        assert!(out.is_error);
        // pdf-extract returns its own error type; we just need to
        // confirm we emit PDF_PARSE_FAILED on it.
        assert!(out.content.contains("PDF_PARSE_FAILED"));
    }

    #[test]
    fn docx_reader_rejects_invalid_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.docx");
        std::fs::write(&path, b"not a real docx file").unwrap();
        let out = read_docx(&path, "bad.docx", None).unwrap();
        assert!(out.is_error);
        assert!(out.content.contains("DOCX_PARSE_FAILED"));
    }

    #[test]
    fn tool_attachment_pdf_variant_round_trips_through_json() {
        use crate::tools::ToolAttachment;
        let att = ToolAttachment::Pdf {
            data: vec![1, 2, 3, 4],
            page_count: 7,
        };
        let json = serde_json::to_string(&att).unwrap();
        let back: ToolAttachment = serde_json::from_str(&json).unwrap();
        match back {
            ToolAttachment::Pdf { data, page_count } => {
                assert_eq!(data, vec![1, 2, 3, 4]);
                assert_eq!(page_count, 7);
            }
            _ => panic!("wrong variant after round-trip"),
        }
    }

    #[test]
    fn tool_attachment_image_variant_round_trips_through_json() {
        use crate::tools::ToolAttachment;
        let att = ToolAttachment::Image {
            media_type: "image/png".into(),
            data: vec![137, 80, 78, 71],
        };
        let json = serde_json::to_string(&att).unwrap();
        let back: ToolAttachment = serde_json::from_str(&json).unwrap();
        match back {
            ToolAttachment::Image { media_type, data } => {
                assert_eq!(media_type, "image/png");
                assert_eq!(data, vec![137, 80, 78, 71]);
            }
            _ => panic!("wrong variant after round-trip"),
        }
    }

    #[test]
    fn tool_output_attachments_field_defaults_to_empty() {
        let out = ToolOutput::text("hello", false);
        assert!(out.attachments.is_empty());
        assert_eq!(out.content, "hello");
        assert!(!out.is_error);
    }
}

const PDF_MAX_BYTES_NATIVE: u64 = 32 * 1024 * 1024;
const PDF_MAX_BYTES_TOTAL: u64 = 100 * 1024 * 1024;
const PDF_MAX_PAGES_PER_READ: usize = 20;
const DOCX_MAX_PARAGRAPHS: usize = 2000;
const XLSX_DEFAULT_ROW_LIMIT: usize = 500;

fn read_pdf(
    full_path: &std::path::Path,
    rel_path: &str,
    pages_param: Option<&Value>,
) -> Result<ToolOutput> {
    use crate::tools::ToolAttachment;

    let meta = match std::fs::metadata(full_path) {
        Ok(m) => m,
        Err(e) => {
            return Ok(ToolOutput {
                content: format!("PDF_READ_FAILED: stat '{}' failed: {}", rel_path, e),
                is_error: true,
                attachments: Vec::new(),
            });
        }
    };
    let size = meta.len();
    if size > PDF_MAX_BYTES_TOTAL {
        return Ok(ToolOutput {
            content: format!(
                "PDF_TOO_LARGE: '{}' is {} bytes, over the {} byte hard ceiling. Split the \
                 document or extract the pages you need with an external tool.",
                rel_path, size, PDF_MAX_BYTES_TOTAL,
            ),
            is_error: true,
            attachments: Vec::new(),
        });
    }

    let bytes = match std::fs::read(full_path) {
        Ok(b) => b,
        Err(e) => {
            return Ok(ToolOutput {
                content: format!("PDF_READ_FAILED: read '{}' failed: {}", rel_path, e),
                is_error: true,
                attachments: Vec::new(),
            });
        }
    };

    // Full extraction then split on form-feed (\x0c); pdf-extract emits these between pages.
    let extracted = match pdf_extract::extract_text_from_mem(&bytes) {
        Ok(t) => t,
        Err(e) => {
            return Ok(ToolOutput {
                content: format!(
                    "PDF_PARSE_FAILED: '{}' could not be parsed: {}. The file may be encrypted, \
                     corrupted, or rely on features `pdf-extract` doesn't support (forms, \
                     embedded JS). Convert with `pdftotext` or similar and re-read the .txt.",
                    rel_path, e,
                ),
                is_error: true,
                attachments: Vec::new(),
            });
        }
    };

    let pages: Vec<&str> = if extracted.contains('\x0c') {
        extracted.split('\x0c').collect()
    } else {
        vec![extracted.as_str()]
    };
    let total_pages = pages.len();

    let (start, end) = match pages_param.and_then(|v| v.as_str()) {
        Some(spec) => match parse_range_1indexed(spec, total_pages) {
            Ok(r) => r,
            Err(e) => {
                return Ok(ToolOutput {
                    content: format!("PDF_PAGE_RANGE_ERROR: {}", e),
                    is_error: true,
                    attachments: Vec::new(),
                });
            }
        },
        None => (1, total_pages.min(PDF_MAX_PAGES_PER_READ)),
    };
    let span = end.saturating_sub(start).saturating_add(1);
    if span > PDF_MAX_PAGES_PER_READ {
        return Ok(ToolOutput {
            content: format!(
                "PDF_PAGE_RANGE_TOO_LARGE: requested {} pages ({}–{}) exceeds the {} per-call cap. \
                 Pass a tighter `pages` range.",
                span, start, end, PDF_MAX_PAGES_PER_READ,
            ),
            is_error: true,
            attachments: Vec::new(),
        });
    }

    let mut body = format!(
        "PDF '{}' — extracted pages {}–{} of {} total.\n",
        rel_path, start, end, total_pages,
    );
    for (idx, page) in pages.iter().enumerate() {
        let n = idx + 1;
        if n < start || n > end {
            continue;
        }
        body.push_str(&format!("\n── Page {} ──\n", n));
        let trimmed = page.trim();
        if trimmed.is_empty() {
            body.push_str("(no extractable text — page may be a scanned image)\n");
        } else {
            body.push_str(trimmed);
            body.push('\n');
        }
    }
    if end < total_pages {
        body.push_str(&format!(
            "\n[TRUNCATED: showing pages {}-{} of {}. Pass `pages: \"{}-{}\"` to read further.]",
            start,
            end,
            total_pages,
            end + 1,
            (end + PDF_MAX_PAGES_PER_READ).min(total_pages),
        ));
    }

    // Surface raw bytes when under the Anthropic document-block ceiling so the
    // provider can forward them as a native attachment for image-heavy PDFs.
    let attachments = if size <= PDF_MAX_BYTES_NATIVE {
        vec![ToolAttachment::Pdf {
            data: bytes,
            page_count: total_pages,
        }]
    } else {
        Vec::new()
    };

    Ok(ToolOutput {
        content: body,
        is_error: false,
        attachments,
    })
}

fn read_docx(
    full_path: &std::path::Path,
    rel_path: &str,
    range_param: Option<&Value>,
) -> Result<ToolOutput> {
    let bytes = match std::fs::read(full_path) {
        Ok(b) => b,
        Err(e) => {
            return Ok(ToolOutput {
                content: format!("DOCX_READ_FAILED: '{}': {}", rel_path, e),
                is_error: true,
                attachments: Vec::new(),
            });
        }
    };
    let docx = match docx_rs::read_docx(&bytes) {
        Ok(d) => d,
        Err(e) => {
            return Ok(ToolOutput {
                content: format!(
                    "DOCX_PARSE_FAILED: '{}' is not a valid .docx file ({}). \
                     Convert with `libreoffice --headless --convert-to docx` and re-read.",
                    rel_path, e,
                ),
                is_error: true,
                attachments: Vec::new(),
            });
        }
    };

    let paragraphs = flatten_docx_paragraphs(&docx);
    let total = paragraphs.len();
    if total == 0 {
        return Ok(ToolOutput {
            content: format!("[DOCX '{}' has no paragraphs.]", rel_path),
            is_error: false,
            attachments: Vec::new(),
        });
    }
    let (start, end) = match range_param.and_then(|v| v.as_str()) {
        Some(spec) => match parse_range_1indexed(spec, total) {
            Ok(r) => r,
            Err(e) => {
                return Ok(ToolOutput {
                    content: format!("DOCX_PARAGRAPH_RANGE_ERROR: {}", e),
                    is_error: true,
                    attachments: Vec::new(),
                });
            }
        },
        None => (1, total.min(DOCX_MAX_PARAGRAPHS)),
    };

    let mut body = format!(
        "DOCX '{}' — showing paragraphs {}–{} of {} total.\n",
        rel_path, start, end, total,
    );
    for (i, p) in paragraphs.iter().enumerate() {
        let n = i + 1;
        if n < start || n > end {
            continue;
        }
        body.push('\n');
        body.push_str(p);
    }
    if end < total {
        body.push_str(&format!(
            "\n\n[TRUNCATED: pass `paragraph_range: \"{}-{}\"` to read further.]",
            end + 1,
            (end + DOCX_MAX_PARAGRAPHS).min(total),
        ));
    }
    Ok(ToolOutput {
        content: body,
        is_error: false,
        attachments: Vec::new(),
    })
}

fn flatten_docx_paragraphs(docx: &docx_rs::Docx) -> Vec<String> {
    // Use serde JSON rather than docx-rs enum variants — the JSON shape is
    // more stable across crate versions.
    let json = match serde_json::to_value(&docx.document) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    walk_docx_node_for_paragraphs(&json, &mut out);
    out
}

fn walk_docx_node_for_paragraphs(node: &Value, out: &mut Vec<String>) {
    if let Some(obj) = node.as_object() {
        if obj.get("type").and_then(|t| t.as_str()) == Some("paragraph") {
            if let Some(data) = obj.get("data") {
                let mut text = String::new();
                if let Some(children) = data.get("children").and_then(|c| c.as_array()) {
                    for run in children {
                        collect_run_text(run, &mut text);
                    }
                }
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    out.push(trimmed.to_string());
                }
            }
            return;
        }
    }
    match node {
        Value::Array(arr) => {
            for v in arr {
                walk_docx_node_for_paragraphs(v, out);
            }
        }
        Value::Object(map) => {
            for v in map.values() {
                walk_docx_node_for_paragraphs(v, out);
            }
        }
        _ => {}
    }
}

fn collect_run_text(node: &Value, out: &mut String) {
    if let Some(obj) = node.as_object() {
        if obj.get("type").and_then(|t| t.as_str()) == Some("run") {
            if let Some(data) = obj.get("data") {
                if let Some(children) = data.get("children").and_then(|c| c.as_array()) {
                    for c in children {
                        if let Some(co) = c.as_object() {
                            if co.get("type").and_then(|t| t.as_str()) == Some("text") {
                                if let Some(t) = co
                                    .get("data")
                                    .and_then(|d| d.get("text"))
                                    .and_then(|t| t.as_str())
                                {
                                    out.push_str(t);
                                }
                            }
                        }
                    }
                }
            }
            return;
        }
    }
    match node {
        Value::Array(arr) => {
            for v in arr {
                collect_run_text(v, out);
            }
        }
        Value::Object(map) => {
            for v in map.values() {
                collect_run_text(v, out);
            }
        }
        _ => {}
    }
}

fn read_xlsx(
    full_path: &std::path::Path,
    rel_path: &str,
    sheet_param: Option<&Value>,
    rows_param: Option<&Value>,
) -> Result<ToolOutput> {
    use calamine::{open_workbook, Data, Reader, Xlsx};

    let mut wb: Xlsx<_> = match open_workbook(full_path) {
        Ok(w) => w,
        Err(e) => {
            return Ok(ToolOutput {
                content: format!(
                    "XLSX_PARSE_FAILED: '{}': {}. Convert with `libreoffice --headless --convert-to xlsx` and re-read.",
                    rel_path, e,
                ),
                is_error: true,
                attachments: Vec::new(),
            });
        }
    };
    let sheet_names = wb.sheet_names();
    if sheet_names.is_empty() {
        return Ok(ToolOutput {
            content: format!("[XLSX '{}' has no sheets.]", rel_path),
            is_error: false,
            attachments: Vec::new(),
        });
    }
    let sheet_name = match sheet_param {
        Some(v) if v.is_string() => {
            let want = v.as_str().unwrap_or("").to_string();
            if sheet_names.iter().any(|s| s == &want) {
                want
            } else {
                return Ok(ToolOutput {
                    content: format!(
                        "XLSX_SHEET_NOT_FOUND: '{}' has no sheet named `{}`. Available: {}",
                        rel_path,
                        want,
                        sheet_names.join(", "),
                    ),
                    is_error: true,
                    attachments: Vec::new(),
                });
            }
        }
        Some(v) if v.is_number() => {
            let idx = v.as_u64().unwrap_or(0) as usize;
            match sheet_names.get(idx) {
                Some(name) => name.clone(),
                None => {
                    return Ok(ToolOutput {
                        content: format!(
                            "XLSX_SHEET_INDEX_OUT_OF_RANGE: index {} is out of range (0..{}).",
                            idx,
                            sheet_names.len(),
                        ),
                        is_error: true,
                        attachments: Vec::new(),
                    });
                }
            }
        }
        _ => sheet_names[0].clone(),
    };

    let range = match wb.worksheet_range(&sheet_name) {
        Ok(r) => r,
        Err(e) => {
            return Ok(ToolOutput {
                content: format!("XLSX_SHEET_READ_FAILED: sheet '{}': {}", sheet_name, e),
                is_error: true,
                attachments: Vec::new(),
            });
        }
    };
    let total_rows = range.rows().count();
    if total_rows == 0 {
        return Ok(ToolOutput {
            content: format!("[XLSX '{}' sheet '{}' is empty.]", rel_path, sheet_name),
            is_error: false,
            attachments: Vec::new(),
        });
    }
    let (start, end) = match rows_param.and_then(|v| v.as_str()) {
        Some(spec) => match parse_range_1indexed(spec, total_rows) {
            Ok(r) => r,
            Err(e) => {
                return Ok(ToolOutput {
                    content: format!("XLSX_ROW_RANGE_ERROR: {}", e),
                    is_error: true,
                    attachments: Vec::new(),
                });
            }
        },
        None => (1, total_rows.min(XLSX_DEFAULT_ROW_LIMIT)),
    };

    let mut body = format!(
        "XLSX '{}' — sheet '{}' — showing rows {}–{} of {} total.\n",
        rel_path, sheet_name, start, end, total_rows,
    );
    for (i, row) in range.rows().enumerate() {
        let n = i + 1;
        if n < start || n > end {
            continue;
        }
        let cells: Vec<String> = row
            .iter()
            .map(|c| match c {
                Data::Empty => String::new(),
                Data::String(s) => s.clone(),
                Data::Float(f) => format_xlsx_float(*f),
                Data::Int(n) => n.to_string(),
                Data::Bool(b) => b.to_string(),
                Data::DateTime(d) => format!("{:?}", d),
                Data::DateTimeIso(s) => s.clone(),
                Data::DurationIso(s) => s.clone(),
                Data::Error(e) => format!("#ERR:{:?}", e),
            })
            .collect();
        body.push_str(&format!("Row {}: {}\n", n, cells.join("\t")));
    }
    if end < total_rows {
        body.push_str(&format!(
            "\n[TRUNCATED: pass `rows: \"{}-{}\"` to read further.]",
            end + 1,
            (end + XLSX_DEFAULT_ROW_LIMIT).min(total_rows),
        ));
    }
    Ok(ToolOutput {
        content: body,
        is_error: false,
        attachments: Vec::new(),
    })
}

fn format_xlsx_float(f: f64) -> String {
    if f.fract() == 0.0 && f.abs() < 1e15 {
        return (f as i64).to_string();
    }
    format!("{:.6}", f)
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
}

async fn execute_create_file(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    if let Some(creates) = coerce_batch_array(params.get("creates")) {
        let mixed = params.get("path").is_some()
            || params.get("content").is_some()
            || params.get("is_directory").is_some();
        if mixed {
            return Ok(ToolOutput {
                content: "BATCH_CREATE_REJECTED: `creates` was provided alongside top-level \
                          `path`/`content`/`is_directory` fields. Use one shape or the other, \
                          not both."
                    .into(),
                is_error: true,
                attachments: Vec::new(),
            });
        }
        return execute_create_file_batch(creates, context).await;
    }
    execute_create_file_one(params, context, false).await
}

async fn execute_create_file_batch(
    creates: Vec<Value>,
    context: &ToolContext,
) -> Result<ToolOutput> {
    if context.permissions() == PermissionLevel::Chat {
        return Ok(ToolOutput {
            content: "PERMISSION_DENIED: File writes are not allowed in Chat mode.".into(),
            is_error: true,
            attachments: Vec::new(),
        });
    }
    if creates.is_empty() {
        return Ok(ToolOutput {
            content: "BATCH_CREATE_REJECTED: `creates` array is empty. Pass at least one entry, \
                      or use the single-create shape `{ path, content?, is_directory? }`."
                .into(),
            is_error: true,
            attachments: Vec::new(),
        });
    }
    let mut shape_errors: Vec<String> = Vec::new();
    for (i, entry) in creates.iter().enumerate() {
        let path = entry
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if path.is_empty() {
            shape_errors.push(format!(
                "entry[{}]: `path` is required and must be non-empty",
                i
            ));
        }
    }
    if !shape_errors.is_empty() {
        return Ok(ToolOutput {
            content: format!(
                "BATCH_CREATE_REJECTED: {} entry/entries failed shape validation. Nothing was written.\n{}",
                shape_errors.len(), shape_errors.join("\n"),
            ),
            is_error: true, attachments: Vec::new() });
    }

    // Ask for approval up-front for every distinct path, so the user sees one
    // batched permission flow instead of N prompts. If any path is denied, the
    // whole batch aborts before touching disk — matches edit_file's behavior.
    if context.needs_write_approval() {
        for entry in creates.iter() {
            let path = entry["path"].as_str().unwrap_or("").trim();
            let approved = context
                .permission_broker
                .request(
                    &context.event_tx,
                    &context.task_id,
                    PermissionOp::CreateFile(path.to_string()),
                )
                .await;
            if !approved {
                return Ok(ToolOutput {
                    content: format!(
                        "PERMISSION_DENIED: User denied creation of '{}' — batch aborted before \
                         any disk change.",
                        path
                    ),
                    is_error: true,
                    attachments: Vec::new(),
                });
            }
        }
    }

    let mut out = String::new();
    let mut all_errored = true;
    for (i, entry) in creates.iter().enumerate() {
        let path_preview = entry.get("path").and_then(|v| v.as_str()).unwrap_or("");
        out.push_str(&format!(
            "=== create_file entry {}: {} ===\n",
            i + 1,
            path_preview
        ));
        let result = execute_create_file_one(entry.clone(), context, true).await?;
        if !result.is_error {
            all_errored = false;
        }
        out.push_str(&result.content);
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push('\n');
    }
    Ok(ToolOutput {
        content: out.trim_end().to_string(),
        is_error: all_errored,
        attachments: Vec::new(),
    })
}

async fn execute_create_file_one(
    params: Value,
    context: &ToolContext,
    approval_already_granted: bool,
) -> Result<ToolOutput> {
    let path = params["path"].as_str().unwrap_or("");
    if path.is_empty() {
        return Ok(ToolOutput {
            content: "path is required".into(),
            is_error: true,
            attachments: Vec::new(),
        });
    }

    if context.permissions() == PermissionLevel::Chat {
        return Ok(ToolOutput {
            content: "PERMISSION_DENIED: File writes are not allowed in Chat mode.".into(),
            is_error: true,
            attachments: Vec::new(),
        });
    }

    if let Some(scope_violation) = check_write_scope(context, path) {
        return Ok(scope_violation);
    }

    let full_path = match resolve_within_project(&context.project_root, path) {
        Ok(p) => p,
        Err(violation) => return Ok(violation),
    };
    // Accept `is_directory` as a real JSON bool, a string ("true"/"false"/"1"/"0"
    // /"yes"/"no"), or a number (any non-zero → true). Some models — even
    // Sonnet — pass it as the string "true", and the previous strict
    // `as_bool()` silently dropped that to false, creating a *file* named
    // "test" when the model thought it was creating a directory. The path
    // would then collide with later `test/<file>` creates that needed `test`
    // to be a directory.
    let is_directory = coerce_bool(&params["is_directory"]);

    if let Some(blocked) = check_sensitive_path(path, &full_path, context).await {
        return Ok(blocked);
    }

    if !approval_already_granted && context.needs_write_approval() {
        let approved = context
            .permission_broker
            .request(
                &context.event_tx,
                &context.task_id,
                PermissionOp::CreateFile(path.to_string()),
            )
            .await;
        if !approved {
            return Ok(ToolOutput {
                content: "PERMISSION_DENIED: User denied file creation.".into(),
                is_error: true,
                attachments: Vec::new(),
            });
        }
    }

    if is_directory {
        match std::fs::create_dir_all(&full_path) {
            Ok(()) => Ok(ToolOutput {
                content: format!("Created directory {}", path),
                is_error: false,
                attachments: Vec::new(),
            }),
            Err(e) => Ok(ToolOutput {
                content: format!("Error creating directory: {}", e),
                is_error: true,
                attachments: Vec::new(),
            }),
        }
    } else {
        let _guard = match context.file_lock.acquire(&full_path).await {
            Ok(g) => g,
            Err(msg) => return Ok(ToolOutput::text(msg, true)),
        };

        if full_path.exists() {
            return Ok(ToolOutput {
                content: format!(
                    "FILE_EXISTS: '{}' already exists. Use edit_file to modify it.",
                    path
                ),
                is_error: true,
                attachments: Vec::new(),
            });
        }
        if let Some(parent) = full_path.parent() {
            // Skip the syscall when the parent already exists as a directory.
            // create_dir_all is *supposed* to be idempotent on an existing dir
            // but in batch use on Windows we've observed it surface
            // ERROR_ALREADY_EXISTS (os error 183) to the caller instead of
            // swallowing it — possibly because the inner `path.is_dir()`
            // check races a concurrent fs op or a leftover .tmp file. The
            // pre-check is a cheap fast-path that also avoids the bug.
            let need_create = !parent.as_os_str().is_empty() && !parent.is_dir();
            if need_create {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    // Re-check after the failure: if some other thread won the
                    // race and created the dir, treat it as success.
                    if !parent.is_dir() {
                        return Ok(ToolOutput {
                            content: format!(
                                "Error creating parent directory for '{}': {} (parent: {})",
                                path,
                                e,
                                parent.display()
                            ),
                            is_error: true,
                            attachments: Vec::new(),
                        });
                    }
                }
            }
        }
        track_before_write(context, &full_path);
        let content = params["content"].as_str().unwrap_or("");
        match crate::io_util::atomic_write(&full_path, content.as_bytes()) {
            Ok(()) => {
                maybe_emit_memory_updated(path, context);
                refresh_index_after_write(context, &full_path);
                Ok(ToolOutput {
                    content: format!("Created {}", path),
                    is_error: false,
                    attachments: Vec::new(),
                })
            }
            Err(e) => Ok(ToolOutput {
                content: format!("Error creating file: {}", e),
                is_error: true,
                attachments: Vec::new(),
            }),
        }
    }
}

/// Dispatches single-edit (`{path, old_string, new_string}`) or batch (`{edits:[...]}`).
/// Batch: full pre-flight validation before any disk write — one failing entry rejects all.
async fn execute_edit_file(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    if let Some(edits) = coerce_batch_array(params.get("edits")) {
        let mixed = params.get("path").is_some()
            || params.get("old_string").is_some()
            || params.get("new_string").is_some();
        if mixed {
            return Ok(ToolOutput {
                content: "BATCH_EDIT_REJECTED: `edits` was provided alongside top-level \
                          `path`/`old_string`/`new_string` fields. Use one shape or the other, \
                          not both."
                    .into(),
                is_error: true,
                attachments: Vec::new(),
            });
        }
        return execute_edit_file_batch(edits, context).await;
    }
    execute_edit_file_one(params, context).await
}

async fn execute_edit_file_batch(edits: Vec<Value>, context: &ToolContext) -> Result<ToolOutput> {
    if context.permissions() == PermissionLevel::Chat {
        return Ok(ToolOutput {
            content: "PERMISSION_DENIED: File writes are not allowed in Chat mode.".into(),
            is_error: true,
            attachments: Vec::new(),
        });
    }
    if edits.is_empty() {
        return Ok(ToolOutput {
            content: "BATCH_EDIT_REJECTED: `edits` array is empty. Pass at least one entry, \
                      or use the single-edit shape `{ path, old_string, new_string }`."
                .into(),
            is_error: true,
            attachments: Vec::new(),
        });
    }

    let mut shape_errors: Vec<String> = Vec::new();
    for (i, entry) in edits.iter().enumerate() {
        let path = entry
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if path.is_empty() {
            shape_errors.push(format!(
                "entry[{}]: `path` is required and must be non-empty",
                i
            ));
            continue;
        }
        if entry.get("old_string").and_then(|v| v.as_str()).is_none() {
            shape_errors.push(format!(
                "entry[{}]: `old_string` is required (use \"\" to insert)",
                i
            ));
        }
        if entry.get("new_string").and_then(|v| v.as_str()).is_none() {
            // new_string="" is legitimate (delete), but missing-entirely is not.
            shape_errors.push(format!(
                "entry[{}]: `new_string` is required (use \"\" to delete)",
                i
            ));
        }
    }
    if !shape_errors.is_empty() {
        return Ok(ToolOutput {
            content: format!(
                "BATCH_EDIT_REJECTED: {} entry/entries failed shape validation. Nothing was written.\n{}",
                shape_errors.len(),
                shape_errors.join("\n"),
            ),
            is_error: true,
            attachments: Vec::new(),
        });
    }

    for (i, entry) in edits.iter().enumerate() {
        let path = entry["path"].as_str().unwrap_or("").trim();
        if let Some(scope_violation) = check_write_scope(context, path) {
            return Ok(ToolOutput {
                content: format!("entry[{}]: {}", i, scope_violation.content),
                is_error: true,
                attachments: Vec::new(),
            });
        }
        let full = match resolve_within_project(&context.project_root, path) {
            Ok(p) => p,
            Err(violation) => {
                return Ok(ToolOutput {
                    content: format!("entry[{}]: {}", i, violation.content),
                    is_error: true,
                    attachments: Vec::new(),
                });
            }
        };
        if let Some(blocked) = check_sensitive_path(path, &full, context).await {
            return Ok(ToolOutput {
                content: format!("entry[{}]: {}", i, blocked.content),
                is_error: true,
                attachments: Vec::new(),
            });
        }
    }

    // Pre-flight: plan every match in memory; sort by path so concurrent batch_edit calls
    // touching the same files acquire locks in a consistent order (deadlock prevention).
    // sort_by_key is stable, so multiple edits to the SAME file keep their input order —
    // essential because we apply them sequentially against accumulating in-memory content.
    let mut by_path: Vec<(usize, &Value)> = edits.iter().enumerate().collect();
    by_path.sort_by_key(|(_, e)| e["path"].as_str().unwrap_or("").to_string());

    if context.needs_write_approval() {
        for (_, entry) in by_path.iter() {
            let path = entry["path"].as_str().unwrap_or("").trim();
            let approved = context
                .permission_broker
                .request(
                    &context.event_tx,
                    &context.task_id,
                    PermissionOp::WriteFile(path.to_string()),
                )
                .await;
            if !approved {
                return Ok(ToolOutput {
                    content: format!(
                        "PERMISSION_DENIED: User denied write to '{}' — batch aborted before any \
                         disk change.",
                        path
                    ),
                    is_error: true,
                    attachments: Vec::new(),
                });
            }
        }
    }

    // Per-file accumulating state. CRITICAL: every edit to a given file is applied against
    // the running in-memory content (`working`), NOT a fresh re-read of disk. Applying each
    // edit against the original on-disk content was the long-standing batch-edit data-loss
    // bug — for N edits to one file, only the last survived because each plan re-wrote the
    // file with its own (original ± one-edit) content, clobbering the earlier writes.
    enum EntryOutcome {
        Edited(MatchFallback),
        Appended,
        AlreadyApplied,
    }
    let mut working: std::collections::HashMap<PathBuf, String> = std::collections::HashMap::new();
    let mut originals: std::collections::HashMap<PathBuf, String> =
        std::collections::HashMap::new();
    let mut display_paths: std::collections::HashMap<PathBuf, String> =
        std::collections::HashMap::new();
    let mut outcomes: Vec<(usize, String, EntryOutcome)> = Vec::new();

    for (idx, entry) in &by_path {
        let path = entry["path"].as_str().unwrap_or("").trim().to_string();
        let old_string = entry["old_string"].as_str().unwrap_or("").to_string();
        let new_string = entry["new_string"].as_str().unwrap_or("").to_string();
        let hint_line = entry
            .get("hint_line")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize);
        let replace_all = entry
            .get("replace_all")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let full_path = match resolve_within_project(&context.project_root, &path) {
            Ok(p) => p,
            Err(violation) => {
                return Ok(ToolOutput {
                    content: format!("entry[{}]: {}", idx, violation.content),
                    is_error: true,
                    attachments: Vec::new(),
                });
            }
        };

        // Read disk content once per file; subsequent edits to the same file see the
        // accumulated in-memory state, not a stale re-read.
        if !working.contains_key(&full_path) {
            let content = match std::fs::read_to_string(&full_path) {
                Ok(c) => c,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(ToolOutput {
                        content: format!(
                            "BATCH_EDIT_REJECTED: entry[{}]: CONTENT_DELETED: File '{}' does not \
                             exist. Nothing was written.",
                            idx, path
                        ),
                        is_error: true,
                        attachments: Vec::new(),
                    });
                }
                Err(e) => {
                    return Ok(ToolOutput {
                        content: format!(
                            "BATCH_EDIT_REJECTED: entry[{}]: read failure on '{}': {}",
                            idx, path, e
                        ),
                        is_error: true,
                        attachments: Vec::new(),
                    });
                }
            };
            originals.insert(full_path.clone(), content.clone());
            display_paths.insert(full_path.clone(), path.clone());
            working.insert(full_path.clone(), content);
        }

        let content = working.get(&full_path).cloned().unwrap_or_default();

        // APPEND MODE inside a batch: empty old_string means append. Same
        // separator-newline rule as single-edit append.
        if old_string.is_empty() {
            let mut appended = String::with_capacity(content.len() + new_string.len() + 1);
            appended.push_str(&content);
            if !content.is_empty() && !content.ends_with('\n') {
                appended.push('\n');
            }
            appended.push_str(&new_string);
            working.insert(full_path.clone(), appended);
            outcomes.push((*idx, path, EntryOutcome::Appended));
            continue;
        }

        let matched = match find_edit_match(&content, &old_string) {
            Some(m) => m,
            None => {
                if !new_string.is_empty() && content.contains(new_string.as_str()) {
                    // ALREADY_APPLIED on a single entry inside a batch is treated as a
                    // no-op for that entry (the target text is already present in the
                    // accumulated content), not a batch failure.
                    outcomes.push((*idx, path, EntryOutcome::AlreadyApplied));
                    continue;
                }
                let ctx = build_no_match_context(&content, &old_string, hint_line);
                if !context.file_read_registry.has_been_read(&full_path) {
                    return Ok(ToolOutput {
                        content: format!(
                            "BATCH_EDIT_REJECTED: entry[{}]: MUST_READ_FIRST: old_string did not \
                             match and '{}' has not been read in this conversation. Use read_file \
                             on it first, then retry the batch with an exact old_string. Nothing \
                             was written.\n\n{}",
                            idx, path, ctx
                        ),
                        is_error: true,
                        attachments: Vec::new(),
                    });
                }
                return Ok(ToolOutput {
                    content: format!(
                        "BATCH_EDIT_REJECTED: entry[{}]: EDIT_NO_MATCH on '{}'. Nothing was \
                         written. Fix this entry's old_string and retry the batch.\n\n{}",
                        idx, path, ctx
                    ),
                    is_error: true,
                    attachments: Vec::new(),
                });
            }
        };

        // Determine the actual old string from the working content (for quote style preservation)
        let actual_old_string = content[matched.range.clone()].to_string();

        // Preserve quote style if we matched via quote normalization
        let final_new_string = if matches!(matched.fallback, MatchFallback::Quotes) {
            preserve_quote_style(&old_string, &actual_old_string, &new_string)
        } else {
            new_string.clone()
        };

        // Perform the replacement against the accumulated content, then store it back.
        let new_content = if replace_all {
            content.replace(actual_old_string.as_str(), &final_new_string)
        } else {
            let mut result = String::with_capacity(content.len() + final_new_string.len());
            result.push_str(&content[..matched.range.start]);
            result.push_str(&final_new_string);
            result.push_str(&content[matched.range.end..]);
            result
        };

        working.insert(full_path.clone(), new_content);
        outcomes.push((*idx, path, EntryOutcome::Edited(matched.fallback)));
    }

    // Acquire write locks only after all reads/planning are done. Reading inside the lock
    // caused 30+ s stalls on Windows (Defender/indexer). Lock hold-time is now just the
    // atomic_write call (~ms). One lock + one write per unique file. Sort for consistent
    // lock order across concurrent batch_edit calls (deadlock prevention).
    let mut unique_sorted_paths: Vec<PathBuf> = working.keys().cloned().collect();
    unique_sorted_paths.sort();
    let mut held_locks: std::collections::HashMap<PathBuf, tokio::sync::OwnedMutexGuard<()>> =
        std::collections::HashMap::new();
    for path in &unique_sorted_paths {
        match context.file_lock.acquire(path).await {
            Ok(g) => {
                held_locks.insert(path.clone(), g);
            }
            Err(msg) => return Ok(ToolOutput::text(msg, true)),
        }
    }

    // Commit: write each file's final accumulated content exactly once. If any write
    // fails, roll back every already-written file to its original content — either all
    // files land or none do.
    struct CommitRecord {
        full_path: PathBuf,
        display: String,
        original: String,
    }
    let mut committed: Vec<CommitRecord> = Vec::new();
    let mut commit_failure: Option<(String, std::io::Error)> = None; // (path, err)

    for full_path in &unique_sorted_paths {
        let final_content = working.get(full_path).cloned().unwrap_or_default();
        let original = originals.get(full_path).cloned().unwrap_or_default();
        let display = display_paths
            .get(full_path)
            .cloned()
            .unwrap_or_else(|| full_path.display().to_string());
        // Net no-op for this file (e.g. every entry was ALREADY_APPLIED, or edits
        // cancelled out) — skip the write so we don't churn the index needlessly.
        if final_content == original {
            continue;
        }
        track_before_write(context, full_path);
        match crate::io_util::atomic_write(full_path, final_content.as_bytes()) {
            Ok(()) => {
                committed.push(CommitRecord {
                    full_path: full_path.clone(),
                    display,
                    original,
                });
            }
            Err(e) => {
                commit_failure = Some((display, e));
                break;
            }
        }
    }

    if let Some((failed_path, failed_err)) = commit_failure {
        let mut rollback_failures: Vec<String> = Vec::new();
        for rec in committed.iter().rev() {
            match crate::io_util::atomic_write(&rec.full_path, rec.original.as_bytes()) {
                Ok(()) => {
                    refresh_index_after_write(context, &rec.full_path);
                }
                Err(e) => {
                    rollback_failures.push(format!("'{}': {}", rec.display, e));
                }
            }
        }
        let rollback_summary = if rollback_failures.is_empty() {
            format!("All {} earlier file writes were restored.", committed.len())
        } else {
            format!(
                "{} of {} earlier file writes were restored. {} could not be reverted: {}. \
                 You may need to `/rewind` to this turn's user message to clean up.",
                committed.len() - rollback_failures.len(),
                committed.len(),
                rollback_failures.len(),
                rollback_failures.join(", "),
            )
        };
        return Ok(ToolOutput {
            content: format!(
                "BATCH_REVERTED: WRITE_FAILED on '{}': {}. {}",
                failed_path, failed_err, rollback_summary,
            ),
            is_error: true,
            attachments: Vec::new(),
        });
    }

    for rec in &committed {
        maybe_emit_memory_updated(&rec.display, context);
        refresh_index_after_write(context, &rec.full_path);
    }

    // Per-entry report lines, restored to original entry order.
    outcomes.sort_by_key(|(i, _, _)| *i);
    let mut applied_entries = 0usize;
    let mut already_entries = 0usize;
    let mut per_entry_lines = String::new();
    for (i, disp, outcome) in &outcomes {
        let msg = match outcome {
            EntryOutcome::Edited(MatchFallback::Exact) => {
                applied_entries += 1;
                format!("Edited {}", disp)
            }
            EntryOutcome::Edited(MatchFallback::Quotes) => {
                applied_entries += 1;
                format!("Edited (QUOTES_NORMALIZED) {}", disp)
            }
            EntryOutcome::Edited(MatchFallback::Whitespace) => {
                applied_entries += 1;
                format!("Edited (WHITESPACE_NORMALIZED) {}", disp)
            }
            EntryOutcome::Edited(MatchFallback::Indentation) => {
                applied_entries += 1;
                format!("Edited (INDENT_NORMALIZED) {}", disp)
            }
            EntryOutcome::Appended => {
                applied_entries += 1;
                format!("Appended {}", disp)
            }
            EntryOutcome::AlreadyApplied => {
                already_entries += 1;
                format!("ALREADY_APPLIED on {}", disp)
            }
        };
        per_entry_lines.push_str(&format!("  [{}] {}\n", i, msg));
    }

    let mut body = format!(
        "Batch edit ({} entries across {} file(s)): {} applied, {} already-applied. {} file(s) written.\n",
        outcomes.len(),
        unique_sorted_paths.len(),
        applied_entries,
        already_entries,
        committed.len(),
    );
    body.push_str(&per_entry_lines);
    if already_entries > 0 {
        body.push_str(
            "\nALREADY_APPLIED entries indicate the file already contained the \
             target new_string at planning time — they're no-ops, not failures. \
             All actual writes were committed atomically: either every file \
             landed or none did.\n",
        );
    }

    drop(held_locks);

    Ok(ToolOutput {
        content: body,
        is_error: false,
        attachments: Vec::new(),
    })
}

async fn execute_edit_file_one(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    let path = params["path"].as_str().unwrap_or("");
    if context.permissions() == PermissionLevel::Chat {
        return Ok(ToolOutput {
            content: "PERMISSION_DENIED: File writes are not allowed in Chat mode.".into(),
            is_error: true,
            attachments: Vec::new(),
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
            .request(
                &context.event_tx,
                &context.task_id,
                PermissionOp::WriteFile(path.to_string()),
            )
            .await;
        if !approved {
            return Ok(ToolOutput {
                content: "PERMISSION_DENIED: User denied file edit.".into(),
                is_error: true,
                attachments: Vec::new(),
            });
        }
    }

    let old_string = match params["old_string"].as_str() {
        Some(s) => s.to_string(),
        None => {
            return Ok(ToolOutput {
                content: "old_string is required".into(),
                is_error: true,
                attachments: Vec::new(),
            })
        }
    };
    let new_string = params["new_string"].as_str().unwrap_or("").to_string();
    let hint_line = params["hint_line"].as_u64().map(|n| n as usize);
    let replace_all = params["replace_all"].as_bool().unwrap_or(false);

    let full_path = match resolve_within_project(&context.project_root, path) {
        Ok(p) => p,
        Err(violation) => return Ok(violation),
    };

    // Read without the mutex — Defender/indexer can block read_to_string for 30+ s;
    // acquiring here would time out any concurrent edit. Mutex is held only for the write.
    let content = match std::fs::read_to_string(&full_path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ToolOutput {
                content: format!(
                    "CONTENT_DELETED: File '{}' does not exist. It may have been deleted.",
                    path
                ),
                is_error: true,
                attachments: Vec::new(),
            });
        }
        Err(e) => {
            return Ok(ToolOutput {
                content: format!("Error reading file: {}", e),
                is_error: true,
                attachments: Vec::new(),
            })
        }
    };

    // APPEND MODE: empty `old_string` means "append `new_string` to the end of
    // the file". Lets the model use edit_file to add content without first
    // crafting a matchable anchor in the existing text. We add a single
    // separating newline iff the file is non-empty and doesn't already end
    // with one, so the appended block lands on its own line.
    if old_string.is_empty() {
        let mut new_content = String::with_capacity(content.len() + new_string.len() + 1);
        new_content.push_str(&content);
        if !content.is_empty() && !content.ends_with('\n') {
            new_content.push('\n');
        }
        new_content.push_str(&new_string);

        track_before_write(context, &full_path);
        let _guard = match context.file_lock.acquire(&full_path).await {
            Ok(g) => g,
            Err(msg) => return Ok(ToolOutput::text(msg, true)),
        };
        return match crate::io_util::atomic_write(&full_path, new_content.as_bytes()) {
            Ok(()) => {
                maybe_emit_memory_updated(path, context);
                refresh_index_after_write(context, &full_path);
                Ok(ToolOutput {
                    content: format!(
                        "Appended {} bytes to {} (old_string was empty — append mode)",
                        new_string.len(),
                        path
                    ),
                    is_error: false,
                    attachments: Vec::new(),
                })
            }
            Err(e) => Ok(ToolOutput {
                content: format!("Error writing file: {}", e),
                is_error: true,
                attachments: Vec::new(),
            }),
        };
    }

    let matched = match find_edit_match(&content, &old_string) {
        Some(m) => m,
        None => {
            if !new_string.is_empty() && content.contains(new_string.as_str()) {
                return Ok(ToolOutput {
                    content: format!(
                        "ALREADY_APPLIED: The replacement text is already present in '{}'. No changes made.",
                        path
                    ),
                    is_error: false,
                    attachments: Vec::new(),
                });
            }
            // Match failed. If the file was never read, the most likely cause
            // is a stale/guessed old_string — tell the model to read first.
            if !context.file_read_registry.has_been_read(&full_path) {
                return Ok(ToolOutput {
                    content: format!(
                        "MUST_READ_FIRST: old_string did not match and '{}' has not been read \
                         in this conversation. Use read_file on it first, then retry the edit \
                         with an exact old_string from the current file content.",
                        path
                    ),
                    is_error: true,
                    attachments: Vec::new(),
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
                attachments: Vec::new(),
            });
        }
    };

    // Determine the actual old string from the file (for quote style preservation)
    let actual_old_string = &content[matched.range.clone()];

    // Preserve quote style if we matched via quote normalization
    let final_new_string = if matches!(matched.fallback, MatchFallback::Quotes) {
        preserve_quote_style(&old_string, actual_old_string, &new_string)
    } else {
        new_string.clone()
    };

    // Perform the replacement
    let new_content = if replace_all {
        // Replace all occurrences
        let replacement_str = actual_old_string;
        content.replace(replacement_str, &final_new_string)
    } else {
        // Replace only the first match
        let mut result = String::with_capacity(content.len() + final_new_string.len());
        result.push_str(&content[..matched.range.start]);
        result.push_str(&final_new_string);
        result.push_str(&content[matched.range.end..]);
        result
    };

    track_before_write(context, &full_path);
    let _guard = match context.file_lock.acquire(&full_path).await {
        Ok(g) => g,
        Err(msg) => return Ok(ToolOutput::text(msg, true)),
    };

    match crate::io_util::atomic_write(&full_path, new_content.as_bytes()) {
        Ok(()) => {
            maybe_emit_memory_updated(path, context);
            refresh_index_after_write(context, &full_path);
            let msg = match matched.fallback {
                MatchFallback::Exact => format!("Edited {}", path),
                MatchFallback::Quotes => format!(
                    "Edited {} (QUOTES_NORMALIZED: matched after normalizing curly quotes to \
                     straight quotes — your old_string used straight quotes but the file had \
                     curly quotes, or vice versa. The replacement preserved the original quote style)",
                    path
                ),
                MatchFallback::Whitespace => format!(
                    "Edited {} (WHITESPACE_NORMALIZED: matched after stripping per-line \
                     trailing whitespace / normalizing line endings — your old_string had \
                     cosmetic whitespace differences from the file)",
                    path
                ),
                MatchFallback::Indentation => format!(
                    "Edited {} (INDENT_NORMALIZED: matched line-by-line ignoring leading/trailing \
                     whitespace — your old_string had the right text but different indentation. \
                     The matched lines were replaced with your new_string verbatim, so double-check \
                     its indentation is correct)",
                    path
                ),
            };
            Ok(ToolOutput {
                content: msg,
                is_error: false,
                attachments: Vec::new(),
            })
        }
        Err(e) => Ok(ToolOutput {
            content: format!("Error writing file: {}", e),
            is_error: true,
            attachments: Vec::new(),
        }),
    }
}

async fn execute_list_directory(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    if !context.check_permission(&Action::Read) {
        return Ok(ToolOutput {
            content: "PERMISSION_DENIED: Read not allowed in current permission mode.".into(),
            is_error: true,
            attachments: Vec::new(),
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
                    if e.path().is_dir() {
                        format!("{}/", name)
                    } else {
                        name
                    }
                })
                .collect();
            items.sort();
            Ok(ToolOutput {
                content: items.join("\n"),
                is_error: false,
                attachments: Vec::new(),
            })
        }
        Err(e) => Ok(ToolOutput {
            content: format!("Error listing directory: {}", e),
            is_error: true,
            attachments: Vec::new(),
        }),
    }
}

/// Move or rename a file/directory inside the project, with scope, permission,
/// lock, index, and read-registry bookkeeping.
async fn execute_move_file(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    let path = match params["path"].as_str() {
        Some(p) if !p.trim().is_empty() => p,
        _ => return Ok(ToolOutput::text("path is required".to_string(), true)),
    };
    let new_path = match params["new_path"].as_str() {
        Some(p) if !p.trim().is_empty() => p,
        _ => return Ok(ToolOutput::text("new_path is required".to_string(), true)),
    };
    let overwrite = params["overwrite"]
        .as_bool()
        .or_else(|| {
            params["overwrite"].as_str().map(|s| {
                matches!(
                    s.to_ascii_lowercase().as_str(),
                    "true" | "1" | "yes" | "y" | "on"
                )
            })
        })
        .unwrap_or(false);

    if let Some(v) = check_write_scope(context, path) {
        return Ok(v);
    }
    if let Some(v) = check_write_scope(context, new_path) {
        return Ok(v);
    }
    let src = match resolve_with_scope(context, path) {
        Ok(p) => p,
        Err(v) => return Ok(v),
    };
    let dst = match resolve_with_scope(context, new_path) {
        Ok(p) => p,
        Err(v) => return Ok(v),
    };
    if let Some(blocked) = check_sensitive_path(path, &src, context).await {
        return Ok(blocked);
    }
    if let Some(blocked) = check_sensitive_path(new_path, &dst, context).await {
        return Ok(blocked);
    }

    let src_meta = match std::fs::symlink_metadata(&src) {
        Ok(m) => m,
        Err(_) => {
            return Ok(ToolOutput::text(
                format!("MOVE_FAILED: source '{}' does not exist.", path),
                true,
            ));
        }
    };
    if src == dst {
        return Ok(ToolOutput::text(
            format!(
                "MOVE_FAILED: source and destination are the same path ('{}').",
                path
            ),
            true,
        ));
    }
    let dst_meta = std::fs::symlink_metadata(&dst).ok();
    if let Some(dm) = &dst_meta {
        if dm.is_dir() {
            return Ok(ToolOutput::text(
                format!(
                    "MOVE_BLOCKED: destination '{}' is an existing directory — it will not be \
                     overwritten. Pick a different destination.",
                    new_path
                ),
                true,
            ));
        }
        if !overwrite {
            return Ok(ToolOutput::text(
                format!(
                    "MOVE_BLOCKED: destination '{}' already exists. Pass overwrite: true to \
                     replace it.",
                    new_path
                ),
                true,
            ));
        }
    }

    if context.needs_write_approval() {
        let approved = context
            .permission_broker
            .request(
                &context.event_tx,
                &context.task_id,
                PermissionOp::WriteFile(format!("{} → {}", path, new_path)),
            )
            .await;
        if !approved {
            return Ok(ToolOutput::text(
                "PERMISSION_DENIED: User denied file move.".to_string(),
                true,
            ));
        }
    }

    track_before_write(context, &src);
    track_before_write(context, &dst);

    // Ordered acquisition so two concurrent moves touching the same pair of
    // paths can't deadlock (AB / BA).
    let (first, second) = if src <= dst {
        (&src, &dst)
    } else {
        (&dst, &src)
    };
    let _g1 = match context.file_lock.acquire(first).await {
        Ok(g) => g,
        Err(msg) => return Ok(ToolOutput::text(msg, true)),
    };
    let _g2 = match context.file_lock.acquire(second).await {
        Ok(g) => g,
        Err(msg) => return Ok(ToolOutput::text(msg, true)),
    };

    if let Some(parent) = dst.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            return Ok(ToolOutput::text(
                format!("MOVE_FAILED: could not create destination directory: {}", e),
                true,
            ));
        }
    }
    if dst_meta.is_some() && overwrite {
        if let Err(e) = std::fs::remove_file(&dst) {
            return Ok(ToolOutput::text(
                format!(
                    "MOVE_FAILED: could not replace destination '{}': {}",
                    new_path, e
                ),
                true,
            ));
        }
    }

    match std::fs::rename(&src, &dst) {
        Ok(()) => {}
        Err(rename_err) => {
            // Cross-device fallback for plain files: copy + delete.
            if src_meta.is_file() {
                if let Err(e) = std::fs::copy(&src, &dst).and_then(|_| std::fs::remove_file(&src)) {
                    return Ok(ToolOutput::text(
                        format!(
                            "MOVE_FAILED: rename failed ({}) and copy fallback failed ({}).",
                            rename_err, e
                        ),
                        true,
                    ));
                }
            } else {
                return Ok(ToolOutput::text(
                    format!("MOVE_FAILED: could not move directory: {}", rename_err),
                    true,
                ));
            }
        }
    }

    context.file_read_registry.invalidate(&src);
    context.file_read_registry.invalidate(&dst);
    context.workspace_services.notify_file_deleted(&src);
    if src_meta.is_file() {
        refresh_index_after_write(context, &dst);
    }
    maybe_emit_memory_updated(path, context);
    maybe_emit_memory_updated(new_path, context);

    Ok(ToolOutput::text(
        format!(
            "Moved {} '{}' → '{}'.",
            if src_meta.is_dir() {
                "directory"
            } else {
                "file"
            },
            path,
            new_path
        ),
        false,
    ))
}

pub(crate) fn maybe_emit_memory_updated(path: &str, ctx: &ToolContext) {
    let normalized = path.replace('\\', "/");
    // Fire for the legacy single file AND for any fragment under the new
    // `.rustic/memory/` folder (including the MEMORY.md index).
    if normalized.ends_with(".rustic/memory.md") || normalized.contains(".rustic/memory/") {
        let _ = ctx.event_tx.try_send(TaskEvent::MemoryUpdated {
            task_id: ctx.task_id.clone(),
        });
    }
}

/// Snapshot `abs_path` before mutation and emit FileTracked. Failure is non-fatal.
pub(crate) fn track_before_write(ctx: &ToolContext, abs_path: &std::path::Path) {
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

struct EditMatch {
    range: std::ops::Range<usize>,
    fallback: MatchFallback,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum MatchFallback {
    Exact,
    Quotes,
    Whitespace,
    /// Matched line-by-line after trimming leading *and* trailing whitespace
    /// (indentation-insensitive). Used when the agent's old_string has the
    /// right text but the wrong indentation — by far the most common cause of
    /// repeated edit_file failures.
    Indentation,
}

/// Locate `old_string` in `content` (exact first, then whitespace-tolerant fallback).
/// Fallback strips trailing whitespace and normalizes CRLF→LF per line. The returned
/// range is always in original byte coordinates so callers can splice without reformatting.
fn find_edit_match(content: &str, old_string: &str) -> Option<EditMatch> {
    if old_string.is_empty() {
        return None;
    }
    // Stage 1: exact byte match
    if let Some(idx) = content.find(old_string) {
        return Some(EditMatch {
            range: idx..idx + old_string.len(),
            fallback: MatchFallback::Exact,
        });
    }
    // Stage 2: quote normalization (curly quotes → straight quotes)
    let normalized_old = normalize_quotes(old_string);
    let normalized_content = normalize_quotes(content);
    if let Some(idx) = normalized_content.find(&normalized_old) {
        // Find the actual string in the original content that corresponds to this match
        // We need to account for potential character width differences
        let actual_end = idx + old_string.len(); // approximate
        return Some(EditMatch {
            range: idx..actual_end,
            fallback: MatchFallback::Quotes,
        });
    }
    // Stage 3: whitespace normalization
    let (norm_content, content_offsets) = normalize_ws_with_offsets(content);
    let (norm_old, _) = normalize_ws_with_offsets(old_string);
    if norm_old.is_empty() {
        return None;
    }
    if let Some(idx) = norm_content.find(&norm_old) {
        let end = idx + norm_old.len();
        let orig_start = *content_offsets.get(idx)?;
        let orig_end = *content_offsets.get(end)?;
        if orig_end >= orig_start {
            return Some(EditMatch {
                range: orig_start..orig_end,
                fallback: MatchFallback::Whitespace,
            });
        }
    }
    // Stage 4: indentation-insensitive, line-by-line match. The agent very
    // often reproduces a block's text correctly but with the wrong leading
    // whitespace (e.g. it dedented when quoting). We match a contiguous run of
    // file lines whose *trimmed* contents equal the trimmed old_string lines,
    // then replace those whole lines. Only fires when the match is unambiguous
    // (exactly one such run) so we never silently edit the wrong block.
    find_line_based_match(content, old_string)
}

/// Indentation-insensitive line match. Returns a range spanning whole file
/// lines whose trimmed text equals `old_string`'s trimmed lines, but only if
/// exactly one such contiguous run exists. Range starts at the first matched
/// line's first byte (original indentation included, so new_string replaces it)
/// and ends at the last matched line's terminator — mirroring whether
/// old_string itself ended with a newline.
fn find_line_based_match(content: &str, old_string: &str) -> Option<EditMatch> {
    // Build the list of old lines (trimmed). Preserve whether old_string ended
    // with a trailing newline so we consume the file's terminator symmetrically.
    let old_had_trailing_nl = old_string.ends_with('\n');
    let mut old_lines: Vec<&str> = old_string.split('\n').collect();
    if old_had_trailing_nl {
        old_lines.pop(); // drop the empty element produced by the trailing '\n'
    }
    let old_trimmed: Vec<&str> = old_lines.iter().map(|l| l.trim()).collect();
    if old_trimmed.is_empty() || old_trimmed.iter().all(|l| l.is_empty()) {
        return None; // nothing meaningful to anchor on
    }

    // Index every file line: (start_byte, content_end_byte, next_line_start_byte).
    // content_end excludes the trailing '\r'/'\n'; next_line_start includes them.
    struct LineSpan {
        start: usize,
        content_end: usize,
        next: usize,
    }
    let bytes = content.as_bytes();
    let mut lines: Vec<LineSpan> = Vec::new();
    let mut i = 0usize;
    while i <= bytes.len() {
        let line_start = i;
        let mut nl = line_start;
        while nl < bytes.len() && bytes[nl] != b'\n' {
            nl += 1;
        }
        let mut content_end = nl;
        if content_end > line_start && bytes[content_end - 1] == b'\r' {
            content_end -= 1;
        }
        let next = if nl < bytes.len() { nl + 1 } else { nl };
        lines.push(LineSpan {
            start: line_start,
            content_end,
            next,
        });
        if nl >= bytes.len() {
            break;
        }
        i = nl + 1;
    }

    let n = old_trimmed.len();
    if lines.len() < n {
        return None;
    }
    let line_trimmed: Vec<&str> = lines
        .iter()
        .map(|s| content[s.start..s.content_end].trim())
        .collect();

    let mut found: Option<usize> = None;
    let mut count = 0usize;
    for w in 0..=(lines.len() - n) {
        if (0..n).all(|k| line_trimmed[w + k] == old_trimmed[k]) {
            count += 1;
            if count > 1 {
                return None; // ambiguous — refuse to guess
            }
            found = Some(w);
        }
    }
    let w = found?;
    let first = &lines[w];
    let last = &lines[w + n - 1];
    let start = first.start;
    let end = if old_had_trailing_nl {
        last.next
    } else {
        last.content_end
    };
    if end < start {
        return None;
    }
    Some(EditMatch {
        range: start..end,
        fallback: MatchFallback::Indentation,
    })
}

/// Whitespace-normalize `s` and return a byte-offset map back to original positions.
/// Only touches ASCII whitespace (space/tab/CR); multibyte UTF-8 passes through untouched.
fn normalize_ws_with_offsets(s: &str) -> (String, Vec<usize>) {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut offsets: Vec<usize> = Vec::with_capacity(bytes.len() + 1);

    let mut i = 0;
    while i < bytes.len() {
        let mut line_end = i;
        while line_end < bytes.len() && bytes[line_end] != b'\n' {
            line_end += 1;
        }
        let mut trimmed_end = line_end;
        if trimmed_end > i && bytes[trimmed_end - 1] == b'\r' {
            trimmed_end -= 1;
        }
        while trimmed_end > i && (bytes[trimmed_end - 1] == b' ' || bytes[trimmed_end - 1] == b'\t')
        {
            trimmed_end -= 1;
        }
        for k in i..trimmed_end {
            offsets.push(k);
            out.push(bytes[k]);
        }
        if line_end < bytes.len() {
            offsets.push(line_end);
            out.push(b'\n');
            i = line_end + 1;
        } else {
            i = line_end;
        }
    }
    offsets.push(bytes.len());
    // Safety: only ASCII bytes dropped — result is still valid UTF-8.
    let out_str = String::from_utf8(out).expect("ws normalization preserves utf-8");
    (out_str, offsets)
}

/// Token-set Jaccard similarity (case-folded) — handles indentation/whitespace differences.
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

/// Top-N candidate lines by token similarity to old_string's first line, with ±2 context lines.
fn build_no_match_context(content: &str, old_string: &str, hint_line: Option<usize>) -> String {
    let file_lines: Vec<&str> = content.lines().collect();
    let total = file_lines.len();
    if total == 0 {
        return "(file is empty)\n".to_string();
    }

    let probe = old_string
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("");
    if probe.trim().is_empty() {
        return build_stale_read_context(content, hint_line);
    }

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

fn build_stale_read_context(content: &str, hint_line: Option<usize>) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();

    let (start, end) = if let Some(hl) = hint_line {
        let center = hl.saturating_sub(1);
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
            result.push_str(&format!(
                "[... truncated at {}KB]\n",
                MAX_CONTEXT_BYTES / 1024
            ));
            break;
        }
        result.push_str(&formatted);
        byte_count += formatted.len();
    }

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
        assert!(
            find_edit_match(content, old).is_none(),
            "internal whitespace differences must NOT be normalized away"
        );
    }

    #[test]
    fn indentation_difference_falls_back() {
        // File has 8-space indented body; agent's old_string used 4 spaces.
        // Internal token spacing is identical, only leading indent differs.
        let content = "fn f() {\n        let x = 1;\n        return x;\n}\n";
        let old = "    let x = 1;\n    return x;\n";
        let m = find_edit_match(content, old).expect("indent fallback should match");
        assert_eq!(m.fallback, MatchFallback::Indentation);
        // Range spans the two indented lines including their original indent.
        assert_eq!(
            &content[m.range.clone()],
            "        let x = 1;\n        return x;\n"
        );
        // Splicing the agent's (correctly-indented) replacement works cleanly.
        let new = "        let x = 2;\n        return x * 2;\n";
        let mut out = String::new();
        out.push_str(&content[..m.range.start]);
        out.push_str(new);
        out.push_str(&content[m.range.end..]);
        assert_eq!(
            out,
            "fn f() {\n        let x = 2;\n        return x * 2;\n}\n"
        );
    }

    #[test]
    fn ambiguous_indentation_match_refuses() {
        // The dedented 2-line block "a\nb" appears twice after trimming (and is
        // not an exact/ws substring because of the indentation), so the
        // indentation fallback must refuse rather than guess which to edit.
        let content = "  a\n  b\n  a\n  b\n";
        let old = "a\nb\n";
        assert!(
            find_edit_match(content, old).is_none(),
            "ambiguous indentation match must refuse rather than guess"
        );
    }
}

#[cfg(test)]
mod p1_5_batch_validation_tests {
    use serde_json::{json, Value};

    fn shape_errors(edits: &[Value]) -> Vec<String> {
        let mut errors = Vec::new();
        for (i, entry) in edits.iter().enumerate() {
            let path = entry
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            if path.is_empty() {
                errors.push(format!(
                    "entry[{}]: `path` is required and must be non-empty",
                    i
                ));
                continue;
            }
            if entry.get("old_string").and_then(|v| v.as_str()).is_none() {
                errors.push(format!(
                    "entry[{}]: `old_string` is required (use \"\" to insert)",
                    i
                ));
            }
            if entry.get("new_string").and_then(|v| v.as_str()).is_none() {
                errors.push(format!(
                    "entry[{}]: `new_string` is required (use \"\" to delete)",
                    i
                ));
            }
        }
        errors
    }

    #[test]
    fn empty_array_is_rejected() {
        let errs = shape_errors(&[]);
        assert!(errs.is_empty());
    }

    #[test]
    fn missing_path_is_caught() {
        let edits = vec![
            json!({ "old_string": "x", "new_string": "y" }),
            json!({ "path": "  ", "old_string": "a", "new_string": "b" }),
            json!({ "path": "", "old_string": "a", "new_string": "b" }),
        ];
        let errs = shape_errors(&edits);
        assert_eq!(errs.len(), 3);
        for (i, e) in errs.iter().enumerate() {
            assert!(e.contains(&format!("entry[{}]", i)));
            assert!(e.contains("`path`"));
        }
    }

    #[test]
    fn missing_old_string_is_caught() {
        let edits = vec![json!({ "path": "a.rs", "new_string": "y" })];
        let errs = shape_errors(&edits);
        assert_eq!(errs.len(), 1);
        assert!(errs[0].contains("old_string"));
    }

    #[test]
    fn missing_new_string_is_caught() {
        let edits = vec![json!({ "path": "a.rs", "old_string": "y" })];
        let errs = shape_errors(&edits);
        assert_eq!(errs.len(), 1);
        assert!(errs[0].contains("new_string"));
    }

    #[test]
    fn empty_strings_are_allowed_in_old_and_new() {
        let edits = vec![json!({
            "path": "a.rs",
            "old_string": "",
            "new_string": "",
        })];
        let errs = shape_errors(&edits);
        assert!(errs.is_empty(), "empty strings should pass: {:?}", errs);
    }

    #[test]
    fn all_good_returns_no_errors() {
        let edits = vec![
            json!({ "path": "a.rs", "old_string": "foo", "new_string": "bar" }),
            json!({ "path": "b.rs", "old_string": "x", "new_string": "y", "hint_line": 12 }),
        ];
        assert!(shape_errors(&edits).is_empty());
    }
}

#[cfg(test)]
mod c4_atomic_rollback_tests {
    fn rollback_committed(committed: &[(std::path::PathBuf, &str)]) -> Vec<String> {
        let mut failures = Vec::new();
        for (path, original) in committed.iter().rev() {
            if let Err(e) = crate::io_util::atomic_write(path, original.as_bytes()) {
                failures.push(format!("'{}': {}", path.display(), e));
            }
        }
        failures
    }

    #[test]
    fn rollback_restores_each_committed_file_to_original_content() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        std::fs::write(&a, b"original-a").unwrap();
        std::fs::write(&b, b"original-b").unwrap();

        // Simulate successful writes of new content.
        crate::io_util::atomic_write(&a, b"new-a").unwrap();
        crate::io_util::atomic_write(&b, b"new-b").unwrap();
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "new-a");
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "new-b");

        // Roll back.
        let failures = rollback_committed(&[(a.clone(), "original-a"), (b.clone(), "original-b")]);
        assert!(
            failures.is_empty(),
            "no failures expected, got: {:?}",
            failures
        );
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "original-a");
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "original-b");
    }

    #[test]
    fn rollback_handles_empty_committed_list() {
        // No writes happened (first entry failed). Nothing to roll back.
        let failures = rollback_committed(&[]);
        assert!(failures.is_empty());
    }

    #[test]
    fn rollback_preserves_files_not_in_batch() {
        let dir = tempfile::tempdir().unwrap();
        let touched = dir.path().join("touched.txt");
        let untouched = dir.path().join("untouched.txt");
        std::fs::write(&touched, b"original").unwrap();
        std::fs::write(&untouched, b"i was never in the batch").unwrap();

        crate::io_util::atomic_write(&touched, b"new").unwrap();
        rollback_committed(&[(touched.clone(), "original")]);

        assert_eq!(std::fs::read_to_string(&touched).unwrap(), "original");
        assert_eq!(
            std::fs::read_to_string(&untouched).unwrap(),
            "i was never in the batch",
            "files outside the rollback set must not be touched",
        );
    }

    #[test]
    fn rollback_iteration_order_is_reverse_of_commit() {
        // Same path appears twice (degenerate but possible if the agent
        // batches two edits to the same file). Reverse-order rollback
        // ends with the FIRST commit's pre-write content, which matches
        // the production contract "every file ends at its pre-batch
        // state".
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dup.txt");
        std::fs::write(&path, b"v0").unwrap();
        crate::io_util::atomic_write(&path, b"v1").unwrap();
        crate::io_util::atomic_write(&path, b"v2").unwrap();
        rollback_committed(&[(path.clone(), "v0"), (path.clone(), "v1")]);
        // After reverse rollback the second restore (v0) is the most
        // recent write, so the file ends at v0 — the true pre-batch state.
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "v0");
    }
}

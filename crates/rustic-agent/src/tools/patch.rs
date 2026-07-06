//! `apply_patch` — apply a unified diff to project files.
//!
//! Contract (wired in `tools/mod.rs`):
//!   - `pub fn definitions() -> Vec<ToolDef>` — the tool's schema.
//!   - `pub async fn execute(params: Value, context: &ToolContext) -> Result<ToolOutput>`
//!
//! The parser and apply logic are pure functions (unit-testable without a
//! `ToolContext`); `execute` wraps them in the shared write pipeline from
//! `file_ops.rs` (`check_write_scope`, `check_sensitive_path`,
//! `resolve_within_project`, `track_before_write`, `refresh_index_after_write`,
//! `maybe_emit_memory_updated`).

use crate::provider::ToolDef;
use crate::task::permissions::PermissionLevel;
use crate::task::PermissionOp;
use crate::tools::file_ops::{
    check_sensitive_path, check_write_scope, maybe_emit_memory_updated, refresh_index_after_write,
    resolve_within_project, track_before_write,
};
use crate::tools::{ToolContext, ToolOutput};
use anyhow::Result;
use serde_json::{json, Value};

pub fn definitions() -> Vec<ToolDef> {
    vec![ToolDef {
        name: "apply_patch".into(),
        description: "Apply a unified diff to project files — the efficient write primitive \
                      for bulk mechanical changes where edit_file old/new pairs would be \
                      wasteful. Accepts standard `--- a/path` / `+++ b/path` / `@@ -l,c +l,c @@` \
                      format; one patch may cover many files.\n\
                      • File creation: `--- /dev/null` → `+++ b/path`. File deletion: \
                        `--- a/path` → `+++ /dev/null`.\n\
                      • `diff --git` / `index` header lines are tolerated but not required; \
                        `\\ No newline at end of file` markers and CRLF are handled; each \
                        target file's dominant line ending is preserved on write.\n\
                      • Hunk placement: exact match at the stated line first, then a \
                        whole-file search for a UNIQUE position matching the hunk's \
                        context+deleted lines (exact, then trailing-whitespace-stripped). \
                        A hunk matching at 0 or 2+ positions fails THAT FILE with a precise \
                        error — the tool never guesses among ambiguous positions. Add more \
                        context lines to disambiguate.\n\
                      • Per-file atomicity: all hunks for a file apply or that file is left \
                        untouched; other files in the patch still proceed. The result lists \
                        per-file outcomes. `PATCH_FAILED` (is_error) only when every file \
                        failed."
            .into(),
        parameters: json!({
            "type": "object",
            "properties": {
                "patch": {
                    "type": "string",
                    "description": "The unified diff to apply. May contain multiple file \
                                    sections. Paths are interpreted relative to the project \
                                    root after `strip` prefix components are removed."
                },
                "strip": {
                    "type": "integer",
                    "description": "Number of leading path components to strip from patch \
                                    paths, like `patch -pN`. Default 1 (turns `a/src/main.rs` \
                                    into `src/main.rs`). Pass 0 if your patch paths are \
                                    already project-relative."
                }
            },
            "required": ["patch"]
        }),
    }]
}

// ─────────────────────────── data model ────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FileOp {
    Create,
    Delete,
    Modify,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum HunkLine {
    Context(String),
    Del(String),
    Add(String),
}

#[derive(Debug, Clone)]
pub(crate) struct Hunk {
    /// 1-indexed start line on the old side (0 allowed for empty-file inserts).
    pub old_start: usize,
    #[allow(dead_code)]
    pub old_count: usize,
    #[allow(dead_code)]
    pub new_start: usize,
    #[allow(dead_code)]
    pub new_count: usize,
    pub lines: Vec<HunkLine>,
}

#[derive(Debug, Clone)]
pub(crate) struct FilePatch {
    /// Project-relative path (after `strip`), forward slashes.
    pub path: String,
    pub op: FileOp,
    pub hunks: Vec<Hunk>,
    /// A `\ No newline at end of file` marker was seen anywhere in this
    /// file's hunks — the patch makes a statement about the file's end.
    pub saw_eof_marker: bool,
    /// The NEW side of the patch ends without a trailing newline (marker
    /// followed an added or context line).
    pub new_no_trailing_newline: bool,
}

// ─────────────────────────── parsing ────────────────────────────

fn strip_components(path: &str, strip: usize) -> String {
    let norm = path.replace('\\', "/");
    let comps: Vec<&str> = norm.split('/').filter(|c| !c.is_empty()).collect();
    if comps.is_empty() {
        return norm;
    }
    // Lenient: never strip the final component away entirely.
    let skip = strip.min(comps.len() - 1);
    comps[skip..].join("/")
}

/// Extract the path from a `---`/`+++` header body: cut at the first tab
/// (timestamp suffix), map `/dev/null` to `None`, apply `strip`.
fn header_path(rest: &str, strip: usize) -> Option<String> {
    let p = rest.split('\t').next().unwrap_or(rest).trim();
    if p == "/dev/null" {
        return None;
    }
    Some(strip_components(p, strip))
}

/// Parse `@@ -l[,c] +l[,c] @@ ...` → (old_start, old_count, new_start, new_count).
fn parse_hunk_header(line: &str) -> Option<(usize, usize, usize, usize)> {
    let rest = line.strip_prefix("@@ ")?;
    let end = rest.find(" @@")?;
    let core = &rest[..end];
    let mut parts = core.split(' ');
    let old = parts.next()?.strip_prefix('-')?;
    let new = parts.next()?.strip_prefix('+')?;
    fn range(s: &str) -> Option<(usize, usize)> {
        match s.split_once(',') {
            Some((a, b)) => Some((a.parse().ok()?, b.parse().ok()?)),
            None => Some((s.parse().ok()?, 1)),
        }
    }
    let (os, oc) = range(old)?;
    let (ns, nc) = range(new)?;
    Some((os, oc, ns, nc))
}

/// Record a `\ No newline at end of file` marker against the file patch,
/// based on which side the immediately preceding hunk line belongs to.
fn record_eof_marker(last_line: Option<&HunkLine>, fp: &mut FilePatch) {
    fp.saw_eof_marker = true;
    match last_line {
        // After an added or context line: the NEW content lacks the newline.
        Some(HunkLine::Add(_)) | Some(HunkLine::Context(_)) => {
            fp.new_no_trailing_newline = true;
        }
        // After a deleted line: only the OLD side lacked it — the new
        // content gains a trailing newline.
        _ => {}
    }
}

/// Parse a (possibly multi-file) unified diff. `strip` = path prefix
/// components to remove, like `patch -p`. Tolerates `diff --git`/`index`/mode
/// header lines, CRLF line endings, and `\ No newline at end of file` markers.
pub(crate) fn parse_unified_diff(patch: &str, strip: usize) -> Result<Vec<FilePatch>, String> {
    // `str::lines()` splits on '\n' and strips a trailing '\r' — CRLF patches
    // parse identically to LF ones.
    let lines: Vec<&str> = patch.lines().collect();
    let mut files: Vec<FilePatch> = Vec::new();
    let mut i = 0usize;

    while i < lines.len() {
        let line = lines[i];
        let Some(old_rest) = line.strip_prefix("--- ") else {
            i += 1; // skip `diff --git`, `index`, mode lines, prose, etc.
            continue;
        };
        let Some(new_line) = lines.get(i + 1) else {
            return Err(
                "malformed patch: `---` header on the last line with no `+++` line after it".into(),
            );
        };
        let Some(new_rest) = new_line.strip_prefix("+++ ") else {
            return Err(format!(
                "malformed patch: `--- {}` is not immediately followed by a `+++` line",
                old_rest
            ));
        };

        let old_path = header_path(old_rest, strip);
        let new_path = header_path(new_rest, strip);
        let (path, op) = match (&old_path, &new_path) {
            (None, None) => {
                return Err("malformed patch: both `---` and `+++` are /dev/null".into())
            }
            (None, Some(p)) => (p.clone(), FileOp::Create),
            (Some(p), None) => (p.clone(), FileOp::Delete),
            (Some(_), Some(p)) => (p.clone(), FileOp::Modify),
        };
        i += 2;

        let mut fp = FilePatch {
            path,
            op,
            hunks: Vec::new(),
            saw_eof_marker: false,
            new_no_trailing_newline: false,
        };

        while i < lines.len() && lines[i].starts_with("@@") {
            let (old_start, old_count, new_start, new_count) = parse_hunk_header(lines[i])
                .ok_or_else(|| format!("malformed hunk header in '{}': {:?}", fp.path, lines[i]))?;
            i += 1;

            let mut hlines: Vec<HunkLine> = Vec::new();
            let (mut old_seen, mut new_seen) = (0usize, 0usize);
            while old_seen < old_count || new_seen < new_count {
                let Some(&l) = lines.get(i) else {
                    return Err(format!(
                        "truncated hunk in '{}': header declared {} old / {} new lines but only {} / {} were present",
                        fp.path, old_count, new_count, old_seen, new_seen
                    ));
                };
                if let Some(rest) = l.strip_prefix('+') {
                    hlines.push(HunkLine::Add(rest.to_string()));
                    new_seen += 1;
                } else if let Some(rest) = l.strip_prefix('-') {
                    hlines.push(HunkLine::Del(rest.to_string()));
                    old_seen += 1;
                } else if let Some(rest) = l.strip_prefix(' ') {
                    hlines.push(HunkLine::Context(rest.to_string()));
                    old_seen += 1;
                    new_seen += 1;
                } else if l.starts_with('\\') {
                    // `\ No newline at end of file` — doesn't count toward
                    // either side.
                    record_eof_marker(hlines.last(), &mut fp);
                } else if l.is_empty() {
                    // Tolerate an empty context line whose leading space was
                    // stripped in transit (common patch mangling).
                    hlines.push(HunkLine::Context(String::new()));
                    old_seen += 1;
                    new_seen += 1;
                } else {
                    return Err(format!(
                        "malformed hunk in '{}': unexpected line {:?} (hunk lines must start with ' ', '-', '+', or '\\')",
                        fp.path, l
                    ));
                }
                i += 1;
            }
            // Marker after the hunk's final counted line.
            if i < lines.len() && lines[i].starts_with('\\') {
                record_eof_marker(hlines.last(), &mut fp);
                i += 1;
            }

            fp.hunks.push(Hunk {
                old_start,
                old_count,
                new_start,
                new_count,
                lines: hlines,
            });
        }

        files.push(fp);
    }

    if files.is_empty() {
        return Err("no `--- old` / `+++ new` file sections found in the patch".into());
    }
    Ok(files)
}

// ─────────────────────────── apply logic (pure) ────────────────────────────

fn block_matches_at(lines: &[String], pos: usize, block: &[&str], ws_tolerant: bool) -> bool {
    if pos + block.len() > lines.len() {
        return false;
    }
    block.iter().enumerate().all(|(j, b)| {
        let a = lines[pos + j].as_str();
        if ws_tolerant {
            a.trim_end() == b.trim_end()
        } else {
            a == *b
        }
    })
}

fn find_block_positions(lines: &[String], block: &[&str], ws_tolerant: bool) -> Vec<usize> {
    if block.is_empty() || block.len() > lines.len() {
        return Vec::new();
    }
    (0..=lines.len() - block.len())
        .filter(|&p| block_matches_at(lines, p, block, ws_tolerant))
        .collect()
}

/// First (expected, found) line pair that differs at `pos`. `found` is
/// `"<end of file>"` when the file is too short.
fn first_mismatch(lines: &[String], pos: usize, block: &[&str]) -> (String, String) {
    for (j, b) in block.iter().enumerate() {
        match lines.get(pos + j) {
            Some(a) if a == b => continue,
            Some(a) => return (b.to_string(), a.clone()),
            None => return (b.to_string(), "<end of file>".to_string()),
        }
    }
    (String::new(), String::new())
}

/// Apply all hunks to `file_lines`. All-or-nothing: any hunk failure returns
/// `Err` and the caller must discard the result. Placement: exact match at
/// the stated line, else a UNIQUE whole-file exact match, else a UNIQUE
/// trailing-whitespace-stripped match; 0 or 2+ candidates → error naming the
/// hunk and the first mismatching context line. Never guesses.
pub(crate) fn apply_hunks(file_lines: &[String], hunks: &[Hunk]) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = file_lines.to_vec();
    // Cumulative line drift from already-applied hunks, so later hunks'
    // stated positions stay aligned.
    let mut delta: isize = 0;

    for (n, h) in hunks.iter().enumerate() {
        let hunk_no = n + 1;
        let old_block: Vec<&str> = h
            .lines
            .iter()
            .filter_map(|l| match l {
                HunkLine::Context(s) | HunkLine::Del(s) => Some(s.as_str()),
                HunkLine::Add(_) => None,
            })
            .collect();
        let new_block: Vec<String> = h
            .lines
            .iter()
            .filter_map(|l| match l {
                HunkLine::Context(s) | HunkLine::Add(s) => Some(s.clone()),
                HunkLine::Del(_) => None,
            })
            .collect();

        let pos = if old_block.is_empty() {
            // Pure insertion (`-N,0`): unified-diff semantics say N is the
            // line AFTER which to insert (0 = top of file). No context to
            // search for — the stated position is all we have; reject
            // out-of-range instead of guessing.
            let stated = h.old_start as isize + delta;
            if stated < 0 || stated as usize > out.len() {
                return Err(format!(
                    "hunk {}: insertion position {} is outside the file ({} lines)",
                    hunk_no,
                    h.old_start,
                    out.len()
                ));
            }
            stated as usize
        } else {
            let stated = h.old_start as isize - 1 + delta;
            if stated >= 0 && block_matches_at(&out, stated as usize, &old_block, false) {
                stated as usize
            } else {
                let exact = find_block_positions(&out, &old_block, false);
                match exact.len() {
                    1 => exact[0],
                    0 => {
                        let ws = find_block_positions(&out, &old_block, true);
                        match ws.len() {
                            1 => ws[0],
                            0 => {
                                let probe = stated.max(0) as usize;
                                let (expected, found) = first_mismatch(&out, probe, &old_block);
                                return Err(format!(
                                    "hunk {}: no position in the file matches the hunk's context/deleted \
                                     lines (tried exact and trailing-whitespace-stripped). At stated line {}: \
                                     expected {:?}, found {:?}",
                                    hunk_no, h.old_start, expected, found
                                ));
                            }
                            k => {
                                return Err(format!(
                                    "hunk {}: whitespace-tolerant match is ambiguous ({} candidate positions) \
                                     — refusing to guess. Add more context lines to make the hunk unique.",
                                    hunk_no, k
                                ));
                            }
                        }
                    }
                    k => {
                        return Err(format!(
                            "hunk {}: context/deleted lines match at {} positions — refusing to guess. \
                             Add more context lines to make the hunk unique.",
                            hunk_no, k
                        ));
                    }
                }
            }
        };

        let old_len = old_block.len();
        let new_len = new_block.len();
        out.splice(pos..pos + old_len, new_block);
        delta += new_len as isize - old_len as isize;
    }

    Ok(out)
}

/// True when CRLF is the file's dominant line ending.
pub(crate) fn detect_crlf(content: &str) -> bool {
    let crlf = content.matches("\r\n").count();
    let lf = content.matches('\n').count();
    crlf > 0 && crlf * 2 >= lf
}

/// Split into lines (line endings removed) + whether a trailing newline existed.
pub(crate) fn split_lines(content: &str) -> (Vec<String>, bool) {
    let trailing = content.ends_with('\n');
    (content.lines().map(String::from).collect(), trailing)
}

pub(crate) fn join_lines(lines: &[String], crlf: bool, trailing_newline: bool) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let eol = if crlf { "\r\n" } else { "\n" };
    let mut s = lines.join(eol);
    if trailing_newline {
        s.push_str(eol);
    }
    s
}

/// Apply a Modify-op file patch to existing content, preserving the file's
/// dominant line ending and honoring `\ No newline at end of file` markers.
pub(crate) fn apply_to_content(content: &str, fp: &FilePatch) -> Result<String, String> {
    let crlf = detect_crlf(content);
    let (lines, had_trailing) = split_lines(content);
    let new_lines = apply_hunks(&lines, &fp.hunks)?;
    // Only override the trailing-newline state when the patch made an
    // explicit statement about the file's end; otherwise preserve it.
    let trailing = if fp.saw_eof_marker {
        !fp.new_no_trailing_newline
    } else {
        had_trailing
    };
    Ok(join_lines(&new_lines, crlf, trailing))
}

/// Build the content of a newly created file (`--- /dev/null`) from its
/// added lines. LF endings; trailing newline unless the patch marked none.
pub(crate) fn build_created_content(fp: &FilePatch) -> Result<String, String> {
    let mut lines: Vec<String> = Vec::new();
    for (n, h) in fp.hunks.iter().enumerate() {
        for l in &h.lines {
            match l {
                HunkLine::Add(s) => lines.push(s.clone()),
                // Blank context line from patch mangling — tolerated.
                HunkLine::Context(s) if s.is_empty() => lines.push(String::new()),
                _ => {
                    return Err(format!(
                        "hunk {}: a file-creation patch (--- /dev/null) must contain only added (+) lines",
                        n + 1
                    ));
                }
            }
        }
    }
    Ok(join_lines(&lines, false, !fp.new_no_trailing_newline))
}

// ─────────────────────────── execution ────────────────────────────

enum FileOutcome {
    Applied { hunks: usize },
    Created { lines: usize },
    Deleted,
    Failed { reason: String },
}

/// Apply a unified diff to one or more project files.
pub async fn execute(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    if context.permissions() == PermissionLevel::Chat {
        return Ok(ToolOutput {
            content: "PERMISSION_DENIED: File writes are not allowed in Chat mode.".into(),
            is_error: true,
            attachments: Vec::new(),
        });
    }

    let patch_text = match params["patch"].as_str() {
        Some(p) if !p.trim().is_empty() => p,
        _ => {
            return Ok(ToolOutput {
                content: "PATCH_FAILED: `patch` (a non-empty unified diff string) is required."
                    .into(),
                is_error: true,
                attachments: Vec::new(),
            });
        }
    };
    let strip = match params.get("strip") {
        Some(Value::Number(n)) => n.as_u64().unwrap_or(1) as usize,
        Some(Value::String(s)) => s.trim().parse().unwrap_or(1),
        _ => 1,
    };

    let file_patches = match parse_unified_diff(patch_text, strip) {
        Ok(f) => f,
        Err(e) => {
            return Ok(ToolOutput {
                content: format!("PATCH_FAILED: could not parse the patch — {}", e),
                is_error: true,
                attachments: Vec::new(),
            });
        }
    };

    // One permission prompt for the whole patch, not one per file.
    if context.needs_write_approval() {
        let op_path = if file_patches.len() == 1 {
            file_patches[0].path.clone()
        } else {
            format!(
                "{} (+{} more file(s) in one patch)",
                file_patches[0].path,
                file_patches.len() - 1
            )
        };
        let approved = context
            .permission_broker
            .request(
                &context.event_tx,
                &context.task_id,
                PermissionOp::WriteFile(op_path),
            )
            .await;
        if !approved {
            return Ok(ToolOutput {
                content: "PERMISSION_DENIED: User denied the patch application.".into(),
                is_error: true,
                attachments: Vec::new(),
            });
        }
    }

    // Per-file atomicity: each file either fully applies or is untouched;
    // one file's failure does not stop the others.
    let mut outcomes: Vec<(String, FileOutcome)> = Vec::with_capacity(file_patches.len());
    for fp in &file_patches {
        let outcome = match apply_one_file(fp, context).await {
            Ok(o) => o,
            Err(reason) => FileOutcome::Failed { reason },
        };
        outcomes.push((fp.path.clone(), outcome));
    }

    let failed = outcomes
        .iter()
        .filter(|(_, o)| matches!(o, FileOutcome::Failed { .. }))
        .count();
    let succeeded = outcomes.len() - failed;
    let all_failed = succeeded == 0;

    let mut body = if all_failed {
        format!(
            "PATCH_FAILED: 0/{} file(s) applied. No files were modified.\n",
            outcomes.len()
        )
    } else {
        format!(
            "Patch applied: {}/{} file(s) succeeded.\n",
            succeeded,
            outcomes.len()
        )
    };
    for (path, outcome) in &outcomes {
        let line = match outcome {
            FileOutcome::Applied { hunks } => {
                format!("  [ok] {} — applied {} hunk(s)", path, hunks)
            }
            FileOutcome::Created { lines } => {
                format!("  [ok] {} — created ({} line(s))", path, lines)
            }
            FileOutcome::Deleted => format!("  [ok] {} — deleted", path),
            FileOutcome::Failed { reason } => format!("  [FAILED] {} — {}", path, reason),
        };
        body.push_str(&line);
        body.push('\n');
    }
    if failed > 0 && !all_failed {
        body.push_str(
            "\nFailed files were left completely untouched (per-file atomicity). Fix the \
             failing hunks and re-issue a patch covering only the failed files.\n",
        );
    }

    Ok(ToolOutput {
        content: body,
        is_error: all_failed,
        attachments: Vec::new(),
    })
}

/// Apply one file's patch through the full write pipeline. `Err(reason)` on
/// any failure — in which case the file on disk is guaranteed untouched.
async fn apply_one_file(fp: &FilePatch, context: &ToolContext) -> Result<FileOutcome, String> {
    if let Some(violation) = check_write_scope(context, &fp.path) {
        return Err(violation.content);
    }
    let full_path = match resolve_within_project(&context.project_root, &fp.path) {
        Ok(p) => p,
        Err(violation) => return Err(violation.content),
    };
    if let Some(blocked) = check_sensitive_path(&fp.path, &full_path, context).await {
        return Err(blocked.content);
    }

    match fp.op {
        FileOp::Create => {
            if full_path.exists() {
                return Err(format!(
                    "the patch declares file creation (--- /dev/null) but '{}' already exists",
                    fp.path
                ));
            }
            let content = build_created_content(fp)?;
            let line_count = content.lines().count();
            if let Some(parent) = full_path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("could not create parent directories: {}", e))?;
            }
            track_before_write(context, &full_path);
            let _guard = context.file_lock.acquire(&full_path).await.map_err(|m| m)?;
            crate::io_util::atomic_write(&full_path, content.as_bytes())
                .map_err(|e| format!("write error: {}", e))?;
            maybe_emit_memory_updated(&fp.path, context);
            refresh_index_after_write(context, &full_path);
            context.file_read_registry.invalidate(&full_path);
            Ok(FileOutcome::Created { lines: line_count })
        }
        FileOp::Delete => {
            let content = match std::fs::read_to_string(&full_path) {
                Ok(c) => c,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return Err(format!("'{}' does not exist — nothing to delete", fp.path));
                }
                Err(e) => return Err(format!("could not read file: {}", e)),
            };
            // Verify the deletion hunks actually match the file before
            // removing it (all-Del hunks apply to an empty result).
            if !fp.hunks.is_empty() {
                let (lines, _) = split_lines(&content);
                apply_hunks(&lines, &fp.hunks)?;
            }
            track_before_write(context, &full_path);
            let _guard = context.file_lock.acquire(&full_path).await.map_err(|m| m)?;
            std::fs::remove_file(&full_path).map_err(|e| format!("delete error: {}", e))?;
            refresh_index_after_write(context, &full_path);
            context.file_read_registry.invalidate(&full_path);
            Ok(FileOutcome::Deleted)
        }
        FileOp::Modify => {
            let content = match std::fs::read_to_string(&full_path) {
                Ok(c) => c,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return Err(format!(
                        "'{}' does not exist (use --- /dev/null to create a new file)",
                        fp.path
                    ));
                }
                Err(e) => return Err(format!("could not read file: {}", e)),
            };
            let new_content = apply_to_content(&content, fp)?;
            track_before_write(context, &full_path);
            let _guard = context.file_lock.acquire(&full_path).await.map_err(|m| m)?;
            crate::io_util::atomic_write(&full_path, new_content.as_bytes())
                .map_err(|e| format!("write error: {}", e))?;
            maybe_emit_memory_updated(&fp.path, context);
            refresh_index_after_write(context, &full_path);
            context.file_read_registry.invalidate(&full_path);
            Ok(FileOutcome::Applied {
                hunks: fp.hunks.len(),
            })
        }
    }
}

// ─────────────────────────── tests ────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn lv(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    // ---------- parser ----------

    #[test]
    fn parse_two_files() {
        let patch = "\
diff --git a/src/a.rs b/src/a.rs
index 123..456 100644
--- a/src/a.rs
+++ b/src/a.rs
@@ -1,3 +1,3 @@
 fn main() {
-    old();
+    new();
 }
--- a/src/b.rs
+++ b/src/b.rs
@@ -5,2 +5,3 @@
 line5
+inserted
 line6
";
        let files = parse_unified_diff(patch, 1).unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, "src/a.rs");
        assert_eq!(files[0].op, FileOp::Modify);
        assert_eq!(files[0].hunks.len(), 1);
        assert_eq!(files[0].hunks[0].lines.len(), 4);
        assert_eq!(files[1].path, "src/b.rs");
        assert_eq!(files[1].hunks[0].old_count, 2);
        assert_eq!(files[1].hunks[0].new_count, 3);
    }

    #[test]
    fn parse_create_file() {
        let patch = "\
--- /dev/null
+++ b/src/new.rs
@@ -0,0 +1,2 @@
+fn hello() {}
+fn world() {}
";
        let files = parse_unified_diff(patch, 1).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].op, FileOp::Create);
        assert_eq!(files[0].path, "src/new.rs");
        let content = build_created_content(&files[0]).unwrap();
        assert_eq!(content, "fn hello() {}\nfn world() {}\n");
    }

    #[test]
    fn parse_delete_file() {
        let patch = "\
--- a/src/gone.rs
+++ /dev/null
@@ -1,2 +0,0 @@
-fn hello() {}
-fn world() {}
";
        let files = parse_unified_diff(patch, 1).unwrap();
        assert_eq!(files[0].op, FileOp::Delete);
        assert_eq!(files[0].path, "src/gone.rs");
        // Deletion hunks apply to an empty result.
        let out = apply_hunks(&lv(&["fn hello() {}", "fn world() {}"]), &files[0].hunks).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn parse_no_newline_marker_new_side() {
        let patch = "\
--- a/f.txt
+++ b/f.txt
@@ -1,2 +1,2 @@
 keep
-old
+new
\\ No newline at end of file
";
        let files = parse_unified_diff(patch, 1).unwrap();
        assert!(files[0].saw_eof_marker);
        assert!(files[0].new_no_trailing_newline);
        let out = apply_to_content("keep\nold\n", &files[0]).unwrap();
        assert_eq!(out, "keep\nnew"); // no trailing newline
    }

    #[test]
    fn parse_no_newline_marker_old_side_only() {
        // Marker after a deleted line: only the OLD content lacked the
        // newline — the new content gains one.
        let patch = "\
--- a/f.txt
+++ b/f.txt
@@ -1,2 +1,2 @@
 keep
-old
\\ No newline at end of file
+new
";
        let files = parse_unified_diff(patch, 1).unwrap();
        assert!(files[0].saw_eof_marker);
        assert!(!files[0].new_no_trailing_newline);
        let out = apply_to_content("keep\nold", &files[0]).unwrap();
        assert_eq!(out, "keep\nnew\n");
    }

    #[test]
    fn parse_header_without_counts() {
        let patch = "\
--- a/f.txt
+++ b/f.txt
@@ -1 +1 @@
-old
+new
";
        let files = parse_unified_diff(patch, 1).unwrap();
        let h = &files[0].hunks[0];
        assert_eq!(
            (h.old_start, h.old_count, h.new_start, h.new_count),
            (1, 1, 1, 1)
        );
    }

    #[test]
    fn parse_strip_components() {
        let patch = "\
--- x/y/src/f.rs
+++ x/y/src/f.rs
@@ -1 +1 @@
-a
+b
";
        assert_eq!(parse_unified_diff(patch, 2).unwrap()[0].path, "src/f.rs");
        assert_eq!(
            parse_unified_diff(patch, 0).unwrap()[0].path,
            "x/y/src/f.rs"
        );
        // Lenient: never strips the final component away.
        assert_eq!(parse_unified_diff(patch, 99).unwrap()[0].path, "f.rs");
    }

    #[test]
    fn parse_crlf_patch() {
        let patch = "--- a/f.txt\r\n+++ b/f.txt\r\n@@ -1 +1 @@\r\n-old\r\n+new\r\n";
        let files = parse_unified_diff(patch, 1).unwrap();
        assert_eq!(files[0].hunks[0].lines[0], HunkLine::Del("old".into()));
        assert_eq!(files[0].hunks[0].lines[1], HunkLine::Add("new".into()));
    }

    #[test]
    fn parse_rejects_garbage_only() {
        let err = parse_unified_diff("hello world\nnot a patch\n", 1).unwrap_err();
        assert!(err.contains("no `--- old`"), "got: {}", err);
    }

    #[test]
    fn parse_rejects_truncated_hunk() {
        let patch = "\
--- a/f.txt
+++ b/f.txt
@@ -1,3 +1,3 @@
 only-one-line
";
        let err = parse_unified_diff(patch, 1).unwrap_err();
        assert!(err.contains("truncated hunk"), "got: {}", err);
    }

    // ---------- apply ----------

    #[test]
    fn apply_exact_at_stated_line() {
        let file = lv(&["a", "b", "c", "d"]);
        let patch = "\
--- a/f
+++ b/f
@@ -2,2 +2,2 @@
 b
-c
+C
";
        let fp = &parse_unified_diff(patch, 1).unwrap()[0];
        let out = apply_hunks(&file, &fp.hunks).unwrap();
        assert_eq!(out, lv(&["a", "b", "C", "d"]));
    }

    #[test]
    fn apply_relocated_unique_match() {
        // Stated line is wrong (line 1), but the block matches uniquely at
        // line 5 — relocation succeeds.
        let file = lv(&["x", "y", "z", "w", "b", "c", "d"]);
        let patch = "\
--- a/f
+++ b/f
@@ -1,2 +1,2 @@
 b
-c
+C
";
        let fp = &parse_unified_diff(patch, 1).unwrap()[0];
        let out = apply_hunks(&file, &fp.hunks).unwrap();
        assert_eq!(out, lv(&["x", "y", "z", "w", "b", "C", "d"]));
    }

    #[test]
    fn apply_ambiguous_rejected() {
        // The block matches at two positions and the stated line matches at
        // neither — must refuse, never guess.
        let file = lv(&["b", "c", "spacer", "b", "c"]);
        let patch = "\
--- a/f
+++ b/f
@@ -10,2 +10,2 @@
 b
-c
+C
";
        let fp = &parse_unified_diff(patch, 1).unwrap()[0];
        let err = apply_hunks(&file, &fp.hunks).unwrap_err();
        assert!(err.contains("hunk 1"), "got: {}", err);
        assert!(err.contains("2 positions"), "got: {}", err);
        assert!(err.contains("refusing to guess"), "got: {}", err);
    }

    #[test]
    fn apply_stated_position_wins_over_ambiguity() {
        // Block matches at multiple places, but the stated line is one of
        // them — the stated position is authoritative.
        let file = lv(&["b", "c", "spacer", "b", "c"]);
        let patch = "\
--- a/f
+++ b/f
@@ -4,2 +4,2 @@
 b
-c
+C
";
        let fp = &parse_unified_diff(patch, 1).unwrap()[0];
        let out = apply_hunks(&file, &fp.hunks).unwrap();
        assert_eq!(out, lv(&["b", "c", "spacer", "b", "C"]));
    }

    #[test]
    fn apply_no_match_reports_first_mismatch() {
        let file = lv(&["alpha", "beta", "gamma"]);
        let patch = "\
--- a/f
+++ b/f
@@ -2,2 +2,2 @@
 beta
-DOES_NOT_EXIST
+replacement
";
        let fp = &parse_unified_diff(patch, 1).unwrap()[0];
        let err = apply_hunks(&file, &fp.hunks).unwrap_err();
        assert!(err.contains("hunk 1"), "got: {}", err);
        assert!(err.contains("\"DOES_NOT_EXIST\""), "got: {}", err);
        assert!(err.contains("\"gamma\""), "got: {}", err);
    }

    #[test]
    fn apply_whitespace_tolerant_fallback() {
        // File lines carry trailing whitespace the patch doesn't have.
        let file = lv(&["a", "b  ", "c\t", "d"]);
        let patch = "\
--- a/f
+++ b/f
@@ -2,2 +2,2 @@
 b
-c
+C
";
        let fp = &parse_unified_diff(patch, 1).unwrap()[0];
        let out = apply_hunks(&file, &fp.hunks).unwrap();
        // The matched region is replaced with the patch's new lines verbatim;
        // the context line `b` survives as the patch wrote it.
        assert_eq!(out, lv(&["a", "b", "C", "d"]));
    }

    #[test]
    fn apply_multi_hunk_with_drift() {
        let file = lv(&["1", "2", "3", "4", "5", "6", "7", "8"]);
        let patch = "\
--- a/f
+++ b/f
@@ -2,1 +2,3 @@
-2
+two
+two-and-a-bit
+two-more
@@ -7,1 +9,1 @@
-7
+seven
";
        let fp = &parse_unified_diff(patch, 1).unwrap()[0];
        let out = apply_hunks(&file, &fp.hunks).unwrap();
        assert_eq!(
            out,
            lv(&[
                "1",
                "two",
                "two-and-a-bit",
                "two-more",
                "3",
                "4",
                "5",
                "6",
                "seven",
                "8"
            ])
        );
    }

    #[test]
    fn apply_pure_insertion_zero_old_count() {
        // `-2,0` = insert after old line 2.
        let file = lv(&["a", "b", "c"]);
        let patch = "\
--- a/f
+++ b/f
@@ -2,0 +3,2 @@
+x
+y
";
        let fp = &parse_unified_diff(patch, 1).unwrap()[0];
        let out = apply_hunks(&file, &fp.hunks).unwrap();
        assert_eq!(out, lv(&["a", "b", "x", "y", "c"]));
    }

    #[test]
    fn apply_insertion_beyond_eof_rejected() {
        let file = lv(&["a"]);
        let patch = "\
--- a/f
+++ b/f
@@ -50,0 +51,1 @@
+x
";
        let fp = &parse_unified_diff(patch, 1).unwrap()[0];
        let err = apply_hunks(&file, &fp.hunks).unwrap_err();
        assert!(err.contains("outside the file"), "got: {}", err);
    }

    #[test]
    fn apply_atomicity_second_hunk_failure_errors_whole_file() {
        let file = lv(&["a", "b", "c"]);
        let patch = "\
--- a/f
+++ b/f
@@ -1,1 +1,1 @@
-a
+A
@@ -3,1 +3,1 @@
-NOPE
+never
";
        let fp = &parse_unified_diff(patch, 1).unwrap()[0];
        let err = apply_hunks(&file, &fp.hunks).unwrap_err();
        assert!(err.contains("hunk 2"), "got: {}", err);
        // Caller discards the result on Err — `file` itself is untouched.
        assert_eq!(file, lv(&["a", "b", "c"]));
    }

    // ---------- line endings / content round-trips ----------

    #[test]
    fn crlf_file_preserves_crlf_on_write() {
        let content = "a\r\nb\r\nc\r\n";
        let patch = "\
--- a/f
+++ b/f
@@ -2,1 +2,1 @@
-b
+B
";
        let fp = &parse_unified_diff(patch, 1).unwrap()[0];
        let out = apply_to_content(content, fp).unwrap();
        assert_eq!(out, "a\r\nB\r\nc\r\n");
    }

    #[test]
    fn lf_file_stays_lf() {
        let content = "a\nb\nc\n";
        let patch = "\
--- a/f
+++ b/f
@@ -2,1 +2,1 @@
-b
+B
";
        let fp = &parse_unified_diff(patch, 1).unwrap()[0];
        assert_eq!(apply_to_content(content, fp).unwrap(), "a\nB\nc\n");
    }

    #[test]
    fn missing_trailing_newline_preserved_when_patch_silent() {
        // Original file has no trailing newline; the patch touches the middle
        // and makes no statement about EOF — the missing newline survives.
        let content = "a\nb\nc";
        let patch = "\
--- a/f
+++ b/f
@@ -1,1 +1,1 @@
-a
+A
";
        let fp = &parse_unified_diff(patch, 1).unwrap()[0];
        assert_eq!(apply_to_content(content, fp).unwrap(), "A\nb\nc");
    }

    #[test]
    fn created_file_no_trailing_newline_marker() {
        let patch = "\
--- /dev/null
+++ b/n.txt
@@ -0,0 +1,1 @@
+only-line
\\ No newline at end of file
";
        let fp = &parse_unified_diff(patch, 1).unwrap()[0];
        assert_eq!(build_created_content(fp).unwrap(), "only-line");
    }

    #[test]
    fn created_file_rejects_deletion_lines() {
        let patch = "\
--- /dev/null
+++ b/n.txt
@@ -1,1 +1,1 @@
-bogus
+real
";
        let fp = &parse_unified_diff(patch, 1).unwrap()[0];
        let err = build_created_content(fp).unwrap_err();
        assert!(err.contains("only added"), "got: {}", err);
    }

    #[test]
    fn detect_crlf_dominance() {
        assert!(detect_crlf("a\r\nb\r\n"));
        assert!(!detect_crlf("a\nb\n"));
        assert!(!detect_crlf(""));
        // Mixed but CRLF-dominant.
        assert!(detect_crlf("a\r\nb\r\nc\n"));
    }

    #[test]
    fn header_paths_with_timestamps_and_devnull() {
        assert_eq!(
            header_path("a/f.rs\t2024-01-01 00:00:00", 1),
            Some("f.rs".into())
        );
        assert_eq!(header_path("/dev/null", 1), None);
        assert_eq!(header_path("b/dir/f.rs", 1), Some("dir/f.rs".into()));
    }
}

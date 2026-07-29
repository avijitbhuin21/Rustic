//! The single write choke point for every file-mutating tool.
//!
//! Every write goes: acquire the per-file lock → re-read the file *inside* the
//! lock → recompute the new content from that fresh text → write. Computing
//! content before the lock is what silently lost work: two agents editing one
//! file each spliced their change into a stale copy and wrote the whole file
//! back, so the second write erased the first with no error.
//!
//! Recomputing inside the lock means disjoint edits both land (the second
//! agent's anchor is still present in the first agent's output) and colliding
//! edits fail visibly instead of clobbering.

use std::path::Path;

use super::file_ops::{maybe_emit_memory_updated, refresh_index_after_write, track_before_write};
use super::{ToolContext, ToolOutput};

/// Which tool asked for the write. Recorded with every divergence so the
/// Phase-3 gate can answer "which tool actually carries collision volume".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WriteTool {
    CreateFile,
    EditFile,
    EditNotebook,
    MoveFile,
    /// A conflict resolver re-applying an intent. Never itself resolved.
    Resolver,
}

impl WriteTool {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            WriteTool::CreateFile => "create_file",
            WriteTool::EditFile => "edit_file",
            WriteTool::EditNotebook => "edit_notebook",
            WriteTool::MoveFile => "move_file",
            WriteTool::Resolver => "resolver",
        }
    }
}

/// On-disk state of the target, read inside the per-file lock.
pub(crate) struct FreshFile {
    pub existed: bool,
    /// `None` when the file is absent or is not valid UTF-8.
    pub text: Option<String>,
    /// True when the fresh bytes differ from the task's base snapshot, i.e.
    /// something changed the file since this turn started.
    pub diverged_from_base: bool,
    /// Base-snapshot text, present only when the file diverged and the base
    /// was valid UTF-8. This is the 3-way merge base.
    pub base_text: Option<String>,
}

impl FreshFile {
    /// Fresh text or the empty string, for callers that treat absent as empty.
    pub(crate) fn text_or_empty(&self) -> &str {
        self.text.as_deref().unwrap_or("")
    }
}

/// A write site's decision, computed from the fresh content.
pub(crate) struct Prepared<T> {
    pub content: Vec<u8>,
    pub payload: T,
    /// Drop cached read coverage after the write. Set when byte offsets or
    /// cell numbers shift under the model (notebook insert/delete).
    pub invalidate_reads: bool,
}

impl<T> Prepared<T> {
    pub(crate) fn new(content: Vec<u8>, payload: T) -> Self {
        Self {
            content,
            payload,
            invalidate_reads: false,
        }
    }
}

/// Why a guarded write produced no bytes.
pub(crate) enum GuardedAbort {
    /// Terminal outcome — hand this to the model verbatim.
    Output(ToolOutput),
    /// The intent could not be replayed against the fresh content and could
    /// not be merged. Surfaced only *after* the per-file lock is released, so
    /// a resolver can re-acquire it without deadlocking.
    Conflict(Box<ConflictBriefing>),
}

impl GuardedAbort {
    pub(crate) fn error(msg: impl Into<String>) -> Self {
        GuardedAbort::Output(ToolOutput::text(msg, true))
    }

    /// Collapse to a `ToolOutput`. A conflict that nothing handled is reported
    /// as an error rather than silently dropped.
    pub(crate) fn into_output(self) -> ToolOutput {
        match self {
            GuardedAbort::Output(o) => o,
            GuardedAbort::Conflict(b) => ToolOutput::text(b.unresolved_message(), true),
        }
    }
}

/// Everything a resolver agent needs to reconcile one file by hand.
pub struct ConflictBriefing {
    /// Project-relative path, as the model referred to it.
    pub display_path: String,
    pub abs_path: std::path::PathBuf,
    pub tool: &'static str,
    /// What this agent was trying to do, in its own terms.
    pub intent: ConflictIntent,
    /// Unified diff of base → disk: what changed underneath this agent.
    pub underneath_diff: String,
    /// Merged text with diff3 conflict markers, when a merge was attempted.
    pub merged_text: Option<String>,
    /// Structural defects reported by the merge layer (`dangling:foo`,
    /// `unchecked:duplicates`, …). Empty when no structural pass ran.
    pub defects: Vec<String>,
    /// Merge-layer status label, when a merge was attempted.
    pub merge_status: Option<String>,
    /// Symbol-index hints for where the anchor may have moved.
    pub relocation_hints: Vec<String>,
}

/// The edit that could not be replayed.
pub enum ConflictIntent {
    /// `edit_file` whose anchor is gone from the fresh content.
    StaleAnchor {
        old_string: String,
        new_string: String,
        replace_all: bool,
    },
}

impl ConflictBriefing {
    /// Message used when no resolver could run — states the conflict plainly
    /// instead of pretending the write succeeded.
    pub fn unresolved_message(&self) -> String {
        let defects = if self.defects.is_empty() {
            String::new()
        } else {
            format!("\nStructural defects: {}", self.defects.join(", "))
        };
        format!(
            "WRITE_CONFLICT: '{}' changed under you since this turn started and your change \
             could not be applied automatically. Nothing was written — the file on disk is \
             intact. Re-read the file and redo your edit against its current content.{}\n\n\
             What changed underneath:\n{}",
            self.display_path,
            defects,
            truncate_for_prompt(&self.underneath_diff, 4000)
        )
    }
}

pub(crate) fn truncate_for_prompt(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n… [truncated, {} bytes total]", &s[..end], s.len())
}

/// Read the target inside the lock and diff it against the task's base
/// snapshot. Divergence is what every later phase keys off: telemetry, merge,
/// and resolver handoff all need to know the file moved under this agent.
fn read_fresh(ctx: &ToolContext, abs_path: &Path) -> FreshFile {
    let bytes = std::fs::read(abs_path).ok();
    let existed = bytes.is_some();
    let text = bytes
        .as_ref()
        .and_then(|b| std::str::from_utf8(b).ok())
        .map(|s| s.to_string());

    let base = ctx
        .file_history
        .as_ref()
        .zip(ctx.current_user_message_id.as_ref())
        .and_then(|(history, message_id)| history.base_content(message_id, abs_path).ok())
        .flatten();

    let diverged = match (&base, &bytes) {
        (Some(b), Some(d)) => b != d,
        // Absent from the base snapshot and present now (or vice versa) is a
        // real change, but for a file this turn legitimately created there is
        // no base to merge against — treat as not diverged so create_file's
        // own FILE_EXISTS check stays the authority.
        _ => false,
    };

    FreshFile {
        existed,
        text,
        diverged_from_base: diverged,
        base_text: if diverged {
            base.and_then(|b| String::from_utf8(b).ok())
        } else {
            None
        },
    }
}

/// Run a write through the choke point.
///
/// `prepare` receives the content that is on disk *right now*, with the lock
/// already held, and returns the bytes to write. It runs at most once and must
/// not block for long — the per-file lock is held across it.
pub(crate) async fn guarded_write<T, F>(
    ctx: &ToolContext,
    abs_path: &Path,
    display_path: &str,
    tool: WriteTool,
    prepare: F,
) -> Result<T, GuardedAbort>
where
    F: FnOnce(&FreshFile) -> Result<Prepared<T>, GuardedAbort>,
{
    let guard = match ctx.file_lock.acquire(abs_path).await {
        Ok(g) => g,
        Err(msg) => return Err(GuardedAbort::error(msg)),
    };

    let fresh = read_fresh(ctx, abs_path);
    if fresh.diverged_from_base {
        super::write_telemetry::record(
            ctx,
            abs_path,
            tool,
            super::write_telemetry::Event::Diverged,
        );
    }

    let prepared = match prepare(&fresh) {
        Ok(p) => p,
        Err(abort) => {
            // Drop the lock before returning: a Conflict is handed to a
            // resolver that has to re-acquire this very lock.
            drop(guard);
            return Err(abort);
        }
    };

    track_before_write(ctx, abs_path);
    let write_result = crate::io_util::atomic_write(abs_path, &prepared.content);

    match write_result {
        Ok(()) => {
            maybe_emit_memory_updated(display_path, ctx);
            refresh_index_after_write(ctx, abs_path);
            if prepared.invalidate_reads {
                ctx.file_read_registry.invalidate(abs_path);
            }
            drop(guard);
            Ok(prepared.payload)
        }
        Err(e) => {
            drop(guard);
            Err(GuardedAbort::error(format!("Error writing file: {}", e)))
        }
    }
}

/// Guarded rename. Two locks in a stable order so concurrent moves over the
/// same pair can't deadlock; merge is meaningless for a rename, but a move
/// racing another agent's edit is still a real conflict, so divergence on the
/// source is detected and reported rather than silently overwritten.
pub(crate) async fn guarded_move<F>(
    ctx: &ToolContext,
    src: &Path,
    dst: &Path,
    display_src: &str,
    display_dst: &str,
    perform: F,
) -> Result<(), GuardedAbort>
where
    F: FnOnce() -> Result<(), String>,
{
    let (first, second) = if src <= dst { (src, dst) } else { (dst, src) };
    let g1 = match ctx.file_lock.acquire(first).await {
        Ok(g) => g,
        Err(msg) => return Err(GuardedAbort::error(msg)),
    };
    let g2 = match ctx.file_lock.acquire(second).await {
        Ok(g) => g,
        Err(msg) => {
            drop(g1);
            return Err(GuardedAbort::error(msg));
        }
    };

    let fresh_src = read_fresh(ctx, src);
    if fresh_src.diverged_from_base {
        super::write_telemetry::record(
            ctx,
            src,
            WriteTool::MoveFile,
            super::write_telemetry::Event::Diverged,
        );
        drop(g2);
        drop(g1);
        return Err(GuardedAbort::error(format!(
            "MOVE_CONFLICT: '{}' was modified by another agent since this turn started. \
             The move was NOT performed so those changes aren't carried to an unexpected \
             path. Re-read '{}' and retry the move if it's still what you want.",
            display_src, display_src
        )));
    }

    track_before_write(ctx, src);
    track_before_write(ctx, dst);

    let outcome = perform();

    match outcome {
        Ok(()) => {
            ctx.file_read_registry.invalidate(src);
            ctx.file_read_registry.invalidate(dst);
            ctx.workspace_services.notify_file_deleted(src);
            if dst.is_file() {
                refresh_index_after_write(ctx, dst);
            }
            maybe_emit_memory_updated(display_src, ctx);
            maybe_emit_memory_updated(display_dst, ctx);
            drop(g2);
            drop(g1);
            Ok(())
        }
        Err(msg) => {
            drop(g2);
            drop(g1);
            Err(GuardedAbort::error(msg))
        }
    }
}

//! Phase-3 measurement for the write choke point.
//!
//! Answers three questions with data instead of assumption: how often does a
//! file actually change under an agent mid-turn, which languages does it happen
//! in, and which tool causes it. Recording is best-effort and never fails a
//! write.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use super::guarded_write::WriteTool;
use super::ToolContext;

/// What happened at the choke point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Event {
    /// On-disk content differed from the task's base snapshot at write time.
    Diverged,
    /// The anchor was gone AND the file had changed since the base snapshot —
    /// a genuine concurrent-edit collision rather than a bad `old_string`.
    NoMatchAfterDivergence,
    /// A 3-way merge reconciled the two changes and landed silently.
    AutoMerged,
    /// Merge was impossible or defective; a resolver was asked to reconcile.
    ConflictEscalated,
    /// A resolver re-applied the intent successfully.
    ResolverResolved,
}

impl Event {
    fn as_str(self) -> &'static str {
        match self {
            Event::Diverged => "diverged",
            Event::NoMatchAfterDivergence => "no_match_after_divergence",
            Event::AutoMerged => "auto_merged",
            Event::ConflictEscalated => "conflict_escalated",
            Event::ResolverResolved => "resolver_resolved",
        }
    }
}

static DIVERGED: AtomicU64 = AtomicU64::new(0);
static NO_MATCH_AFTER_DIVERGENCE: AtomicU64 = AtomicU64::new(0);
static AUTO_MERGED: AtomicU64 = AtomicU64::new(0);
static CONFLICT_ESCALATED: AtomicU64 = AtomicU64::new(0);
static RESOLVER_RESOLVED: AtomicU64 = AtomicU64::new(0);

fn counter(event: Event) -> &'static AtomicU64 {
    match event {
        Event::Diverged => &DIVERGED,
        Event::NoMatchAfterDivergence => &NO_MATCH_AFTER_DIVERGENCE,
        Event::AutoMerged => &AUTO_MERGED,
        Event::ConflictEscalated => &CONFLICT_ESCALATED,
        Event::ResolverResolved => &RESOLVER_RESOLVED,
    }
}

/// Process-wide counts. The Phase-3 gate is computed from the JSONL log across
/// sessions, so in-process counts only ever serve assertions.
#[cfg(test)]
pub(crate) fn snapshot() -> [(Event, u64); 5] {
    [
        (Event::Diverged, DIVERGED.load(Ordering::Relaxed)),
        (
            Event::NoMatchAfterDivergence,
            NO_MATCH_AFTER_DIVERGENCE.load(Ordering::Relaxed),
        ),
        (Event::AutoMerged, AUTO_MERGED.load(Ordering::Relaxed)),
        (
            Event::ConflictEscalated,
            CONFLICT_ESCALATED.load(Ordering::Relaxed),
        ),
        (
            Event::ResolverResolved,
            RESOLVER_RESOLVED.load(Ordering::Relaxed),
        ),
    ]
}

/// Cap the log so a pathological workload can't fill the disk. Once past it the
/// counters keep incrementing but the JSONL stops growing.
const MAX_LOG_BYTES: u64 = 4 * 1024 * 1024;

/// Record one choke-point event. Emits a structured tracing event always, and
/// appends a JSONL row under `.rustic/` when a project root is available.
pub(crate) fn record(ctx: &ToolContext, abs_path: &Path, tool: WriteTool, event: Event) {
    counter(event).fetch_add(1, Ordering::Relaxed);

    let lang = rustic_treesitter::detect::language_for_path(abs_path).unwrap_or("unknown");
    let rel = abs_path
        .strip_prefix(&ctx.project_root)
        .unwrap_or(abs_path)
        .to_string_lossy()
        .replace('\\', "/");

    tracing::info!(
        event = event.as_str(),
        tool = tool.as_str(),
        lang,
        path = %rel,
        task_id = %ctx.task_id,
        "write_divergence"
    );

    let row = serde_json::json!({
        "ts": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        "event": event.as_str(),
        "tool": tool.as_str(),
        "lang": lang,
        "path": rel,
        "task_id": ctx.task_id,
        "agent": ctx.subagent_self.as_ref().map(|(_, id)| id.clone()),
    });
    append_row(&ctx.project_root, &row);
}

fn append_row(project_root: &Path, row: &serde_json::Value) {
    let dir = project_root.join(".rustic");
    if !dir.is_dir() && std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let log = dir.join("write-divergence.jsonl");
    if std::fs::metadata(&log).is_ok_and(|m| m.len() > MAX_LOG_BYTES) {
        return;
    }
    use std::io::Write as _;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log)
    {
        let _ = writeln!(f, "{}", row);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count(events: &[(Event, u64)], want: Event) -> u64 {
        events
            .iter()
            .find(|(e, _)| *e == want)
            .map(|(_, n)| *n)
            .unwrap_or(0)
    }

    #[test]
    fn recording_bumps_the_counter_and_appends_a_jsonl_row() {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, _rx) = ToolContext::new_test(dir.path().to_path_buf());
        let before = count(&snapshot(), Event::Diverged);

        record(
            &ctx,
            &dir.path().join("src/lib.rs"),
            WriteTool::EditFile,
            Event::Diverged,
        );

        assert_eq!(count(&snapshot(), Event::Diverged), before + 1);
        let log = dir.path().join(".rustic").join("write-divergence.jsonl");
        let body = std::fs::read_to_string(&log).expect("jsonl row written");
        let row: serde_json::Value = serde_json::from_str(body.lines().next().unwrap()).unwrap();
        assert_eq!(row["event"], "diverged");
        assert_eq!(row["tool"], "edit_file");
        assert_eq!(row["lang"], "rust");
        assert_eq!(row["path"], "src/lib.rs");
    }

    #[test]
    fn a_missing_project_root_never_panics() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("gone");
        let (ctx, _rx) = ToolContext::new_test(root.clone());
        drop(dir);
        record(
            &ctx,
            &root.join("a.txt"),
            WriteTool::CreateFile,
            Event::AutoMerged,
        );
    }
}

//! End-to-end tests for the concurrent-write guard, with a *real*
//! `FileHistory` attached.
//!
//! The tests in `file_ops::concurrent_write_tests` drive the real `execute`
//! entry point but run on a context with no file history, so they only cover
//! Phase 1 (re-read inside the lock). Without a base snapshot there is no merge
//! base, so the whole Lattice path — divergence detection, 3-way merge,
//! structural defect gating, resolver escalation — is never reached.
//!
//! These tests attach a real tracker over a temp project, open a baseline
//! snapshot, then let a "second agent" land a write directly on disk before the
//! first agent's tool call reaches the choke point. That is exactly what a
//! parallel agent's completed write looks like from the victim's side.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use super::file_ops;
use super::{ToolContext, ToolOutput};
use crate::file_history::FileHistory;
use rustic_db::Database;

const BASE_MSG: &str = "msg-base";

struct Harness {
    _cfg_dir: tempfile::TempDir,
    _proj_dir: tempfile::TempDir,
    ctx: Arc<ToolContext>,
    history: Arc<FileHistory>,
    root: PathBuf,
}

impl Harness {
    fn abs(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    fn disk(&self, rel: &str) -> String {
        std::fs::read_to_string(self.abs(rel)).unwrap()
    }

    /// A second agent's write that has already landed. Written straight to disk
    /// so it is invisible to the first agent until it re-reads inside the lock.
    fn other_agent_writes(&self, rel: &str, body: &str) {
        std::fs::write(self.abs(rel), body).unwrap();
    }

    async fn call(&self, tool: &str, params: Value) -> ToolOutput {
        file_ops::execute(tool, params, &self.ctx).await.unwrap()
    }

    /// Register the file as read, as a real agent does before editing. Without
    /// this a failed match is reported as MUST_READ_FIRST and never reaches the
    /// stale-anchor recovery path.
    async fn agent_reads(&self, rel: &str) {
        let out = self.call("read_file", json!({ "path": rel })).await;
        assert!(!out.is_error, "read_file failed: {}", out.content);
    }

    /// Rows the choke point appended to this project's divergence log.
    fn telemetry_rows(&self) -> Vec<Value> {
        let log = self.root.join(".rustic").join("write-divergence.jsonl");
        let Ok(text) = std::fs::read_to_string(log) else {
            return Vec::new();
        };
        text.lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    fn logged_events(&self) -> Vec<String> {
        self.telemetry_rows()
            .iter()
            .filter_map(|r| r["event"].as_str().map(|s| s.to_string()))
            .collect()
    }
}

/// Seed `files`, open the baseline snapshot over them, and hand back a context
/// wired to that tracker. Seeding happens *before* the snapshot so the baseline
/// holds the original content — that is the merge base.
fn harness(files: &[(&str, &str)]) -> Harness {
    let cfg_dir = tempfile::tempdir().unwrap();
    let proj_dir = tempfile::tempdir().unwrap();
    let root = proj_dir.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join(".git")).unwrap();

    for (name, body) in files {
        let p = root.join(name);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(p, body).unwrap();
    }

    let db = Arc::new(Mutex::new(Database::in_memory().unwrap()));
    {
        let g = db.lock().unwrap();
        g.conn()
            .execute(
                "INSERT INTO projects (id, name, root_path) VALUES ('p', 'p', 'p')",
                [],
            )
            .unwrap();
        g.conn()
            .execute(
                "INSERT INTO tasks (id, project_id, title, status, provider_type, model)
                 VALUES ('test-task', 'p', 'title', 'created', 'native', 'm')",
                [],
            )
            .unwrap();
    }

    let history = FileHistory::new(db, root.clone(), cfg_dir.path()).unwrap();
    history.open_snapshot(BASE_MSG, "test-task").unwrap();
    let history = Arc::new(history);

    let (mut ctx, rx) = ToolContext::new_test(root.clone());
    // A closed event channel makes every emission fail, which is not what any
    // of these tests are exercising.
    std::mem::forget(rx);
    ctx.file_history = Some(history.clone());
    ctx.current_user_message_id = Some(BASE_MSG.to_string());

    Harness {
        _cfg_dir: cfg_dir,
        _proj_dir: proj_dir,
        ctx: Arc::new(ctx),
        history,
        root,
    }
}

fn edit(rel: &str, old: &str, new: &str) -> Value {
    json!({ "path": rel, "old_string": old, "new_string": new })
}

// ---------------------------------------------------------------------------
// Phase 1 — the headline scenario, now with history attached
// ---------------------------------------------------------------------------

const FIVE_FNS: &str = "\
fn a() {
    1
}

fn b() {
    2
}

fn c() {
    3
}

fn d() {
    4
}

fn e() {
    5
}
";

#[tokio::test]
async fn five_concurrent_agents_on_one_file_all_land() {
    let h = harness(&[("src/lib.rs", FIVE_FNS)]);

    let bodies = [("1", "111"), ("2", "222"), ("3", "333"), ("4", "444")];
    let mut handles = Vec::new();
    for (from, to) in bodies {
        let ctx = h.ctx.clone();
        let params = edit(
            "src/lib.rs",
            &format!("    {}\n", from),
            &format!("    {}\n", to),
        );
        handles.push(tokio::spawn(async move {
            file_ops::execute("edit_file", params, &ctx).await.unwrap()
        }));
    }
    // Fifth agent appends, which reads the whole file to concatenate — the
    // classic clobber shape.
    {
        let ctx = h.ctx.clone();
        let params = edit("src/lib.rs", "", "fn f() {\n    6\n}\n");
        handles.push(tokio::spawn(async move {
            file_ops::execute("edit_file", params, &ctx).await.unwrap()
        }));
    }

    for handle in handles {
        let out = handle.await.unwrap();
        assert!(!out.is_error, "a disjoint edit failed: {}", out.content);
    }

    let final_text = h.disk("src/lib.rs");
    for expected in ["111", "222", "333", "444", "fn f()"] {
        assert!(
            final_text.contains(expected),
            "{} was lost:\n{}",
            expected,
            final_text
        );
    }
    assert!(
        final_text.contains("fn e() {\n    5\n}"),
        "untouched function was damaged:\n{}",
        final_text
    );
}

// ---------------------------------------------------------------------------
// Phase 6 — Lattice merge on the stale-anchor path
// ---------------------------------------------------------------------------

const THREE_FNS: &str = "\
fn a() {
    1
}

fn b() {
    2
}

fn c() {
    3
}
";

/// The agent's anchor spans `a` and `b`; it only means to change `a`. Another
/// agent changes `b`, so the anchor is gone from disk but still present in the
/// base snapshot. The intent is therefore well-defined and the two changes are
/// structurally disjoint, so the merge must land both with no escalation.
#[tokio::test]
async fn stale_anchor_auto_merges_when_the_two_intents_are_disjoint() {
    let h = harness(&[("src/lib.rs", THREE_FNS)]);
    h.agent_reads("src/lib.rs").await;

    h.other_agent_writes(
        "src/lib.rs",
        "fn a() {\n    1\n}\n\nfn b() {\n    222\n}\n\nfn c() {\n    3\n}\n",
    );

    let out = h
        .call(
            "edit_file",
            edit(
                "src/lib.rs",
                "fn a() {\n    1\n}\n\nfn b() {\n    2\n}\n",
                "fn a() {\n    111\n}\n\nfn b() {\n    2\n}\n",
            ),
        )
        .await;

    assert!(
        !out.is_error,
        "a structurally disjoint merge must land silently: {}",
        out.content
    );
    assert!(
        out.content.contains("AUTO_MERGED"),
        "the agent must be told its edit was merged, got: {}",
        out.content
    );

    let final_text = h.disk("src/lib.rs");
    assert!(
        final_text.contains("111"),
        "our change was lost:\n{}",
        final_text
    );
    assert!(
        final_text.contains("222"),
        "the other agent's change was clobbered:\n{}",
        final_text
    );
    assert!(
        final_text.contains("fn c() {\n    3\n}"),
        "untouched function was damaged:\n{}",
        final_text
    );

    let events = h.logged_events();
    assert!(
        events.contains(&"auto_merged".to_string()),
        "auto-merge must be recorded for the Phase-3 gate, got {:?}",
        events
    );
    let row = h
        .telemetry_rows()
        .into_iter()
        .find(|r| r["event"] == "auto_merged")
        .unwrap();
    assert_eq!(row["tool"], "edit_file");
    assert_eq!(
        row["lang"], "rust",
        "language must be recorded, not unknown"
    );
    assert_eq!(row["path"], "src/lib.rs");
}

/// Both agents rewrite the same function body. There is no correct automatic
/// answer, so nothing may be written and the loser must be told.
#[tokio::test]
async fn same_region_conflict_escalates_and_leaves_disk_untouched() {
    let h = harness(&[("src/lib.rs", THREE_FNS)]);
    h.agent_reads("src/lib.rs").await;

    let theirs = "fn a() {\n    999\n}\n\nfn b() {\n    2\n}\n\nfn c() {\n    3\n}\n";
    h.other_agent_writes("src/lib.rs", theirs);

    let out = h
        .call(
            "edit_file",
            edit("src/lib.rs", "fn a() {\n    1\n}", "fn a() {\n    111\n}"),
        )
        .await;

    assert!(out.is_error, "a same-region collision must fail visibly");
    assert!(
        out.content.contains("WRITE_CONFLICT"),
        "expected a conflict briefing, got: {}",
        out.content
    );
    assert!(
        out.content.contains("What changed underneath"),
        "the briefing must carry the underneath diff, got: {}",
        out.content
    );
    assert_eq!(
        h.disk("src/lib.rs"),
        theirs,
        "the other agent's work must survive byte-for-byte"
    );
    assert!(
        h.logged_events()
            .contains(&"conflict_escalated".to_string()),
        "escalation must be recorded, got {:?}",
        h.logged_events()
    );
}

/// The money case for Lattice over a plain line merge: the 3-way merge is
/// *textually* clean, but the result is semantically broken — one agent deleted
/// `helper` while the other added a new call to it. A diff3 merge would land
/// this silently. The structural check must catch it and refuse.
#[tokio::test]
async fn textually_clean_but_dangling_merge_does_not_auto_land() {
    let base = "\
fn helper(x: i32) -> i32 {
    x
}

fn one() {
    helper(1);
}

fn two() {
    let _ = 2;
}
";
    let h = harness(&[("src/lib.rs", base)]);
    h.agent_reads("src/lib.rs").await;

    // Other agent adds a *new* call to helper, in a region we don't touch.
    let theirs = "\
fn helper(x: i32) -> i32 {
    x
}

fn one() {
    helper(1);
}

fn two() {
    let _ = helper(3);
}
";
    h.other_agent_writes("src/lib.rs", theirs);

    // Our intent, expressed as a whole-file rewrite: delete helper and its only
    // known call. Anchor is the base file, which is no longer on disk.
    let ours = "fn one() {\n}\n\nfn two() {\n    let _ = 2;\n}\n";
    let out = h.call("edit_file", edit("src/lib.rs", base, ours)).await;

    assert!(
        out.is_error,
        "a merge that leaves a dangling reference must not land: {}",
        out.content
    );
    assert!(
        out.content.contains("WRITE_CONFLICT"),
        "expected a conflict briefing, got: {}",
        out.content
    );
    assert!(
        out.content.contains("helper"),
        "the briefing must name the dangling symbol so a resolver can act: {}",
        out.content
    );
    assert_eq!(
        h.disk("src/lib.rs"),
        theirs,
        "disk must be untouched — a broken file is worse than a failed write"
    );
}

// ---------------------------------------------------------------------------
// Phase 2 — the non-edit write sites
// ---------------------------------------------------------------------------

#[tokio::test]
async fn move_file_refuses_when_the_source_diverged() {
    let h = harness(&[("a.txt", "original\n")]);

    let theirs = "changed by another agent\n";
    h.other_agent_writes("a.txt", theirs);

    let out = h
        .call("move_file", json!({ "path": "a.txt", "new_path": "b.txt" }))
        .await;

    assert!(out.is_error, "moving a diverged source must fail");
    assert!(
        out.content.contains("MOVE_CONFLICT"),
        "expected MOVE_CONFLICT, got: {}",
        out.content
    );
    assert_eq!(
        h.disk("a.txt"),
        theirs,
        "the other agent's content must stay where it is"
    );
    assert!(
        !h.abs("b.txt").exists(),
        "the destination must not have been created"
    );
}

#[tokio::test]
async fn create_file_refuses_to_clobber_a_file_that_appeared_underneath() {
    let h = harness(&[]);

    h.other_agent_writes("new.txt", "written by another agent\n");

    let out = h
        .call(
            "create_file",
            json!({ "path": "new.txt", "content": "mine\n" }),
        )
        .await;

    assert!(out.is_error, "create_file must not overwrite blind");
    assert_eq!(
        h.disk("new.txt"),
        "written by another agent\n",
        "an anchorless full-content write must never clobber"
    );
}

// ---------------------------------------------------------------------------
// Phase 8 — revert still works across an auto-merge
// ---------------------------------------------------------------------------

#[tokio::test]
async fn revert_restores_the_baseline_after_an_auto_merge() {
    let h = harness(&[("src/lib.rs", THREE_FNS)]);
    h.agent_reads("src/lib.rs").await;

    h.other_agent_writes(
        "src/lib.rs",
        "fn a() {\n    1\n}\n\nfn b() {\n    222\n}\n\nfn c() {\n    3\n}\n",
    );
    let out = h
        .call(
            "edit_file",
            edit(
                "src/lib.rs",
                "fn a() {\n    1\n}\n\nfn b() {\n    2\n}\n",
                "fn a() {\n    111\n}\n\nfn b() {\n    2\n}\n",
            ),
        )
        .await;
    assert!(!out.is_error, "setup merge failed: {}", out.content);
    assert!(h.disk("src/lib.rs").contains("111"));

    // Production records the final tree at every turn end; revert's
    // cross-session guard uses it to tell "this task wrote that" from "another
    // session wrote that". Without it revert correctly refuses to touch the
    // file, which is the documented behaviour, not a merge-specific gap.
    h.history.record_final_state("test-task").unwrap();
    let outcomes = h.history.revert_from_message(BASE_MSG).unwrap();
    assert!(
        !outcomes.is_empty(),
        "revert produced no outcomes — the merged write was not attributed \
         to the task"
    );

    assert_eq!(
        h.disk("src/lib.rs"),
        THREE_FNS,
        "revert must restore the pre-turn baseline even after an auto-merge"
    );
}

// ---------------------------------------------------------------------------
// Phase 5 — a resolver's own writes are never re-escalated
// ---------------------------------------------------------------------------

/// A resolver writes the very file it was spawned to reconcile, so its writes
/// see a diverged file by definition. If it were escalated like any other agent
/// it would spawn a resolver for itself and recurse.
#[tokio::test]
async fn a_resolver_write_bypasses_escalation_instead_of_recursing() {
    let h = harness(&[("src/lib.rs", THREE_FNS)]);

    let mut resolver_ctx = {
        let (mut c, rx) = ToolContext::new_test(h.root.clone());
        std::mem::forget(rx);
        c.file_history = h.ctx.file_history.clone();
        c.current_user_message_id = Some(BASE_MSG.to_string());
        c
    };
    resolver_ctx.subagent_self = Some((
        "test-task".to_string(),
        format!("{}src/lib.rs", super::write_conflict::RESOLVER_PREFIX),
    ));
    let resolver_ctx = Arc::new(resolver_ctx);
    assert!(super::write_conflict::is_resolver(&resolver_ctx));

    // Mark as read, then let another agent move the file underneath.
    file_ops::execute("read_file", json!({ "path": "src/lib.rs" }), &resolver_ctx)
        .await
        .unwrap();
    h.other_agent_writes(
        "src/lib.rs",
        "fn a() {\n    1\n}\n\nfn b() {\n    999\n}\n\nfn c() {\n    3\n}\n",
    );

    let out = file_ops::execute(
        "edit_file",
        edit("src/lib.rs", "fn b() {\n    2\n}", "fn b() {\n    2222\n}"),
        &resolver_ctx,
    )
    .await
    .unwrap();

    assert!(
        out.is_error,
        "the stale anchor still can't be applied: {}",
        out.content
    );
    assert!(
        out.content.contains("EDIT_NO_MATCH"),
        "a resolver must get the plain no-match, never a conflict escalation \
         (that is what would recurse), got: {}",
        out.content
    );
    assert!(
        !out.content.contains("WRITE_CONFLICT"),
        "resolver writes must not be escalated: {}",
        out.content
    );
}

// ---------------------------------------------------------------------------
// Phase 3 — the divergence log is actually usable as gate input
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_divergence_log_records_tool_language_and_path() {
    let h = harness(&[("app/main.py", "def a():\n    return 1\n")]);
    h.agent_reads("app/main.py").await;

    h.other_agent_writes("app/main.py", "def a():\n    return 99\n");
    let out = h
        .call(
            "edit_file",
            edit("app/main.py", "    return 1\n", "    return 2\n"),
        )
        .await;
    assert!(out.is_error, "same-region python collision must fail");

    let rows = h.telemetry_rows();
    assert!(!rows.is_empty(), "nothing was logged");
    for row in &rows {
        assert_eq!(row["lang"], "python", "row missing language: {}", row);
        assert_eq!(row["path"], "app/main.py");
        assert_eq!(row["task_id"], "test-task");
        assert!(row["ts"].as_u64().unwrap() > 0);
    }
    let events = h.logged_events();
    assert!(
        events.contains(&"diverged".to_string()),
        "divergence itself must be logged, got {:?}",
        events
    );
    assert!(
        events.contains(&"no_match_after_divergence".to_string()),
        "the Phase-3 gate needs no-match-after-divergence separated from a \
         plain bad anchor, got {:?}",
        events
    );
}

/// A file nobody else touched must not be logged as diverged, or the gate data
/// is noise.
#[tokio::test]
async fn an_uncontended_write_logs_nothing() {
    let h = harness(&[("src/lib.rs", THREE_FNS)]);

    let out = h
        .call(
            "edit_file",
            edit("src/lib.rs", "fn a() {\n    1\n}", "fn a() {\n    111\n}"),
        )
        .await;
    assert!(!out.is_error, "uncontended edit failed: {}", out.content);
    assert!(h.disk("src/lib.rs").contains("111"));
    assert!(
        h.telemetry_rows().is_empty(),
        "an uncontended write must be invisible to telemetry, got {:?}",
        h.logged_events()
    );
}

/// The choke point must not add measurable cost to the common case.
#[tokio::test]
async fn uncontended_writes_stay_cheap() {
    let h = harness(&[("src/lib.rs", FIVE_FNS)]);

    let started = std::time::Instant::now();
    for i in 0..50 {
        let out = h
            .call(
                "edit_file",
                edit("src/lib.rs", "", &format!("// line {}\n", i)),
            )
            .await;
        assert!(!out.is_error, "append {} failed: {}", i, out.content);
    }
    let elapsed = started.elapsed();
    assert!(
        elapsed < std::time::Duration::from_secs(20),
        "50 guarded appends took {:?} — the choke point is too expensive",
        elapsed
    );
    let text = h.disk("src/lib.rs");
    for i in 0..50 {
        assert!(
            text.contains(&format!("// line {}\n", i)),
            "lost line {}",
            i
        );
    }
    eprintln!(
        "50 guarded appends (incl. snapshot capture) took {:?}",
        elapsed
    );
}

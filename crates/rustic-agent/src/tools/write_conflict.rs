//! Concurrent-write conflict handling: briefing construction, the resolver
//! handoff, and the `ask_user` fallback.
//!
//! Reached only when an agent's edit cannot be replayed against the content
//! that is on disk — either the anchor is gone or a full-content write would
//! clobber someone else's change. Nothing here ever writes silently: a
//! conflict is merged, resolved by an agent, escalated to the user, or
//! reported. In every case the file on disk stays intact until something
//! decides what the merged result should be.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use anyhow::Result;

use super::guarded_write::{truncate_for_prompt, ConflictBriefing, ConflictIntent, GuardedAbort};
use super::write_telemetry;
use super::{ToolContext, ToolOutput};

/// Agent-id prefix marking a conflict resolver. Resolver writes are
/// authoritative: they skip conflict detection so a resolver can't trigger a
/// resolver, and their edits land even though the file diverged (reconciling
/// that divergence is the whole point).
pub(crate) const RESOLVER_PREFIX: &str = "resolver-";

/// Hard cap on resolver attempts for one path within one task.
const MAX_RESOLVER_ATTEMPTS_PER_PATH: u32 = 2;
/// Hard cap on resolver attempts across a whole task, so a pathological run
/// can't fan out resolvers over dozens of files.
const MAX_RESOLVER_ATTEMPTS_PER_TASK: u32 = 12;

pub(crate) fn is_resolver(ctx: &ToolContext) -> bool {
    ctx.subagent_self
        .as_ref()
        .is_some_and(|(_, agent_id)| agent_id.starts_with(RESOLVER_PREFIX))
}

#[derive(Default)]
struct ResolverState {
    /// (task_id, path) currently being resolved.
    in_progress: HashSet<(String, PathBuf)>,
    /// Attempts per (task_id, path).
    per_path: HashMap<(String, PathBuf), u32>,
    /// Attempts per task.
    per_task: HashMap<String, u32>,
}

fn state() -> &'static Mutex<ResolverState> {
    static STATE: OnceLock<Mutex<ResolverState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(ResolverState::default()))
}

/// RAII claim on "this path is being resolved". A second conflict on the same
/// path while a resolver is running gets told to wait instead of spawning a
/// competing resolver.
struct ResolverClaim {
    key: (String, PathBuf),
}

impl ResolverClaim {
    fn try_acquire(task_id: &str, path: &Path) -> Option<Self> {
        let key = (task_id.to_string(), path.to_path_buf());
        let mut st = state().lock().ok()?;
        if !st.in_progress.insert(key.clone()) {
            return None;
        }
        Some(ResolverClaim { key })
    }
}

impl Drop for ResolverClaim {
    fn drop(&mut self) {
        if let Ok(mut st) = state().lock() {
            st.in_progress.remove(&self.key);
        }
    }
}

/// Consume one attempt from the budget. `false` means the budget is spent and
/// the conflict must go to the user instead of another agent.
fn take_attempt(task_id: &str, path: &Path) -> bool {
    let Ok(mut st) = state().lock() else {
        return false;
    };
    let task_total = st.per_task.entry(task_id.to_string()).or_insert(0);
    if *task_total >= MAX_RESOLVER_ATTEMPTS_PER_TASK {
        return false;
    }
    let key = (task_id.to_string(), path.to_path_buf());
    let per_path = st.per_path.entry(key).or_insert(0);
    if *per_path >= MAX_RESOLVER_ATTEMPTS_PER_PATH {
        return false;
    }
    *per_path += 1;
    let task_total = st.per_task.entry(task_id.to_string()).or_insert(0);
    *task_total += 1;
    true
}

#[cfg(test)]
pub(crate) fn reset_resolver_state() {
    if let Ok(mut st) = state().lock() {
        *st = ResolverState::default();
    }
}

/// Unified diff of what changed underneath an agent, base → disk.
pub(crate) fn unified_diff(base: &str, disk: &str, path: &str) -> String {
    let diff = similar::TextDiff::from_lines(base, disk);
    let mut out = String::new();
    for hunk in diff.unified_diff().context_radius(3).iter_hunks() {
        out.push_str(&hunk.to_string());
    }
    if out.is_empty() {
        format!("(no textual difference detected in {})", path)
    } else {
        out
    }
}

/// Where the symbols named in a stale anchor live now, according to the symbol
/// index. Turns "your anchor is gone" into "it looks like it moved to X:120".
pub(crate) fn relocation_hints(ctx: &ToolContext, anchor: &str) -> Vec<String> {
    let index = ctx.workspace_services.symbol_index();
    let mut seen: HashSet<String> = HashSet::new();
    let mut hints = Vec::new();
    for ident in identifiers(anchor) {
        if !seen.insert(ident.clone()) {
            continue;
        }
        for entry in index.find(&ident, None, 3) {
            let file = entry
                .file
                .strip_prefix(&ctx.project_root)
                .unwrap_or(&entry.file)
                .to_string_lossy()
                .replace('\\', "/");
            hints.push(format!("{} → {}:{}", ident, file, entry.line));
            if hints.len() >= 12 {
                return hints;
            }
        }
    }
    hints
}

/// Identifier-shaped tokens in a chunk of source, longest first — the long
/// ones are the distinctive names worth looking up.
fn identifiers(text: &str) -> Vec<String> {
    let mut out: Vec<String> = text
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|t| t.len() >= 4 && !t.chars().next().is_some_and(|c| c.is_numeric()))
        .map(|t| t.to_string())
        .collect();
    out.sort_by_key(|t| std::cmp::Reverse(t.len()));
    out.truncate(24);
    out
}

impl ConflictBriefing {
    /// The prompt handed to a resolver agent. States the competing intents, the
    /// merged text when there is one, and exactly what "done" means.
    pub(crate) fn resolver_prompt(&self) -> String {
        let mut s = String::new();
        s.push_str(&format!(
            "You are a CONFLICT RESOLVER. Two agents changed `{}` at the same time and their \
             changes could not be combined automatically. Reconcile them so BOTH intents \
             survive, then write the file.\n\n",
            self.display_path
        ));

        match &self.intent {
            ConflictIntent::StaleAnchor {
                old_string,
                new_string,
                replace_all,
            } => {
                s.push_str(
                    "## Intent A — the edit that could not be applied\n\nThe agent wanted to \
                     replace this text:\n\n```\n",
                );
                s.push_str(&truncate_for_prompt(old_string, 3000));
                s.push_str("\n```\n\nwith this text:\n\n```\n");
                s.push_str(&truncate_for_prompt(new_string, 3000));
                s.push_str("\n```\n");
                if *replace_all {
                    s.push_str("\n(It was a replace-all edit — every occurrence.)\n");
                }
                s.push_str(
                    "\nThat anchor text is no longer present in the file: another agent \
                     rewrote that region while this edit was being prepared.\n\n",
                );
            }
        }

        s.push_str("## Intent B — what changed underneath (base → current on disk)\n\n```diff\n");
        s.push_str(&truncate_for_prompt(&self.underneath_diff, 6000));
        s.push_str("\n```\n\n");

        if let Some(merged) = &self.merged_text {
            s.push_str(&format!(
                "## Machine merge attempt (status: {})\n\nA 3-way merge produced the text below. \
                 Conflict markers (`<<<<<<<` / `=======` / `>>>>>>>`) mark the regions it could \
                 not decide. Use it as a starting point — verify it, resolve every marker, and \
                 remove all markers before writing.\n\n```\n",
                self.merge_status.as_deref().unwrap_or("unknown")
            ));
            s.push_str(&truncate_for_prompt(merged, 12000));
            s.push_str("\n```\n\n");
        }

        if !self.defects.is_empty() {
            s.push_str(&format!(
                "## Structural defects reported by the merge layer\n\n{}\n\n\
                 `dangling:<name>` means the merged text references a symbol nothing defines \
                 any more — a hidden conflict that compiles-in-two-halves but breaks together. \
                 `duplicate:<name>` means both sides added the same declaration. \
                 `arity:<name>` means a call site disagrees with the definition's parameter \
                 count. `unchecked:<check>` means that check could not run for this language — \
                 verify it yourself by reading the code.\n\n",
                self.defects
                    .iter()
                    .map(|d| format!("- {}", d))
                    .collect::<Vec<_>>()
                    .join("\n")
            ));
        }

        if !self.relocation_hints.is_empty() {
            s.push_str(&format!(
                "## Where those symbols live now (from the symbol index)\n\n{}\n\n",
                self.relocation_hints
                    .iter()
                    .map(|h| format!("- {}", h))
                    .collect::<Vec<_>>()
                    .join("\n")
            ));
        }

        s.push_str(&format!(
            "## What to do\n\n\
             1. `read_file` on `{}` to see its current state — it is unchanged, nothing was \
             written.\n\
             2. Work out how to express Intent A against the code that is there now. The other \
             agent's change stays; yours has to be re-expressed on top of it, not instead of \
             it.\n\
             3. Write the reconciled result with `edit_file`. Your writes are authoritative — \
             they will not be re-checked for conflicts.\n\
             4. Do NOT leave conflict markers in the file. Do NOT commit to git. Do NOT change \
             anything unrelated to these two intents.\n\
             5. Reply with one short paragraph: what each side wanted, and how you combined \
             them. If you genuinely cannot reconcile them, say so explicitly and leave the file \
             untouched.\n",
            self.display_path
        ));
        s
    }
}

/// Handle a guarded-write abort: pass plain outcomes straight through, and put
/// real conflicts through the resolver → user → report ladder.
pub(crate) async fn resolve_or_report(
    ctx: &ToolContext,
    abort: GuardedAbort,
) -> Result<ToolOutput> {
    let briefing = match abort {
        GuardedAbort::Output(o) => return Ok(o),
        GuardedAbort::Conflict(b) => b,
    };

    write_telemetry::record(
        ctx,
        &briefing.abs_path,
        WriteToolLabel(briefing.tool).into(),
        write_telemetry::Event::ConflictEscalated,
    );

    // A resolver builds its provider from the parent task's provider config,
    // which only the main agent carries. A sub-agent that hits a conflict gets
    // the briefing directly — it can re-read and redo the edit itself.
    if ctx.agent_depth > 0 || ctx.parent_provider_config.is_none() {
        return Ok(ToolOutput::text(briefing.unresolved_message(), true));
    }

    let Some(_claim) = ResolverClaim::try_acquire(&ctx.task_id, &briefing.abs_path) else {
        return Ok(ToolOutput::text(
            format!(
                "WRITE_CONFLICT_RESOLVING: '{}' is already being reconciled by another resolver. \
                 Nothing was written. Wait for that to finish, then re-read the file and check \
                 whether your change is still needed.",
                briefing.display_path
            ),
            true,
        ));
    };

    if !take_attempt(&ctx.task_id, &briefing.abs_path) {
        return ask_user_fallback(ctx, &briefing, "resolver budget exhausted for this path").await;
    }

    let prompt = briefing.resolver_prompt();
    let name = format!("{}conflict {}", RESOLVER_PREFIX, briefing.display_path);
    match super::subagent_tools::spawn_system_subagent(ctx, &name, &prompt, &briefing.display_path)
        .await
    {
        Ok(summary) => {
            write_telemetry::record(
                ctx,
                &briefing.abs_path,
                WriteToolLabel(briefing.tool).into(),
                write_telemetry::Event::ResolverResolved,
            );
            Ok(ToolOutput::text(
                format!(
                    "WRITE_CONFLICT_RESOLVED: '{}' changed under you mid-turn, so a resolver \
                     agent reconciled your change with the other agent's. Re-read the file \
                     before editing it again.\n\nResolver report:\n{}",
                    briefing.display_path, summary
                ),
                false,
            ))
        }
        Err(e) => ask_user_fallback(ctx, &briefing, &e.to_string()).await,
    }
}

/// D4: a failed or unaffordable resolver asks the user rather than hard-failing
/// the write. The file is still untouched at this point.
async fn ask_user_fallback(
    ctx: &ToolContext,
    briefing: &ConflictBriefing,
    reason: &str,
) -> Result<ToolOutput> {
    let questions = serde_json::json!([{
        "id": "write_conflict",
        "text": format!(
            "'{}' was changed by two agents at once and I couldn't reconcile it automatically \
             ({}). Nothing has been written — the file on disk is intact.\n\nWhat changed \
             underneath:\n{}\n\nHow should I proceed?",
            briefing.display_path,
            reason,
            truncate_for_prompt(&briefing.underneath_diff, 2000)
        ),
        "kind": "single",
        "options": [
            "Keep the file as it is — drop my change",
            "Let me look at it myself, pause here",
            "Retry: re-read the file and redo the change"
        ]
    }]);

    let answer = ctx
        .ask_user_broker
        .request(&ctx.event_tx, &ctx.task_id, questions)
        .await;

    let choice = answer
        .as_ref()
        .and_then(|r| r.answers.get("write_conflict"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    Ok(ToolOutput::text(
        format!(
            "WRITE_CONFLICT_UNRESOLVED: '{}' — {}. Nothing was written.\nUser's decision: {}",
            briefing.display_path,
            reason,
            if choice.is_empty() {
                "no answer (dialog dismissed or timed out) — treat the change as not applied"
            } else {
                choice
            }
        ),
        true,
    ))
}

/// Bridge the briefing's `&'static str` tool label back to the telemetry enum.
struct WriteToolLabel(&'static str);

impl From<WriteToolLabel> for super::guarded_write::WriteTool {
    fn from(label: WriteToolLabel) -> Self {
        use super::guarded_write::WriteTool;
        match label.0 {
            "create_file" => WriteTool::CreateFile,
            "edit_notebook" => WriteTool::EditNotebook,
            "move_file" => WriteTool::MoveFile,
            "resolver" => WriteTool::Resolver,
            _ => WriteTool::EditFile,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::ToolContext;

    /// The resolver budget lives in process-wide state, so the tests that
    /// reset and consume it must not run concurrently with each other.
    fn budget_guard() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        let lock = LOCK.get_or_init(|| Mutex::new(()));
        match lock.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn ctx_with_agent(root: &Path, agent_id: Option<&str>) -> ToolContext {
        let (mut ctx, rx) = ToolContext::new_test(root.to_path_buf());
        // Keep the receiver alive for the context's lifetime: a closed channel
        // makes every event emission fail, which is not what we're testing.
        std::mem::forget(rx);
        ctx.subagent_self = agent_id.map(|id| ("parent-task".to_string(), id.to_string()));
        ctx
    }

    #[test]
    fn resolver_agents_are_recognised_by_their_id_prefix() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!is_resolver(&ctx_with_agent(dir.path(), None)));
        assert!(!is_resolver(&ctx_with_agent(
            dir.path(),
            Some("explorer-1234")
        )));
        assert!(is_resolver(&ctx_with_agent(
            dir.path(),
            Some("resolver-src/lib.rs")
        )));
    }

    #[test]
    fn per_path_budget_is_capped_and_other_paths_are_unaffected() {
        let _g = budget_guard();
        reset_resolver_state();
        let a = Path::new("/p/a.rs");
        let b = Path::new("/p/b.rs");
        for i in 0..MAX_RESOLVER_ATTEMPTS_PER_PATH {
            assert!(take_attempt("t1", a), "attempt {i} should be granted");
        }
        assert!(
            !take_attempt("t1", a),
            "third attempt on the same path must be refused"
        );
        assert!(
            take_attempt("t1", b),
            "a different path has its own per-path budget"
        );
    }

    #[test]
    fn task_wide_budget_caps_resolver_fan_out_across_paths() {
        let _g = budget_guard();
        reset_resolver_state();
        let mut granted = 0;
        // One attempt per distinct path, so only the task-wide cap can bite.
        for i in 0..(MAX_RESOLVER_ATTEMPTS_PER_TASK * 2) {
            let p = PathBuf::from(format!("/p/f{i}.rs"));
            if take_attempt("t2", &p) {
                granted += 1;
            }
        }
        assert_eq!(granted, MAX_RESOLVER_ATTEMPTS_PER_TASK);
    }

    #[test]
    fn budgets_are_scoped_per_task() {
        let _g = budget_guard();
        reset_resolver_state();
        let a = Path::new("/p/a.rs");
        for _ in 0..MAX_RESOLVER_ATTEMPTS_PER_PATH {
            assert!(take_attempt("t3", a));
        }
        assert!(!take_attempt("t3", a));
        assert!(
            take_attempt("t4", a),
            "another task must not inherit t3's spent budget"
        );
    }

    #[test]
    fn a_second_conflict_on_a_path_under_resolution_cannot_claim_it() {
        let _g = budget_guard();
        reset_resolver_state();
        let p = Path::new("/p/busy.rs");
        let first = ResolverClaim::try_acquire("t5", p).expect("first claim");
        assert!(
            ResolverClaim::try_acquire("t5", p).is_none(),
            "a competing resolver must not be spawned for the same path"
        );
        assert!(
            ResolverClaim::try_acquire("t5", Path::new("/p/free.rs")).is_some(),
            "a different path stays claimable"
        );
        drop(first);
        assert!(
            ResolverClaim::try_acquire("t5", p).is_some(),
            "the claim must be released on drop"
        );
    }

    #[test]
    fn unified_diff_reports_changes_and_says_so_when_there_are_none() {
        let d = unified_diff("a\nb\n", "a\nc\n", "src/x.rs");
        assert!(d.contains("-b"), "{d}");
        assert!(d.contains("+c"), "{d}");
        assert!(unified_diff("same\n", "same\n", "src/x.rs").contains("no textual difference"));
    }

    #[test]
    fn identifiers_prefers_long_distinctive_names() {
        let ids = identifiers("fn compute_widget_total(x: i32) { let n = 1; }");
        assert_eq!(
            ids.first().map(String::as_str),
            Some("compute_widget_total")
        );
        assert!(!ids.iter().any(|i| i == "x" || i == "n"));
    }
}

//! P1.8 — Goal loop (`/goal` mode).
//!
//! A goal-loop task runs `TaskExecutor::run_turn` repeatedly against the
//! same message thread until either:
//!
//! 1. The model calls the `goal_complete` builtin tool — i.e. it
//!    explicitly declares the user's goal achieved. Loop exits with the
//!    summary supplied in that call (or the model's last assistant text
//!    if no summary was given).
//! 2. The configured iteration cap fires. Loop exits with a failure-ish
//!    result so the UI can offer a "continue / stop" choice.
//! 3. The user cancels the task via the shared cancel token. Same as the
//!    inner executor's cancel handling.
//!
//! Mechanically the loop just keeps appending a one-line nudge message
//! ("Goal not yet marked complete — continue.") after every natural
//! `end_turn` and re-invokes `run_turn`. `run_turn` itself doesn't need
//! to know about the loop — the goal-loop layer sits one level above.
//!
//! The wrapper is a separate entry point from `run_turn` so existing
//! call sites (sub-agents, normal chat) keep their current semantics.
//! The host opts into the goal loop via the `set_task_goal_mode` Tauri
//! command, which flips a per-task `is_goal_mode` flag; the next
//! `send_message` dispatch reads the flag and routes through here
//! instead of a single `run_turn`. The flag is cleared automatically
//! once the loop terminates (any branch — Achieved / IterationCapReached
//! / Cancelled / Errored), so a subsequent plain `send_message` won't
//! re-enter the loop. Frontend convention: user types `/goal <objective>`
//! (optionally `/goal:N <objective>` to override the iteration cap).

use crate::provider::{ContentBlock, Message, Role};
use crate::task::cost::TaskCost;
use crate::task::executor::TaskExecutor;
use crate::tools::ToolContext;
use anyhow::Result;
use async_trait::async_trait;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Default iteration cap for `/goal`. Picked to be high enough for
/// nontrivial multi-step goals (refactors, test-greening loops) but
/// low enough that a runaway loop hits the wall in minutes, not hours.
pub const DEFAULT_GOAL_ITERATION_CAP: u32 = 50;

/// Outcome a host needs to know from the goal-loop run.
#[derive(Debug, Clone)]
pub struct GoalLoopOutcome {
    /// Sum of `TaskCost` across every inner `run_turn` invocation.
    pub task_cost: TaskCost,
    /// How many turns the goal loop drove through `run_turn`. Useful for
    /// telemetry — does NOT necessarily equal the model's per-turn count
    /// because `run_turn` itself may make many provider calls within one
    /// outer turn.
    pub iterations: u32,
    /// Why the loop ended.
    pub termination: GoalTermination,
    /// `goal_complete`'s `summary` arg, or the model's final assistant text
    /// when no explicit summary was supplied.
    pub summary: Option<String>,
}

#[derive(Debug, Clone)]
pub enum GoalTermination {
    /// Model called `goal_complete`.
    Achieved,
    /// Iteration cap hit before `goal_complete` was called.
    IterationCapReached(u32),
    /// User cancel token flipped.
    Cancelled,
    /// Inner executor returned an error.
    Errored(String),
}

/// Run the goal loop. `messages` already contains the user's goal as its
/// initial entry; the wrapper appends nudge / completion messages as it
/// iterates and returns the same Vec the caller passed in.
pub async fn run_goal_loop(
    executor: &TaskExecutor,
    messages: &mut Vec<Message>,
    context: &ToolContext,
    iteration_cap: u32,
) -> Result<GoalLoopOutcome> {
    let mut runner = ExecutorTurnRunner { executor, context };
    Ok(run_goal_loop_inner(
        messages,
        context.cancel_token.as_ref(),
        iteration_cap,
        &mut runner,
    )
    .await)
}

/// One iteration's worth of model interaction. Production wires this to
/// `TaskExecutor::run_turn`; tests implement it with a stub that pushes
/// canned messages so every termination branch of `run_goal_loop` can be
/// exercised without building a real provider + ToolContext. The trait
/// is `async_trait` rather than a bare closure because the closure-with-
/// borrowed-args pattern requires HRTB ceremony that doesn't compose
/// with the production callsite's borrowed executor and context.
#[async_trait]
pub(crate) trait TurnRunner: Send {
    async fn run(&mut self, messages: &mut Vec<Message>) -> Result<TaskCost>;
}

/// Production runner: delegates to `TaskExecutor::run_turn`.
struct ExecutorTurnRunner<'a> {
    executor: &'a TaskExecutor,
    context: &'a ToolContext,
}

#[async_trait]
impl TurnRunner for ExecutorTurnRunner<'_> {
    async fn run(&mut self, messages: &mut Vec<Message>) -> Result<TaskCost> {
        self.executor.run_turn(messages, self.context).await
    }
}

/// The TaskExecutor-free heart of the goal loop. Takes a `TurnRunner`
/// that simulates one outer iteration's effect on `messages`. All other
/// state — cap check, cancel check, goal_complete scan, nudge injection
/// — lives here so tests can exercise every termination branch by
/// supplying a stub runner.
pub(crate) async fn run_goal_loop_inner(
    messages: &mut Vec<Message>,
    cancel_token: Option<&Arc<AtomicBool>>,
    iteration_cap: u32,
    turn_runner: &mut dyn TurnRunner,
) -> GoalLoopOutcome {
    let cap = if iteration_cap == 0 {
        DEFAULT_GOAL_ITERATION_CAP
    } else {
        iteration_cap
    };

    let mut total_cost = TaskCost::default();
    let mut iterations: u32 = 0;

    loop {
        if let Some(token) = cancel_token {
            if token.load(Ordering::SeqCst) {
                return GoalLoopOutcome {
                    task_cost: total_cost,
                    iterations,
                    termination: GoalTermination::Cancelled,
                    summary: last_assistant_text(messages),
                };
            }
        }

        // Iteration cap check is BEFORE the run_turn call so the
        // last successful iteration always lands cleanly even when the
        // cap is hit exactly on its boundary.
        if iterations >= cap {
            return GoalLoopOutcome {
                task_cost: total_cost,
                iterations,
                termination: GoalTermination::IterationCapReached(cap),
                summary: last_assistant_text(messages),
            };
        }

        let pre_turn_len = messages.len();
        let turn_result = turn_runner.run(messages).await;
        iterations = iterations.saturating_add(1);

        let cost = match turn_result {
            Ok(c) => c,
            Err(e) => {
                return GoalLoopOutcome {
                    task_cost: total_cost,
                    iterations,
                    termination: GoalTermination::Errored(e.to_string()),
                    summary: last_assistant_text(messages),
                };
            }
        };
        total_cost.merge_into(&cost);

        // Walk the messages appended this iteration looking for a
        // `goal_complete` tool_use. The executor logs every tool call
        // into the assistant Message that issued it, so we can find
        // them deterministically without poking the registry.
        if let Some(summary) = find_goal_complete(&messages[pre_turn_len..]) {
            return GoalLoopOutcome {
                task_cost: total_cost,
                iterations,
                termination: GoalTermination::Achieved,
                summary: Some(summary).filter(|s| !s.is_empty()).or_else(|| last_assistant_text(messages)),
            };
        }

        // Model ended its turn without marking the goal done. Push a
        // user-message nudge so the next iteration has fresh stimulus.
        let nudge = format!(
            "[GOAL LOOP — iteration {}/{}] The objective hasn't been marked complete yet. \
             Continue working toward the goal. When (and ONLY when) the user's stated \
             objective is fully achieved, call the `goal_complete` tool with a short \
             `summary` describing what was done. If you've hit a genuine blocker, write \
             a plain-text explanation and call `goal_complete` with that as the summary \
             — don't loop forever.",
            iterations, cap,
        );
        messages.push(Message {
            role: Role::User,
            content: vec![ContentBlock::Text { text: nudge }],
        });
    }
}

/// Find a `goal_complete` tool_use in the message slice; return its
/// `summary` argument (empty string when the tool was called with no
/// summary, `None` when the tool wasn't called).
fn find_goal_complete(messages: &[Message]) -> Option<String> {
    for m in messages.iter().rev() {
        if !matches!(m.role, Role::Assistant) {
            continue;
        }
        for block in &m.content {
            if let ContentBlock::ToolUse { name, input, .. } = block {
                if name == "goal_complete" {
                    let summary = input
                        .get("summary")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    return Some(summary);
                }
            }
        }
    }
    None
}

/// Pick the most recent assistant text block — used as a fallback
/// summary when the model didn't supply one explicitly.
fn last_assistant_text(messages: &[Message]) -> Option<String> {
    for m in messages.iter().rev() {
        if !matches!(m.role, Role::Assistant) {
            continue;
        }
        let mut chunks: Vec<String> = Vec::new();
        for block in &m.content {
            if let ContentBlock::Text { text } = block {
                let t = text.trim();
                if !t.is_empty() {
                    chunks.push(t.to_string());
                }
            }
        }
        if !chunks.is_empty() {
            return Some(chunks.join("\n\n"));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn assistant_with_goal_complete(summary: &str) -> Message {
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "tu_1".into(),
                name: "goal_complete".into(),
                input: json!({ "summary": summary }),
                thought_signature: None,
            }],
        }
    }

    #[test]
    fn find_goal_complete_with_summary() {
        let msgs = vec![
            Message { role: Role::User, content: vec![ContentBlock::Text { text: "x".into() }] },
            assistant_with_goal_complete("done"),
        ];
        assert_eq!(find_goal_complete(&msgs), Some("done".to_string()));
    }

    #[test]
    fn find_goal_complete_returns_empty_for_no_summary() {
        let msgs = vec![Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "tu_1".into(),
                name: "goal_complete".into(),
                input: json!({}),
                thought_signature: None,
            }],
        }];
        assert_eq!(find_goal_complete(&msgs), Some(String::new()));
    }

    #[test]
    fn find_goal_complete_misses_when_not_called() {
        let msgs = vec![Message {
            role: Role::Assistant,
            content: vec![ContentBlock::Text { text: "hello".into() }],
        }];
        assert_eq!(find_goal_complete(&msgs), None);
    }

    #[test]
    fn last_assistant_text_pulls_most_recent() {
        let msgs = vec![
            Message { role: Role::Assistant, content: vec![ContentBlock::Text { text: "first".into() }] },
            Message { role: Role::User, content: vec![ContentBlock::Text { text: "u".into() }] },
            Message { role: Role::Assistant, content: vec![ContentBlock::Text { text: "second".into() }] },
        ];
        assert_eq!(last_assistant_text(&msgs).as_deref(), Some("second"));
    }

    // C6.7 — additional goal_loop coverage focused on the helpers and
    // outcome construction. A full `run_goal_loop` invocation requires a
    // real `TaskExecutor` + provider (heavy); these unit tests target the
    // decision logic the loop relies on.

    #[test]
    fn find_goal_complete_picks_the_last_call_when_multiple_assistants() {
        let msgs = vec![
            assistant_with_goal_complete("first attempt"),
            Message { role: Role::User, content: vec![ContentBlock::Text { text: "retry".into() }] },
            assistant_with_goal_complete("final attempt"),
        ];
        // We iterate in reverse, so the *most recent* call wins.
        assert_eq!(find_goal_complete(&msgs), Some("final attempt".to_string()));
    }

    #[test]
    fn find_goal_complete_skips_user_messages_with_matching_tool_results() {
        // A `ToolResult` block on a User message is NOT a `goal_complete`
        // tool_use; the scan must skip it. (User messages get ToolResult
        // blocks containing the tool's output, but find_goal_complete
        // intentionally looks only at Assistant role.)
        let msgs = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "tu_1".into(),
                content: "fake result mentioning goal_complete".into(),
                is_error: false,
            }],
        }];
        assert_eq!(find_goal_complete(&msgs), None);
    }

    #[test]
    fn last_assistant_text_concatenates_multiple_text_blocks() {
        let msgs = vec![Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::Text { text: "para one".into() },
                ContentBlock::Text { text: "para two".into() },
            ],
        }];
        let got = last_assistant_text(&msgs);
        assert_eq!(got.as_deref(), Some("para one\n\npara two"));
    }

    #[test]
    fn last_assistant_text_returns_none_when_only_tool_uses() {
        let msgs = vec![Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "tu_1".into(),
                name: "read_file".into(),
                input: json!({}),
                thought_signature: None,
            }],
        }];
        assert_eq!(last_assistant_text(&msgs), None);
    }

    #[test]
    fn last_assistant_text_trims_whitespace_only_blocks() {
        let msgs = vec![Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::Text { text: "  ".into() },
                ContentBlock::Text { text: "real content".into() },
            ],
        }];
        assert_eq!(last_assistant_text(&msgs).as_deref(), Some("real content"));
    }

    #[test]
    fn outcome_iteration_cap_default_when_zero_supplied() {
        // The wrapper resolves cap=0 → DEFAULT_GOAL_ITERATION_CAP. We can't
        // exercise the whole run_goal_loop without an executor, but the
        // constant itself is part of the public contract.
        assert!(DEFAULT_GOAL_ITERATION_CAP >= 10, "default cap must allow multi-step work");
        assert!(DEFAULT_GOAL_ITERATION_CAP <= 200, "default cap must avoid runaway costs");
    }

    #[test]
    fn outcome_struct_round_trip_fields() {
        // Sanity-check the outcome shape is what callers depend on.
        let outcome = GoalLoopOutcome {
            task_cost: TaskCost::default(),
            iterations: 7,
            termination: GoalTermination::IterationCapReached(50),
            summary: Some("hit cap".into()),
        };
        assert_eq!(outcome.iterations, 7);
        match outcome.termination {
            GoalTermination::IterationCapReached(c) => assert_eq!(c, 50),
            other => panic!("unexpected: {:?}", other),
        }
        assert_eq!(outcome.summary.as_deref(), Some("hit cap"));
    }

    #[test]
    fn termination_variants_carry_their_payloads() {
        let cap = GoalTermination::IterationCapReached(42);
        match cap {
            GoalTermination::IterationCapReached(c) => assert_eq!(c, 42),
            _ => panic!(),
        }
        let err = GoalTermination::Errored("boom".into());
        match err {
            GoalTermination::Errored(msg) => assert_eq!(msg, "boom"),
            _ => panic!(),
        }
    }

    // C6.6 — E2E loop coverage via `run_goal_loop_inner`. We stub the
    // per-iteration runner to push specific message shapes and assert
    // the outer loop reaches the right termination + summary + nudge
    // behaviour.

    fn user_msg(text: &str) -> Message {
        Message {
            role: Role::User,
            content: vec![ContentBlock::Text { text: text.into() }],
        }
    }

    fn assistant_text_msg(text: &str) -> Message {
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::Text { text: text.into() }],
        }
    }

    fn count_nudges(messages: &[Message]) -> usize {
        messages
            .iter()
            .filter(|m| {
                matches!(m.role, Role::User)
                    && m.content.iter().any(|c| {
                        matches!(c, ContentBlock::Text { text } if text.contains("[GOAL LOOP — iteration"))
                    })
            })
            .count()
    }

    /// Test runner that delegates each call to a user-supplied closure.
    /// The closure is sync — tests don't need awaits inside, and skipping
    /// the async lifetime dance keeps the test bodies readable.
    struct FnRunner<F: FnMut(u32, &mut Vec<Message>) -> Result<TaskCost> + Send> {
        f: F,
        call: u32,
    }

    impl<F: FnMut(u32, &mut Vec<Message>) -> Result<TaskCost> + Send> FnRunner<F> {
        fn new(f: F) -> Self {
            Self { f, call: 0 }
        }
    }

    #[async_trait]
    impl<F: FnMut(u32, &mut Vec<Message>) -> Result<TaskCost> + Send> TurnRunner for FnRunner<F> {
        async fn run(&mut self, messages: &mut Vec<Message>) -> Result<TaskCost> {
            self.call += 1;
            (self.f)(self.call, messages)
        }
    }

    #[tokio::test]
    async fn loop_returns_achieved_when_runner_emits_goal_complete() {
        let mut messages = vec![user_msg("write three tests")];
        let mut runner = FnRunner::new(|_call, msgs| {
            msgs.push(assistant_text_msg("Wrote them."));
            msgs.push(assistant_with_goal_complete("done — three tests added"));
            Ok(TaskCost::default())
        });
        let outcome = run_goal_loop_inner(&mut messages, None, 10, &mut runner).await;

        match outcome.termination {
            GoalTermination::Achieved => {}
            other => panic!("expected Achieved, got {:?}", other),
        }
        assert_eq!(outcome.iterations, 1);
        assert_eq!(outcome.summary.as_deref(), Some("done — three tests added"));
        // No nudge should be appended since the goal completed on the first iteration.
        assert_eq!(count_nudges(&messages), 0);
    }

    #[tokio::test]
    async fn loop_falls_back_to_last_assistant_text_when_goal_complete_has_empty_summary() {
        let mut messages = vec![user_msg("do it")];
        let mut runner = FnRunner::new(|_call, msgs| {
            msgs.push(assistant_text_msg("Working on it…"));
            msgs.push(assistant_text_msg("All set. Final notes here."));
            msgs.push(Message {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: "tu_1".into(),
                    name: "goal_complete".into(),
                    input: json!({}), // empty summary
                    thought_signature: None,
                }],
            });
            Ok(TaskCost::default())
        });
        let outcome = run_goal_loop_inner(&mut messages, None, 5, &mut runner).await;

        match outcome.termination {
            GoalTermination::Achieved => {}
            other => panic!("expected Achieved, got {:?}", other),
        }
        // Empty summary falls back to the last assistant text.
        assert_eq!(outcome.summary.as_deref(), Some("All set. Final notes here."));
    }

    #[tokio::test]
    async fn loop_returns_iteration_cap_reached_when_goal_never_completes() {
        let mut messages = vec![user_msg("infinite goal")];
        let mut runner = FnRunner::new(|call, msgs| {
            msgs.push(assistant_text_msg(&format!(
                "Iteration {} progress note.",
                call
            )));
            Ok(TaskCost::default())
        });
        let outcome = run_goal_loop_inner(&mut messages, None, 3, &mut runner).await;

        match outcome.termination {
            GoalTermination::IterationCapReached(cap) => assert_eq!(cap, 3),
            other => panic!("expected IterationCapReached, got {:?}", other),
        }
        assert_eq!(outcome.iterations, 3);
        // Two nudges should have landed (one between each of the 3 iterations,
        // but the last iteration's nudge isn't pushed because cap fires first).
        // Actually the loop pushes a nudge AFTER each runner call that didn't
        // hit goal_complete, including the final one — so we expect 3 nudges,
        // one for each iteration we ran without finishing.
        assert_eq!(
            count_nudges(&messages),
            3,
            "one nudge per iteration that didn't complete",
        );
    }

    #[tokio::test]
    async fn loop_uses_default_cap_when_iteration_cap_is_zero() {
        let mut messages = vec![user_msg("test")];
        let mut runner = FnRunner::new(|call, msgs| {
            if call == 2 {
                msgs.push(assistant_with_goal_complete("done on 2nd"));
            } else {
                msgs.push(assistant_text_msg("still working"));
            }
            Ok(TaskCost::default())
        });
        let outcome = run_goal_loop_inner(&mut messages, None, 0, &mut runner).await;
        match outcome.termination {
            GoalTermination::Achieved => {}
            other => panic!("expected Achieved with cap=0 default, got {:?}", other),
        }
        assert_eq!(outcome.iterations, 2);
    }

    #[tokio::test]
    async fn loop_returns_cancelled_when_token_already_set_before_first_iteration() {
        let cancel = Arc::new(AtomicBool::new(true));
        let mut messages = vec![user_msg("test")];
        let mut runner = FnRunner::new(|_call, _msgs| Ok(TaskCost::default()));
        let outcome =
            run_goal_loop_inner(&mut messages, Some(&cancel), 10, &mut runner).await;
        match outcome.termination {
            GoalTermination::Cancelled => {}
            other => panic!("expected Cancelled, got {:?}", other),
        }
        assert_eq!(outcome.iterations, 0, "no iterations should run after pre-cancel");
    }

    #[tokio::test]
    async fn loop_returns_cancelled_when_token_set_mid_run() {
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_for_runner = Arc::clone(&cancel);
        let mut messages = vec![user_msg("test")];
        let mut runner = FnRunner::new(move |call, msgs| {
            msgs.push(assistant_text_msg(&format!("turn {}", call)));
            // Cancel after the second iteration's runner returns. The
            // loop's NEXT cancel check (top of iteration 3) should fire.
            if call == 2 {
                cancel_for_runner.store(true, Ordering::SeqCst);
            }
            Ok(TaskCost::default())
        });
        let outcome =
            run_goal_loop_inner(&mut messages, Some(&cancel), 100, &mut runner).await;
        match outcome.termination {
            GoalTermination::Cancelled => {}
            other => panic!("expected Cancelled, got {:?}", other),
        }
        assert_eq!(outcome.iterations, 2, "should have run exactly 2 iterations before cancel fired");
        // The last-assistant-text fallback should pick up turn 2's text.
        assert_eq!(outcome.summary.as_deref(), Some("turn 2"));
    }

    #[tokio::test]
    async fn loop_returns_errored_when_runner_returns_err() {
        let mut messages = vec![user_msg("test")];
        let mut runner = FnRunner::new(|_call, _msgs| {
            Err(anyhow::anyhow!("simulated provider failure"))
        });
        let outcome = run_goal_loop_inner(&mut messages, None, 10, &mut runner).await;
        match &outcome.termination {
            GoalTermination::Errored(msg) => {
                assert!(msg.contains("simulated provider failure"));
            }
            other => panic!("expected Errored, got {:?}", other),
        }
        assert_eq!(outcome.iterations, 1);
    }

    #[tokio::test]
    async fn loop_accumulates_cost_across_iterations() {
        let mut messages = vec![user_msg("test")];
        let mut runner = FnRunner::new(|call, msgs| {
            let mut c = TaskCost::default();
            c.total_input_tokens = (call * 100) as u64;
            c.estimated_cost_usd = (call as f64) * 0.001;
            if call == 3 {
                msgs.push(assistant_with_goal_complete("done"));
            } else {
                msgs.push(assistant_text_msg("progress"));
            }
            Ok(c)
        });
        let outcome = run_goal_loop_inner(&mut messages, None, 5, &mut runner).await;
        match outcome.termination {
            GoalTermination::Achieved => {}
            other => panic!("expected Achieved, got {:?}", other),
        }
        assert_eq!(outcome.iterations, 3);
        // 100 + 200 + 300
        assert_eq!(outcome.task_cost.total_input_tokens, 600);
        // 0.001 + 0.002 + 0.003 — float tolerance.
        assert!((outcome.task_cost.estimated_cost_usd - 0.006).abs() < 1e-9);
    }

    #[tokio::test]
    async fn loop_injects_nudge_with_iteration_marker_after_non_completing_turn() {
        // Verify the nudge text shape — agent prompts depend on it.
        let mut messages = vec![user_msg("g")];
        let mut runner = FnRunner::new(|call, msgs| {
            msgs.push(assistant_text_msg("hmm"));
            if call == 3 {
                msgs.push(assistant_with_goal_complete("k"));
            }
            Ok(TaskCost::default())
        });
        let _outcome = run_goal_loop_inner(&mut messages, None, 5, &mut runner).await;
        // We expect exactly 2 nudges (after iterations 1 and 2 — iteration 3
        // completed, so no nudge after it).
        let nudge_msgs: Vec<&Message> = messages
            .iter()
            .filter(|m| {
                matches!(m.role, Role::User)
                    && m.content.iter().any(|c| {
                        matches!(c, ContentBlock::Text { text } if text.contains("[GOAL LOOP — iteration"))
                    })
            })
            .collect();
        assert_eq!(nudge_msgs.len(), 2);
        // First nudge mentions "iteration 1/5", second "iteration 2/5".
        let first_text = nudge_msgs[0].content.iter().find_map(|c| match c {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        }).unwrap();
        assert!(first_text.contains("iteration 1/5"), "got: {}", first_text);
        let second_text = nudge_msgs[1].content.iter().find_map(|c| match c {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        }).unwrap();
        assert!(second_text.contains("iteration 2/5"), "got: {}", second_text);
    }
}

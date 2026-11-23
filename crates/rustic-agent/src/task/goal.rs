//! /goal loop support: goal state shared with the host, completion-claim
//! detection in assistant output, and the evaluator pass that verifies a
//! claimed completion before the loop is allowed to end. The evaluator runs
//! on the task's own model in an independent session (no history, no tools).

use std::sync::{Arc, Mutex};

use anyhow::Result;

use crate::provider::{AiProvider, ContentBlock, Message, ProviderConfig, Role};

/// Marker the worker model must emit when it believes the goal condition is
/// fully satisfied. Emitting it triggers the evaluator; it never ends the
/// loop by itself.
pub const GOAL_COMPLETE_MARKER: &str = "<goal_complete>";

/// Opening of the marker the worker model emits when the goal cannot be
/// finished by it: `<goal_blocked reason="...">`. It stops the loop without an
/// evaluator pass and hands the reason to the user. The goal stays set until
/// the user sends a message (which clears it) or sets a new /goal.
pub const GOAL_BLOCKED_TAG: &str = "<goal_blocked";

/// What the goal loop should do with the assistant's final message of a turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnEnd {
    /// The worker says it cannot go on; stop and show the reason.
    Blocked(String),
    /// The worker claims the goal is met; audit the claim.
    Claimed,
    /// Neither marker; re-arm the loop.
    Continue,
}

/// Classify the assistant's final text. A blocked marker wins over a
/// completion claim: a worker that reports a blocker has not finished.
pub fn turn_end(text: &str) -> TurnEnd {
    if let Some(reason) = blocked_reason(text) {
        TurnEnd::Blocked(reason)
    } else if claims_completion(text) {
        TurnEnd::Claimed
    } else {
        TurnEnd::Continue
    }
}

/// The reason carried by the last `<goal_blocked ...>` marker in `text`, or
/// `None` when there is no marker. Accepts `reason="..."` in either quote
/// style and the element form `<goal_blocked>reason</goal_blocked>`.
pub fn blocked_reason(text: &str) -> Option<String> {
    const FALLBACK: &str = "The agent reported it cannot finish this goal on its own.";
    let start = text.rfind(GOAL_BLOCKED_TAG)?;
    let rest = &text[start + GOAL_BLOCKED_TAG.len()..];
    // `<goal_blocked_x>` or `<goal_blockedness>` is some other tag.
    if !rest.starts_with(|c: char| c.is_whitespace() || c == '>' || c == '/') {
        return None;
    }
    let clean = |s: &str| {
        let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
        if s.is_empty() {
            FALLBACK.to_string()
        } else {
            s
        }
    };

    let attrs = rest.trim_start();
    if let Some(value) = attrs.strip_prefix("reason").map(str::trim_start) {
        if let Some(value) = value.strip_prefix('=').map(str::trim_start) {
            if let Some(quote) = value.chars().next().filter(|c| *c == '"' || *c == '\'') {
                let body = &value[1..];
                // The reason may itself contain quotes or `>`: it ends at the
                // first quote that is followed by the end of the tag.
                let end = body.match_indices(quote).map(|(i, _)| i).find(|i| {
                    let after = body[i + 1..].trim_start();
                    after.starts_with('>') || after.starts_with("/>")
                });
                let reason = match end {
                    Some(end) => &body[..end],
                    None => body.lines().next().unwrap_or("").trim_end_matches(quote),
                };
                return Some(clean(reason));
            }
        }
    }
    // Element form, or a bare marker with the explanation after it.
    let inner = rest.split_once('>').map_or("", |(_, after)| after);
    let inner = inner.split("</goal_blocked>").next().unwrap_or("");
    Some(clean(
        inner.lines().find(|l| !l.trim().is_empty()).unwrap_or(""),
    ))
}

/// An active /goal on a task: the completion condition plus loop stats.
#[derive(Debug, Clone)]
pub struct GoalState {
    pub condition: String,
    /// Extra turns the goal loop has forced so far (0 on the kickoff turn).
    pub turns: u32,
}

/// Live goal slot shared between the host and the executor. `None` = no goal.
/// The host writes on `/goal <condition>` and `/goal clear`; the executor
/// reads it at every turn end and clears it when the evaluator confirms.
pub type GoalSlot = Arc<Mutex<Option<GoalState>>>;

/// Creates an empty shared goal slot.
pub fn new_goal_slot() -> GoalSlot {
    Arc::new(Mutex::new(None))
}

/// Builds the user-visible kickoff message sent when a goal is set.
pub fn kickoff_message(condition: &str) -> String {
    format!(
        "GOAL MODE — work autonomously until this condition is TRUE:\n\n{condition}\n\n\
         Rules while the goal is active:\n\
         1. Keep working across turns; do not stop to report partial progress.\n\
         2. Verify with real evidence (run the tests / build / command) — the \
         transcript must contain the proof, not just your claim.\n\
         3. Do NOT use ask_user. Decide autonomously; if uncertain, state your \
         assumption in text and continue with the highest-confidence option.\n\
         4. When — and ONLY when — the condition is completely true and verified, \
         output the marker {GOAL_COMPLETE_MARKER} in your final message. An \
         independent evaluator pass audits the transcript; false claims are \
         rejected and cost you a wasted turn.\n\
         5. If what remains cannot be done by you at all (it needs a person, an \
         approval, a credential, or access you do not have), do not keep \
         looping and do not claim completion. Finish everything you can, then \
         end your message with {GOAL_BLOCKED_TAG} reason=\"what is blocking and \
         what the user must do\">. The loop stops and the user is shown the \
         reason; they decide what happens next. Never use it for work that is \
         merely hard or slow."
    )
}

/// One-line reminder of the blocked marker, appended to loop re-prompts.
fn blocked_hint() -> String {
    format!(
        "If the only thing left needs the user or something you cannot do, stop \
         repeating yourself and end with {GOAL_BLOCKED_TAG} reason=\"...\"> instead."
    )
}

/// Builds the synthetic user message that re-arms the loop for another turn.
pub fn continuation_message(condition: &str, evaluator_reason: Option<&str>) -> String {
    match evaluator_reason {
        Some(reason) => format!(
            "[Goal loop — completion claim REJECTED by the evaluator]\n\
             Reason: {reason}\n\n\
             The goal is still active: {condition}\n\n\
             Address the gap above with verifiable evidence, then output \
             {GOAL_COMPLETE_MARKER} again when the condition is truly met. \
             Do not use ask_user; decide autonomously and note assumptions. {}",
            blocked_hint()
        ),
        None => format!(
            "[Goal loop — the goal is still active]\n\
             {condition}\n\n\
             Continue working. Produce verifiable evidence in the transcript \
             (test runs, build output, command results). When the condition is \
             completely true, output {GOAL_COMPLETE_MARKER}. Do not use ask_user; \
             decide autonomously and note assumptions. {}",
            blocked_hint()
        ),
    }
}

/// True when the assistant's final text claims the goal is complete.
pub fn claims_completion(text: &str) -> bool {
    text.contains(GOAL_COMPLETE_MARKER)
}

/// Evaluator verdict on a claimed completion.
#[derive(Debug, Clone)]
pub struct GoalVerdict {
    pub met: bool,
    pub reason: String,
}

const EVALUATOR_SYSTEM_PROMPT: &str = "You are a strict, skeptical completion evaluator. \
You are given a goal condition and the tail of an AI coding agent's session transcript. \
Decide whether the condition is COMPLETELY met based only on evidence present in the \
transcript (test output, build results, command output, file contents). A confident \
claim without supporting evidence is NOT met. Partial completion is NOT met. \
You cannot run commands or read files yourself. \
Respond with ONLY a JSON object, no markdown fences: \
{\"met\": true|false, \"reason\": \"<at most 60 words: if met, cite the evidence; if not met, state exactly what is missing or unproven>\"}";

/// Max transcript characters handed to the evaluator (taken from the end).
const EVALUATOR_TRANSCRIPT_CAP: usize = 40_000;

/// Flattens the tail of the conversation into plain text for the evaluator.
fn transcript_tail(messages: &[Message]) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut total = 0usize;
    for m in messages.iter().rev() {
        let role = match m.role {
            Role::User => "USER",
            Role::Assistant => "ASSISTANT",
            Role::System => "SYSTEM",
        };
        let mut body = String::new();
        for b in &m.content {
            match b {
                ContentBlock::Text { text } => {
                    body.push_str(text);
                    body.push('\n');
                }
                ContentBlock::ToolUse { name, input, .. } => {
                    let args = serde_json::to_string(input).unwrap_or_default();
                    let args_short: String = args.chars().take(400).collect();
                    body.push_str(&format!("[tool call: {name} {args_short}]\n"));
                }
                ContentBlock::ToolResult {
                    content, is_error, ..
                } => {
                    let flag = if *is_error { " (ERROR)" } else { "" };
                    body.push_str(&format!("[tool result{flag}]: {content}\n"));
                }
                _ => {}
            }
        }
        let body = body.trim();
        if body.is_empty() {
            continue;
        }
        let entry = format!("{role}:\n{body}");
        total += entry.len();
        parts.push(entry);
        if total >= EVALUATOR_TRANSCRIPT_CAP {
            break;
        }
    }
    parts.reverse();
    let mut text = parts.join("\n\n");
    if text.len() > EVALUATOR_TRANSCRIPT_CAP {
        let cut = text.len() - EVALUATOR_TRANSCRIPT_CAP;
        // Slice on a char boundary to avoid panicking mid-UTF-8 sequence.
        let mut idx = cut;
        while !text.is_char_boundary(idx) {
            idx += 1;
        }
        text = text[idx..].to_string();
    }
    text
}

/// Runs the evaluator model over the transcript tail and parses its verdict.
pub async fn evaluate_goal(
    provider: &Arc<dyn AiProvider>,
    config: &ProviderConfig,
    condition: &str,
    messages: &[Message],
) -> Result<GoalVerdict> {
    let transcript = transcript_tail(messages);
    let user_message = format!(
        "GOAL CONDITION:\n{condition}\n\n\
         SESSION TRANSCRIPT (tail):\n{transcript}\n\n\
         Is the goal condition completely met? Respond with ONLY the JSON object."
    );

    let eval_config = ProviderConfig {
        api_key: config.api_key.clone(),
        model: config.model.clone(),
        max_tokens: 4096,
        temperature: 0.0,
        base_url: config.base_url.clone(),
        system_prompt: Some(EVALUATOR_SYSTEM_PROMPT.to_string()),
        thinking_budget: 0,
        context_window: 0,
        condense_threshold: 0,
        web_search_enabled: false,
        web_fetch_enabled: false,
        supports_temperature: config.supports_temperature,
        supports_reasoning_effort: config.supports_reasoning_effort,
        supports_adaptive_thinking: config.supports_adaptive_thinking,
        cancel_token: config.cancel_token.clone(),
        custom_input_cost: config.custom_input_cost,
        custom_output_cost: config.custom_output_cost,
        custom_cache_read_cost: config.custom_cache_read_cost,
        custom_cache_write_cost: config.custom_cache_write_cost,
        allowed_providers: None,
        request_overrides: config.request_overrides.clone(),
    };

    let response = provider
        .chat(
            &[Message {
                role: Role::User,
                content: vec![ContentBlock::Text { text: user_message }],
            }],
            &[],
            &eval_config,
            None,
        )
        .await?;

    let raw = response
        .content
        .iter()
        .filter_map(|b| {
            if let ContentBlock::Text { text } = b {
                Some(text.as_str())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    parse_verdict(&raw)
}

/// Extracts the `{met, reason}` JSON object from the evaluator's raw output.
fn parse_verdict(raw: &str) -> Result<GoalVerdict> {
    let start = raw
        .find('{')
        .ok_or_else(|| anyhow::anyhow!("evaluator returned no JSON: {raw}"))?;
    let Some(end) = raw.rfind('}').filter(|e| *e > start) else {
        return salvage_truncated_verdict(&raw[start..]);
    };
    let v: serde_json::Value = match serde_json::from_str(&raw[start..=end]) {
        Ok(v) => v,
        Err(e) if e.is_eof() => return salvage_truncated_verdict(&raw[start..]),
        Err(e) => return Err(e.into()),
    };
    let met = v
        .get("met")
        .and_then(|m| m.as_bool())
        .ok_or_else(|| anyhow::anyhow!("evaluator JSON missing boolean 'met': {raw}"))?;
    let reason = v
        .get("reason")
        .and_then(|r| r.as_str())
        .unwrap_or("")
        .to_string();
    Ok(GoalVerdict { met, reason })
}

/// Recovers a verdict from output cut off mid-JSON: `met` is read textually and whatever prefix of `reason` survived is kept.
fn salvage_truncated_verdict(fragment: &str) -> Result<GoalVerdict> {
    let compact: String = fragment.chars().filter(|c| !c.is_whitespace()).collect();
    let met = if compact.contains("\"met\":true") {
        true
    } else if compact.contains("\"met\":false") {
        false
    } else {
        anyhow::bail!("evaluator returned unterminated JSON without a readable 'met': {fragment}");
    };
    let reason = fragment
        .find("\"reason\"")
        .and_then(|i| fragment[i..].find(':').map(|c| i + c + 1))
        .map(|i| fragment[i..].trim_start())
        .and_then(|s| s.strip_prefix('"'))
        .map(|s| s.trim_end_matches(['"', '}']).to_string())
        .unwrap_or_default();
    tracing::warn!(
        met,
        reason_chars = reason.len(),
        "[goal] evaluator output was truncated mid-JSON — salvaged verdict"
    );
    Ok(GoalVerdict {
        met,
        reason: format!("{reason} [evaluator output truncated]"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_verdict() {
        let v = parse_verdict(r#"{"met": true, "reason": "tests pass"}"#).unwrap();
        assert!(v.met);
        assert_eq!(v.reason, "tests pass");
    }

    #[test]
    fn parses_fenced_verdict() {
        let v =
            parse_verdict("```json\n{\"met\": false, \"reason\": \"lint fails\"}\n```").unwrap();
        assert!(!v.met);
    }

    #[test]
    fn rejects_missing_met() {
        assert!(parse_verdict(r#"{"reason": "no idea"}"#).is_err());
    }

    #[test]
    fn salvages_truncated_verdict() {
        let v = parse_verdict(
            r#"{"met": true, "reason": "gh issue list confirms #80–#92 are CLOSED and"#,
        )
        .unwrap();
        assert!(v.met);
        assert!(v.reason.contains("gh issue list"));
        assert!(v.reason.contains("truncated"));
        assert!(parse_verdict(r#"{"reaso"#).is_err());
    }

    #[test]
    fn detects_marker() {
        assert!(claims_completion("done! <goal_complete>"));
        assert!(!claims_completion("still working on it"));
    }

    #[test]
    fn blocked_marker_carries_its_reason() {
        for (text, reason) in [
            (
                "All code is done.\n<goal_blocked reason=\"Needs the client's sign-off\">",
                "Needs the client's sign-off",
            ),
            (
                "<goal_blocked reason='no deploy token' />",
                "no deploy token",
            ),
            (
                "<goal_blocked reason=\"the check is `a > b`, \"quoted\", and\n  spans lines\">",
                "the check is `a > b`, \"quoted\", and spans lines",
            ),
            (
                "<goal_blocked>waiting for DNS</goal_blocked>",
                "waiting for DNS",
            ),
            ("<goal_blocked reason = \"spaced\" >", "spaced"),
            // The last marker is the one that counts.
            (
                "<goal_blocked reason=\"old\"> ... <goal_blocked reason=\"new\">",
                "new",
            ),
            // Cut off before the closing quote.
            ("<goal_blocked reason=\"out of budget", "out of budget"),
        ] {
            assert_eq!(blocked_reason(text).as_deref(), Some(reason), "{text}");
        }
        for bare in [
            "<goal_blocked>",
            "<goal_blocked reason=\"\">",
            "<goal_blocked />",
        ] {
            let reason = blocked_reason(bare).expect("still a blocked marker");
            assert!(reason.starts_with("The agent reported"), "{bare}: {reason}");
        }
        for text in [
            "still working",
            "<goal_blocked_by x>",
            "<goal_blockedness>",
            "goal_blocked",
        ] {
            assert_eq!(blocked_reason(text), None, "{text}");
        }
    }

    #[test]
    fn turn_end_prefers_blocked_over_a_completion_claim() {
        assert_eq!(turn_end("working"), TurnEnd::Continue);
        assert_eq!(turn_end("done <goal_complete>"), TurnEnd::Claimed);
        assert_eq!(
            turn_end("<goal_complete> but <goal_blocked reason=\"needs approval\">"),
            TurnEnd::Blocked("needs approval".into())
        );
    }

    #[test]
    fn every_loop_prompt_names_the_way_out() {
        for prompt in [
            kickoff_message("ship it"),
            continuation_message("ship it", None),
            continuation_message("ship it", Some("tests missing")),
        ] {
            assert!(prompt.contains("<goal_blocked reason=\""), "{prompt}");
        }
        assert!(!kickoff_message("x").contains("anyway"));
    }
}

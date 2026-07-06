//! /goal loop support: goal state shared with the host, completion-claim
//! detection in assistant output, and the small-model evaluator that verifies
//! a claimed completion before the loop is allowed to end.

use std::sync::{Arc, Mutex};

use anyhow::Result;

use crate::provider::{AiProvider, ContentBlock, Message, ProviderConfig, Role};

/// Marker the worker model must emit when it believes the goal condition is
/// fully satisfied. Emitting it triggers the evaluator; it never ends the
/// loop by itself.
pub const GOAL_COMPLETE_MARKER: &str = "<goal_complete>";

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
         output the marker {GOAL_COMPLETE_MARKER} in your final message. A separate \
         evaluator model will audit the transcript; false claims are rejected and \
         cost you a wasted turn.\n\
         5. If the goal is genuinely impossible, say why in detail and output \
         {GOAL_COMPLETE_MARKER} anyway so the evaluator can review your reasoning."
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
             Do not use ask_user; decide autonomously and note assumptions."
        ),
        None => format!(
            "[Goal loop — the goal is still active]\n\
             {condition}\n\n\
             Continue working. Produce verifiable evidence in the transcript \
             (test runs, build output, command results). When the condition is \
             completely true, output {GOAL_COMPLETE_MARKER}. Do not use ask_user; \
             decide autonomously and note assumptions."
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
{\"met\": true|false, \"reason\": \"<one or two sentences: if met, cite the evidence; if not met, state exactly what is missing or unproven>\"}";

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
        max_tokens: 1024,
        temperature: 0.0,
        base_url: config.base_url.clone(),
        system_prompt: Some(EVALUATOR_SYSTEM_PROMPT.to_string()),
        thinking_budget: 0,
        context_window: 0,
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
    };

    let response = provider
        .chat(
            vec![Message {
                role: Role::User,
                content: vec![ContentBlock::Text { text: user_message }],
            }],
            vec![],
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
    let end = raw
        .rfind('}')
        .ok_or_else(|| anyhow::anyhow!("evaluator returned unterminated JSON: {raw}"))?;
    let v: serde_json::Value = serde_json::from_str(&raw[start..=end])?;
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
    fn detects_marker() {
        assert!(claims_completion("done! <goal_complete>"));
        assert!(!claims_completion("still working on it"));
    }
}

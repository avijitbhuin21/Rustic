//! P0.2 — structured user-question tool.
//!
//! Mirrors Claude Code's [AskUserQuestionTool] schema so the frontend dialog
//! is a drop-in tabbed panel matching what users see in Claude Code itself:
//! one tab per question, radio buttons for `single`, checkboxes for `multi`,
//! free-text input for `free_text`, plus a per-question "Other" textarea.
//! All answers submit in one batch.
//!
//! [AskUserQuestionTool]:
//! references/claude_code_structure/claude-code-main/claude-code-main/src/tools/AskUserQuestionTool

use super::{ToolContext, ToolOutput};
use crate::provider::ToolDef;
use anyhow::Result;
use serde_json::{json, Value};

pub fn definitions() -> Vec<ToolDef> {
    vec![ToolDef {
        name: "ask_user".into(),
        description:
            "Ask the user one or more questions and wait for their answers. \
             Use this when you genuinely need a decision or preference that the \
             user has not provided and that you cannot infer reliably. \
             Each question has its own answer type: \
             - `single` — radio-button choice from the provided `options` list. \
             - `multi` — checkbox subset of the provided `options` list. \
             - `free_text` — free-form textarea answer (no `options` needed). \
             A single call may bundle multiple related questions; the dialog \
             renders one tab per question and the user submits all answers \
             at once. Prefer one batched call over several round-trips. \
             Returns an object keyed by each question's `id` with the user's \
             answer."
                .into(),
        parameters: json!({
            "type": "object",
            "properties": {
                "questions": {
                    "type": "array",
                    "minItems": 1,
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": {
                                "type": "string",
                                "description": "Stable id for this question, used as the key in the response object."
                            },
                            "text": {
                                "type": "string",
                                "description": "The question shown to the user."
                            },
                            "kind": {
                                "type": "string",
                                "enum": ["single", "multi", "free_text"],
                                "description": "Answer type: `single` = pick one option, `multi` = pick any subset of options, `free_text` = type a response."
                            },
                            "options": {
                                "type": "array",
                                "items": { "type": "string" },
                                "description": "Required for `single` and `multi`; omit for `free_text`. The user can always type an answer in the 'Other' field even when options are provided."
                            }
                        },
                        "required": ["id", "text", "kind"]
                    }
                }
            },
            "required": ["questions"]
        }),
    }]
}

pub async fn execute(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    // Sub-agents don't have access to ask_user (no UI dialog in their context)
    if context.subagent_self.is_some() {
        return Ok(ToolOutput {
            content: "ask_user is not available in sub-agents. Sub-agents must work \
                      with the information provided by the parent agent. If you need \
                      user input, end your turn with a summary and let the parent handle it."
                .into(),
            is_error: true,
            attachments: Vec::new(),
        });
    }
    
    // P0.2: pass the questions array straight through to the frontend via
    // the AskUserBroker. The broker emits a TaskEvent::AskUserRequest,
    // parks on a oneshot, and unblocks when the user submits answers.
    // Returns the user's `{ question_id -> answer }` map as a stringified
    // JSON object so the agent can JSON.parse on its side.
    let questions = match params.get("questions") {
        Some(Value::Array(arr)) if !arr.is_empty() => Value::Array(arr.clone()),
        _ => {
            return Ok(ToolOutput {
                content: "ask_user: `questions` must be a non-empty array of \
                          { id, text, kind, options? } objects."
                    .into(),
                is_error: true,
                attachments: Vec::new(),
            });
        }
    };
    let response = context
        .ask_user_broker
        .request(&context.event_tx, &context.task_id, questions)
        .await;
    let Some(resp) = response else {
        return Ok(ToolOutput {
            content: "ASK_USER_TIMEOUT: The user didn't answer within 24 hours. \
                      End your turn with a status message; the user can re-prompt \
                      you when they're ready."
                .into(),
            is_error: true, attachments: Vec::new() });
    };
    if resp.cancelled {
        return Ok(ToolOutput {
            content: "ASK_USER_CANCELLED: The user dismissed the question dialog \
                      without answering. Treat this as 'they don't want to choose \
                      right now' and either propose a default in plain text or \
                      end your turn asking them to be more specific."
                .into(),
            is_error: false, attachments: Vec::new() });
    }
    let answers_json =
        serde_json::to_string(&resp.answers).unwrap_or_else(|_| "{}".to_string());
    // Any images the user attached to their answer ride back to the model as
    // image attachments on this tool result (the executor turns each into an
    // Image content block in the same user turn).
    Ok(ToolOutput {
        content: answers_json,
        is_error: false, attachments: resp.images })
}

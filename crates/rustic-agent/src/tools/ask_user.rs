use super::{ToolContext, ToolOutput};
use crate::provider::ToolDef;
use anyhow::Result;
use serde_json::{json, Value};

pub fn definitions() -> Vec<ToolDef> {
    vec![ToolDef {
        name: "chat_message".into(),
        description: "Send a message to the user. Use type \"question\" to ask a clarifying question \
                      and wait for their response — e.g. ambiguous requirements, missing context, \
                      or design choices that only the user can decide. Use type \"message\" to \
                      communicate status updates, summaries, or any other information without \
                      waiting for a response. Prefer this over plain assistant text when you need \
                      a clear, structured message."
            .into(),
        parameters: json!({
            "type": "object",
            "properties": {
                "text": {
                    "type": "string",
                    "description": "The message text. For questions, ask something specific and concise. For messages, provide a clear summary or status update."
                },
                "type": {
                    "type": "string",
                    "enum": ["question", "message"],
                    "description": "\"question\" pauses execution and waits for the user's reply. \"message\" displays the text and continues immediately."
                }
            },
            "required": ["text", "type"]
        }),
    }]
}

pub async fn execute(_name: &str, params: Value, context: &ToolContext) -> Result<ToolOutput> {
    let text = params["text"].as_str().unwrap_or("");
    let msg_type = params["type"].as_str().unwrap_or("message");

    if text.is_empty() {
        return Ok(ToolOutput {
            content: "No text provided".into(),
            is_error: true,
        });
    }

    match msg_type {
        "question" => {
            // Wait for user response (same as old ask_user behavior)
            match context
                .question_broker
                .ask(&context.event_tx, &context.task_id, text)
                .await
            {
                Ok(answer) => Ok(ToolOutput {
                    content: format!("User response: {}", answer),
                    is_error: false,
                }),
                Err(e) => Ok(ToolOutput {
                    content: e,
                    is_error: true,
                }),
            }
        }
        "message" => {
            // Non-blocking — just acknowledge the message was sent
            Ok(ToolOutput {
                content: "Message delivered to user.".into(),
                is_error: false,
            })
        }
        _ => Ok(ToolOutput {
            content: format!("Unknown message type '{}' — use 'question' or 'message'", msg_type),
            is_error: true,
        }),
    }
}

use super::{ToolContext, ToolOutput};
use crate::provider::ToolDef;
use anyhow::Result;
use serde_json::{json, Value};

pub fn definitions() -> Vec<ToolDef> {
    vec![ToolDef {
        name: "ask_user".into(),
        description: "Ask the user a clarifying question and wait for their response. \
                      Use this when you need more information before proceeding — \
                      e.g. ambiguous requirements, missing context, or design choices \
                      that only the user can decide. Do not use this for status updates."
            .into(),
        parameters: json!({
            "type": "object",
            "properties": {
                "question": {
                    "type": "string",
                    "description": "The question to ask the user. Be specific and concise."
                }
            },
            "required": ["question"]
        }),
    }]
}

pub async fn execute(_name: &str, params: Value, context: &ToolContext) -> Result<ToolOutput> {
    let question = params["question"].as_str().unwrap_or("");
    if question.is_empty() {
        return Ok(ToolOutput {
            content: "No question provided".into(),
            is_error: true,
        });
    }

    match context
        .question_broker
        .ask(&context.event_tx, &context.task_id, question)
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

use anyhow::Result;
use serde_json::{json, Value};

use crate::provider::ToolDef;
use crate::tools::{ToolContext, ToolOutput};

/// The single terminal tool definition. The model MUST call this to end a
/// task — it both signals completion to the executor loop and produces the
/// canonical summary shown to the user (or, for sub-agents, returned to the
/// parent). Emitting plain text as the final assistant message still works as
/// a fallback but is discouraged; the system prompt enforces this rule.
pub fn definitions() -> Vec<ToolDef> {
    vec![ToolDef {
        name: "complete_task".into(),
        description: "Signal that the task is complete. Call this as your final \
                      action once the user's request has been fully addressed. \
                      The `summary` becomes the message shown to the user and — \
                      for sub-agents — the result returned to the parent agent. \
                      Do NOT call this mid-task; only when there is no remaining \
                      work. After calling, no further tools will run."
            .into(),
        parameters: json!({
            "type": "object",
            "properties": {
                "summary": {
                    "type": "string",
                    "description": "Concise, user-facing summary of what was done. \
                                    Include: the change(s) made, files touched, \
                                    any decisions/tradeoffs, and follow-ups. \
                                    Prefer bullet points. Avoid restating the task."
                },
                "artifacts": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional list of file paths that were created or modified."
                }
            },
            "required": ["summary"]
        }),
    }]
}

pub async fn execute(_name: &str, params: Value, context: &ToolContext) -> Result<ToolOutput> {
    let summary = params
        .get("summary")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();

    if summary.is_empty() {
        return Ok(ToolOutput {
            content: "complete_task requires a non-empty `summary`. Provide a \
                      concise description of what was done, then call this tool again."
                .to_string(),
            is_error: true,
        });
    }

    // Record the summary in the shared slot. The executor checks this after
    // the tool batch completes and breaks the loop.
    let final_summary = if let Some(arr) = params.get("artifacts").and_then(|v| v.as_array()) {
        let paths: Vec<String> = arr
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect();
        if paths.is_empty() {
            summary
        } else {
            format!("{}\n\n**Files touched:**\n{}",
                summary,
                paths.iter().map(|p| format!("- `{}`", p)).collect::<Vec<_>>().join("\n"))
        }
    } else {
        summary
    };

    if let Ok(mut slot) = context.completion_summary.lock() {
        *slot = Some(final_summary.clone());
    }

    Ok(ToolOutput {
        content: "Task marked complete. Summary recorded.".to_string(),
        is_error: false,
    })
}

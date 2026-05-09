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
                      for sub-agents — the ONLY data returned to the parent agent \
                      (assistant text streamed to chat is NOT visible to the parent, \
                      only the `summary` parameter is). \
                      \
                      WHEN NOT to call this: (1) You are mid-task and still have \
                      work to do. (2) You are asking a clarifying question — use \
                      chat_message instead, then continue working. (3) You want to \
                      report intermediate status — use chat_message for that too. \
                      \
                      Do NOT call this mid-task or in the same turn as a question. \
                      After calling, no further tools will run."
            .into(),
        parameters: json!({
            "type": "object",
            "properties": {
                "summary": {
                    "type": "string",
                    "description": "The deliverable for this task — this string is \
                                    the ONLY thing the user (or, for sub-agents, the \
                                    parent agent) sees. Two cases:\n\
                                    \n\
                                    1) Research / read / analyze tasks: put the actual \
                                    findings INLINE here — file contents, function \
                                    signatures, code excerpts, conclusions, whatever \
                                    was asked for. Markdown formatting (headers, \
                                    bullets, code fences) is fine inside this string. \
                                    Do NOT write the answer as plain assistant text \
                                    and then put \"see above\" / \"summarized above\" / \
                                    \"as detailed in my message\" here — the recipient \
                                    will not have access to anything you wrote outside \
                                    this `summary` field, only this string.\n\
                                    \n\
                                    2) Write / edit tasks: describe what you changed \
                                    (files touched, decisions/tradeoffs, follow-ups). \
                                    Bullet points preferred. Avoid restating the task.\n\
                                    \n\
                                    When in doubt, err on the side of including more \
                                    detail rather than less.\n\
                                    \n\
                                    NEVER pass an empty or stub summary (e.g. \
                                    \"done\" or \"see above\") — if you do, the call \
                                    will be rejected and you will need to retry with \
                                    the real content."
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

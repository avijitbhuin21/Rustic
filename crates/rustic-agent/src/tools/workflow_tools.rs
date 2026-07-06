use crate::tools::{ToolContext, ToolOutput};
use crate::workflows::{discover_workflows, workflow_body};
use anyhow::Result;
use serde_json::{json, Value};

/// Handle the `read_workflow` tool call.
pub async fn execute(name: &str, params: Value, context: &ToolContext) -> Result<ToolOutput> {
    match name {
        "read_workflow" => read_workflow(params, context).await,
        _ => Ok(ToolOutput {
            content: format!("Unknown workflow tool: {}", name),
            is_error: true,
            attachments: Vec::new(),
        }),
    }
}

/// Tool definitions exposed to the AI provider.
pub fn definitions() -> Vec<crate::provider::ToolDef> {
    vec![crate::provider::ToolDef {
        name: "read_workflow".to_string(),
        description: "Load the full prompt of a named workflow and execute it. Call this \
             when the user's request matches a workflow's advertised purpose, \
             or when the user explicitly names one. Returns the complete \
             workflow body."
            .to_string(),
        parameters: json!({
            "type": "object",
            "required": ["name"],
            "properties": {
                "name": {
                    "type": "string",
                    "description": "The workflow name as shown in the workflows list"
                }
            }
        }),
    }]
}

async fn read_workflow(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    let name = params
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();

    if name.is_empty() {
        return Ok(ToolOutput {
            content: "INVALID_PARAMS: name is required".to_string(),
            is_error: true,
            attachments: Vec::new(),
        });
    }

    let workflows = discover_workflows(&context.project_root);
    let workflow = workflows.iter().find(|w| w.name == name);

    match workflow {
        None => Ok(ToolOutput {
            content: format!(
                "WORKFLOW_NOT_FOUND: No workflow named \"{}\". Available workflows: {}",
                name,
                workflows
                    .iter()
                    .map(|w| w.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            is_error: true,
            attachments: Vec::new(),
        }),
        Some(w) => match std::fs::read_to_string(&w.path) {
            Ok(content) => {
                let body = workflow_body(&content);
                Ok(ToolOutput {
                    content: format!("# Workflow: {}\n\n{}", w.name, body),
                    is_error: false,
                    attachments: Vec::new(),
                })
            }
            Err(e) => Ok(ToolOutput {
                content: format!("CONTENT_DELETED: Could not read workflow file: {}", e),
                is_error: true,
                attachments: Vec::new(),
            }),
        },
    }
}

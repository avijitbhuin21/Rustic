use crate::skills::{discover_skills, skill_body};
use crate::tools::{ToolContext, ToolOutput};
use anyhow::Result;
use serde_json::{json, Value};

/// Handle the `read_skill` tool call.
pub async fn execute(name: &str, params: Value, context: &ToolContext) -> Result<ToolOutput> {
    match name {
        "read_skill" => read_skill(params, context).await,
        _ => Ok(ToolOutput {
            content: format!("Unknown skill tool: {}", name),
            is_error: true,
            attachments: Vec::new(),
        }),
    }
}

/// Tool definitions exposed to the AI provider.
pub fn definitions() -> Vec<crate::provider::ToolDef> {
    vec![crate::provider::ToolDef {
        name: "read_skill".to_string(),
        description: "Load the full instructions for a named skill. Call this before following \
             any skill's guidance. Returns the complete SKILL.md body."
            .to_string(),
        parameters: json!({
            "type": "object",
            "required": ["name"],
            "properties": {
                "name": {
                    "type": "string",
                    "description": "The skill name as shown in the skills list (e.g. \"code-review\")"
                }
            }
        }),
    }]
}

async fn read_skill(params: Value, context: &ToolContext) -> Result<ToolOutput> {
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

    let skills = discover_skills(&context.project_root);
    let skill = skills.iter().find(|s| s.name == name);

    match skill {
        None => Ok(ToolOutput {
            content: format!(
                "SKILL_NOT_FOUND: No skill named \"{}\". Available skills: {}",
                name,
                skills
                    .iter()
                    .map(|s| s.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            is_error: true,
            attachments: Vec::new(),
        }),
        Some(skill_def) => match std::fs::read_to_string(&skill_def.path) {
            Ok(content) => {
                let body = skill_body(&content);
                Ok(ToolOutput {
                    content: format!("# Skill: {}\n\n{}", skill_def.name, body),
                    is_error: false,
                    attachments: Vec::new(),
                })
            }
            Err(e) => Ok(ToolOutput {
                content: format!("CONTENT_DELETED: Could not read skill file: {}", e),
                is_error: true,
                attachments: Vec::new(),
            }),
        },
    }
}

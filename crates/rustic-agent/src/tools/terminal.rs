use super::{ToolContext, ToolOutput};
use crate::provider::ToolDef;
use crate::task::permissions::Action;
use anyhow::Result;
use serde_json::{json, Value};
use std::process::Command;

pub fn definitions() -> Vec<ToolDef> {
    vec![ToolDef {
        name: "run_command".into(),
        description: "Run a shell command and return its output. Use for build, test, and other CLI tasks.".into(),
        parameters: json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "The command to run" },
                "cwd": { "type": "string", "description": "Working directory (relative to project root, optional)" }
            },
            "required": ["command"]
        }),
    }]
}

pub async fn execute(_name: &str, params: Value, context: &ToolContext) -> Result<ToolOutput> {
    if !context.check_permission(&Action::Execute) {
        return Ok(ToolOutput {
            content: "Permission denied: command execution not allowed".into(),
            is_error: true,
        });
    }

    let cmd_str = params["command"].as_str().unwrap_or("");
    if cmd_str.is_empty() {
        return Ok(ToolOutput {
            content: "No command provided".into(),
            is_error: true,
        });
    }

    let cwd = params["cwd"]
        .as_str()
        .map(|c| context.project_root.join(c))
        .unwrap_or_else(|| context.project_root.clone());

    // Use platform shell
    let output = if cfg!(target_os = "windows") {
        Command::new("cmd").args(["/C", cmd_str]).current_dir(&cwd).output()
    } else {
        Command::new("sh").args(["-c", cmd_str]).current_dir(&cwd).output()
    };

    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            let mut result = String::new();
            if !stdout.is_empty() {
                result.push_str(&stdout);
            }
            if !stderr.is_empty() {
                if !result.is_empty() {
                    result.push_str("\n--- stderr ---\n");
                }
                result.push_str(&stderr);
            }
            if result.is_empty() {
                result = format!("Command completed with exit code {}", out.status.code().unwrap_or(-1));
            }
            Ok(ToolOutput {
                content: result,
                is_error: !out.status.success(),
            })
        }
        Err(e) => Ok(ToolOutput {
            content: format!("Failed to execute command: {}", e),
            is_error: true,
        }),
    }
}

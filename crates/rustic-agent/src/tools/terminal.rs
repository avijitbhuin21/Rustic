use super::{ToolContext, ToolOutput};

/// Truncate a UTF-8 string to at most `max_bytes` bytes without splitting a codepoint.
fn truncate_utf8(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}
use crate::provider::ToolDef;
use crate::task::permissions::PermissionLevel;
use crate::task::PermissionOp;
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

pub async fn execute(_name: &str, tool_use_id: &str, params: Value, context: &ToolContext) -> Result<ToolOutput> {
    let cmd_str = params["command"].as_str().unwrap_or("");
    if cmd_str.is_empty() {
        return Ok(ToolOutput {
            content: "No command provided".into(),
            is_error: true,
        });
    }

    // Chat mode: hard deny
    if context.permissions() == PermissionLevel::Chat {
        return Ok(ToolOutput {
            content: "PERMISSION_DENIED: Command execution is not allowed in Chat mode.".into(),
            is_error: true,
        });
    }

    // ManualEdit / AutoEdit: ask the user
    if context.needs_exec_approval() {
        let approved = context
            .permission_broker
            .request(
                &context.event_tx,
                &context.task_id,
                PermissionOp::RunCommand(cmd_str.to_string()),
            )
            .await;
        if !approved {
            return Ok(ToolOutput {
                content: "PERMISSION_DENIED: User denied command execution.".into(),
                is_error: true,
            });
        }
    }

    // FullAuto (or approved): execute
    let cwd = params["cwd"]
        .as_str()
        .map(|c| context.project_root.join(c))
        .unwrap_or_else(|| context.project_root.clone());

    // Emit progress so the UI shows what's running
    let short_cmd = if cmd_str.len() > 60 { &cmd_str[..57] } else { cmd_str };
    context.emit_progress(tool_use_id, &format!("$ {short_cmd}"));

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

            // Hard cap at 16KB to keep context window usage bounded.
            const MAX_BYTES: usize = 16 * 1024;
            let result = if result.len() > MAX_BYTES {
                let truncated = truncate_utf8(&result, MAX_BYTES);
                let remaining_lines = result[truncated.len()..]
                    .lines()
                    .count();
                format!(
                    "{}\nOUTPUT_TRUNCATED: Truncated at 16KB — {} more lines. Use head/tail/grep to filter.",
                    truncated,
                    remaining_lines
                )
            } else {
                result
            };

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

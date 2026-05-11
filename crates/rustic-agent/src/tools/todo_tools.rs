use super::{ToolContext, ToolOutput};
use crate::provider::ToolDef;
use crate::task::{TaskEvent, TodoItem};
use anyhow::Result;
use serde_json::{json, Value};

pub fn definitions() -> Vec<ToolDef> {
    vec![ToolDef {
        name: "todo_write".into(),
        description: "Create or update your task checklist. Pass the full list of todos each time — \
                      items not included are removed. Use this to plan multi-step work and \
                      track progress. Mark each task as completed as soon as you finish it."
            .into(),
        parameters: json!({
            "type": "object",
            "properties": {
                "todos": {
                    "type": "array",
                    "description": "The complete todo list (replaces any previous list)",
                    "items": {
                        "type": "object",
                        "properties": {
                            "content": {
                                "type": "string",
                                "description": "What needs to be done (imperative form, e.g. 'Add login endpoint')"
                            },
                            "status": {
                                "type": "string",
                                "enum": ["pending", "in_progress", "completed"],
                                "description": "Current status of this task"
                            }
                        },
                        "required": ["content", "status"]
                    }
                }
            },
            "required": ["todos"]
        }),
    }]
}

pub async fn execute(_name: &str, params: Value, context: &ToolContext) -> Result<ToolOutput> {
    let todos_val = match params.get("todos").and_then(|v| v.as_array()) {
        Some(arr) => arr,
        None => {
            return Ok(ToolOutput {
                content: "todos array is required".into(),
                is_error: true,
            })
        }
    };

    let mut todos: Vec<TodoItem> = Vec::new();
    for item in todos_val {
        let content = item
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let status = item
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("pending")
            .to_string();

        if content.is_empty() {
            continue;
        }

        if !["pending", "in_progress", "completed"].contains(&status.as_str()) {
            return Ok(ToolOutput {
                content: format!("Invalid status '{}' — use pending, in_progress, or completed", status),
                is_error: true,
            });
        }

        todos.push(TodoItem { content, status });
    }

    let total = todos.len();
    let completed = todos.iter().filter(|t| t.status == "completed").count();
    let in_progress = todos.iter().filter(|t| t.status == "in_progress").count();

    // Emit event so the UI can render the todo list
    let _ = context.event_tx.try_send(TaskEvent::TodoUpdated {
        task_id: context.task_id.clone(),
        todos,
    });

    Ok(ToolOutput {
        content: format!(
            "Todo list updated: {} total, {} completed, {} in progress, {} pending",
            total,
            completed,
            in_progress,
            total - completed - in_progress
        ),
        is_error: false,
    })
}

use super::{ToolContext, ToolOutput};
use crate::provider::ToolDef;
use crate::task::{TaskEvent, TodoItem};
use anyhow::Result;
use serde_json::{json, Value};

pub fn definitions() -> Vec<ToolDef> {
    vec![ToolDef {
        name: "todo_write".into(),
        description:
            "Create or update your task checklist. Pass the full list of todos each time — \
                      items not included are removed. Use this to plan multi-step work and \
                      track progress. Mark each task as completed as soon as you finish it. \
                      The list is durable: it is re-shown to you periodically during long \
                      sessions and survives context summarization, so keeping it current is \
                      what keeps the task on track."
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
                            },
                            "group": {
                                "type": "string",
                                "description": "Optional short label (e.g. 'checks'). Give the SAME label to items that do not depend on each other, so their tool calls can go out together in one response. Omit it for steps that must happen in order."
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
                attachments: Vec::new(),
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
                content: format!(
                    "Invalid status '{}' — use pending, in_progress, or completed",
                    status
                ),
                is_error: true,
                attachments: Vec::new(),
            });
        }

        let group = TodoItem::parse_group(item.get("group"));
        todos.push(TodoItem {
            content,
            status,
            group,
        });
    }

    let total = todos.len();
    let completed = todos.iter().filter(|t| t.status == "completed").count();
    let in_progress = todos.iter().filter(|t| t.status == "in_progress").count();

    // Write through to the shared slot so the executor can reinject the list
    // as a periodic anchor and preserve it verbatim through condensing.
    if let Ok(mut slot) = context.current_todos.lock() {
        *slot = todos.clone();
    }

    // Emit event so the UI can render the todo list
    let _ = context.event_tx.try_send(TaskEvent::TodoUpdated {
        task_id: context.task_id.clone(),
        todos: todos.clone(),
    });

    let list_lines: Vec<String> = todos.iter().enumerate().map(|(i, t)| t.line(i)).collect();
    let list_text = if list_lines.is_empty() {
        "(empty)".to_string()
    } else {
        list_lines.join("\n")
    };

    Ok(ToolOutput {
        content: format!(
            "Todo list updated: {} total, {} completed, {} in progress, {} pending\n\nCurrent list:\n{}",
            total,
            completed,
            in_progress,
            total - completed - in_progress,
            list_text,
        ),
        is_error: false,
        attachments: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn group_label_is_stored_rendered_and_optional() {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, _rx) = ToolContext::new_test(dir.path().to_path_buf());
        let params = json!({ "todos": [
            { "content": "Edit parser", "status": "completed" },
            { "content": "Run typecheck", "status": "pending", "group": " checks " },
            { "content": "Run tests", "status": "pending", "group": "checks" },
            { "content": "Write summary", "status": "pending", "group": "  " }
        ]});
        let out = execute("todo_write", params, &ctx).await.unwrap();
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("1. [completed] Edit parser\n"));
        assert!(out
            .content
            .contains("2. [pending] Run typecheck (group: checks)"));
        assert!(out.content.ends_with("4. [pending] Write summary"));

        let stored = ctx.current_todos.lock().unwrap().clone();
        let groups: Vec<Option<&str>> = stored.iter().map(|t| t.group.as_deref()).collect();
        assert_eq!(groups, vec![None, Some("checks"), Some("checks"), None]);
    }

    #[test]
    fn stored_todos_without_group_still_load() {
        let old: Vec<TodoItem> =
            serde_json::from_str(r#"[{"content":"a","status":"pending"}]"#).unwrap();
        assert_eq!(old[0].group, None);
        // Ungrouped items serialise exactly as before.
        assert_eq!(
            serde_json::to_string(&old).unwrap(),
            r#"[{"content":"a","status":"pending"}]"#
        );
    }
}

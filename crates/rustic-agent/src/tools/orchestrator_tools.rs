//! Tools available to the Global orchestrator agent.
//!
//! All three route through `ToolContext::orchestrator_host`, which the host
//! app wires up only for tasks running in the Global scope. Outside Global
//! the tools return a "not available" error so the model doesn't try to
//! call them from a project-scoped chat.

use anyhow::Result;
use serde_json::{json, Value};

use crate::provider::ToolDef;
use crate::task::orchestrator_host::OrchestratorTaskFilter;
use crate::tools::{ToolContext, ToolOutput};

pub fn definitions() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "list_projects".into(),
            description: "List every project the orchestrator can delegate \
                          to. Returns id, name, root_path, and a compact \
                          `file_tree` overview per project (depth 3, up to \
                          120 entries). The tree respects each project's \
                          .gitignore and drops common bloat dirs \
                          (node_modules, target, .git, dist, __pycache__, \
                          venv, etc.) — so it's a curated structural view, \
                          not a full listing. For deeper detail on a \
                          specific project use `run_command` with `ls` or \
                          `cat` against its root_path. Call this early in \
                          any conversation so you know what's available."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "list_tasks_across_projects".into(),
            description: "List tasks from every project (chats in the agent \
                          panel). Use this to find prior work before spawning \
                          new sub-tasks. Optionally filter by project_id or \
                          status (\"Running\" / \"Completed\" / \"Failed\" / \
                          \"Cancelled\"). `limit` caps results (default: all)."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "project_id": { "type": "string", "description": "Filter to a single project." },
                    "status": { "type": "string", "description": "Filter by status (case-insensitive)." },
                    "limit": { "type": "integer", "minimum": 1, "description": "Cap result count." }
                },
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "read_task_history".into(),
            description: "Fetch the full message history for a task. Use this \
                          after `list_tasks_across_projects` to review what \
                          happened in a prior chat. Returns role + content \
                          (serialized JSON content blocks)."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "task_id": { "type": "string", "description": "The task id returned by list_tasks_across_projects." }
                },
                "required": ["task_id"],
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "spawn_subtask".into(),
            description: "Create a new top-level chat in the named project and \
                          dispatch the given prompt as its first user message. \
                          Fire-and-forget: returns the new task_id immediately; \
                          the sub-task runs independently and appears in the \
                          agent panel. Use to delegate work to a specific \
                          project. Do NOT spawn inside the Global scope."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "project_id": { "type": "string", "description": "Target project id (from list_projects)." },
                    "prompt":     { "type": "string", "description": "First user message for the new chat." },
                    "title":      { "type": "string", "description": "Optional chat title. Default: first 70 chars of prompt." }
                },
                "required": ["project_id", "prompt"],
                "additionalProperties": false
            }),
        },
    ]
}

pub async fn execute(name: &str, params: Value, context: &ToolContext) -> Result<ToolOutput> {
    tracing::warn!("[orchestrator] execute tool={} is_global={}", name, context.is_global);
    if !context.is_global {
        return Ok(ToolOutput {
            content: format!(
                "`{}` is only available to the Global orchestrator. This chat \
                 is scoped to a single project.",
                name
            ),
            is_error: true,
        });
    }

    let host = match &context.orchestrator_host {
        Some(h) => h.clone(),
        None => {
            tracing::warn!("[orchestrator] ERROR: orchestrator_host not set on ToolContext");
            return Ok(ToolOutput {
                content: "Orchestrator host not configured. This is a bug — \
                          report it to the developer."
                    .into(),
                is_error: true,
            });
        }
    };

    match name {
        "list_projects" => {
            // Heavy filesystem walks (one per project) run on a blocking
            // thread so we don't stall the tokio runtime. Without this,
            // the executor's event loop is frozen for the duration of
            // the walk and the model perceives the tool as hung.
            let start = std::time::Instant::now();
            let join = tokio::task::spawn_blocking(move || host.list_projects()).await;
            let result = match join {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!("[orchestrator] list_projects join error: {}", e);
                    return Ok(ToolOutput {
                        content: format!("list_projects failed (join): {}", e),
                        is_error: true,
                    });
                }
            };
            tracing::warn!(
                "[orchestrator] list_projects finished in {} ms",
                start.elapsed().as_millis()
            );
            match result {
                Ok(projects) => {
                    tracing::warn!("[orchestrator] list_projects -> {} projects", projects.len());
                    Ok(ToolOutput {
                        content: serde_json::to_string_pretty(&projects).unwrap_or_default(),
                        is_error: false,
                    })
                }
                Err(e) => {
                    tracing::warn!("[orchestrator] list_projects error: {}", e);
                    Ok(ToolOutput {
                        content: format!("list_projects failed: {}", e),
                        is_error: true,
                    })
                }
            }
        }

        "list_tasks_across_projects" => {
            let filter = OrchestratorTaskFilter {
                project_id: params
                    .get("project_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                status: params
                    .get("status")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                limit: params
                    .get("limit")
                    .and_then(|v| v.as_u64())
                    .map(|n| n as usize),
            };
            match host.list_tasks(filter) {
                Ok(tasks) => Ok(ToolOutput {
                    content: serde_json::to_string_pretty(&tasks).unwrap_or_default(),
                    is_error: false,
                }),
                Err(e) => Ok(ToolOutput {
                    content: format!("list_tasks_across_projects failed: {}", e),
                    is_error: true,
                }),
            }
        }

        "read_task_history" => {
            let task_id = match params.get("task_id").and_then(|v| v.as_str()) {
                Some(s) if !s.is_empty() => s.to_string(),
                _ => {
                    return Ok(ToolOutput {
                        content: "read_task_history requires a non-empty `task_id`.".into(),
                        is_error: true,
                    });
                }
            };
            match host.read_task_history(&task_id) {
                Ok(messages) => Ok(ToolOutput {
                    content: serde_json::to_string_pretty(&messages).unwrap_or_default(),
                    is_error: false,
                }),
                Err(e) => Ok(ToolOutput {
                    content: format!("read_task_history failed: {}", e),
                    is_error: true,
                }),
            }
        }

        "spawn_subtask" => {
            let project_id = match params.get("project_id").and_then(|v| v.as_str()) {
                Some(s) if !s.is_empty() => s.to_string(),
                _ => {
                    return Ok(ToolOutput {
                        content: "spawn_subtask requires a non-empty `project_id`. \
                                  Call `list_projects` first if you don't have one."
                            .into(),
                        is_error: true,
                    });
                }
            };
            let prompt = match params.get("prompt").and_then(|v| v.as_str()) {
                Some(s) if !s.trim().is_empty() => s.to_string(),
                _ => {
                    return Ok(ToolOutput {
                        content: "spawn_subtask requires a non-empty `prompt`.".into(),
                        is_error: true,
                    });
                }
            };
            let title = params
                .get("title")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            match host.spawn_subtask(&project_id, title, prompt, &context.task_id) {
                Ok(task_id) => Ok(ToolOutput {
                    content: format!(
                        "Sub-task created. task_id = \"{}\". It is running \
                         independently; the user will see it appear in the \
                         agent panel.",
                        task_id
                    ),
                    is_error: false,
                }),
                Err(e) => Ok(ToolOutput {
                    content: format!("spawn_subtask failed: {}", e),
                    is_error: true,
                }),
            }
        }

        _ => Ok(ToolOutput {
            content: format!("Unknown orchestrator tool: {}", name),
            is_error: true,
        }),
    }
}

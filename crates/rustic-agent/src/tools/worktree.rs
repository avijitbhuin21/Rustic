//! Worktree tools (`enter_worktree` / `exit_worktree`).
//!
//! Creates/prunes git worktrees under `<project>/.rustic/worktrees/<name>`
//! so parallel sub-agents can work in the same project without checkout conflicts.
//! Sub-agents receive their own `ToolContext` with the worktree as `project_root`.

use super::{ToolContext, ToolOutput};
use crate::provider::ToolDef;
use crate::task::permissions::PermissionLevel;
use anyhow::Result;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub fn definitions() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "enter_worktree".into(),
            description: "Create a fresh git worktree for parallel work on this project. \
                          Worktrees are checked out into `.rustic/worktrees/<name>/` and \
                          point at the same `.git` as the main checkout, so they share \
                          history but have independent indexes and working trees. \
                          Pass `name` (short slug — also the worktree id) and optionally \
                          `branch` (the branch to check out; defaults to a new branch \
                          with the same name as the worktree). \
                          \
                          USE THIS when you're about to fan out write-heavy sub-agents \
                          across the same project: spawn each sub-agent with its own \
                          worktree path as `project_root` and they cannot step on each \
                          other's edits. Returns the absolute path to the worktree.".into(),
            parameters: json!({
                "type": "object",
                "required": ["name"],
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Short slug identifying the worktree (used as both \
                                        the directory name under .rustic/worktrees/ and \
                                        the worktree's admin name). Must be unique."
                    },
                    "branch": {
                        "type": "string",
                        "description": "Optional branch to check out. Created from HEAD if \
                                        it doesn't already exist. Defaults to a new branch \
                                        named the same as `name`."
                    }
                }
            }),
        },
        ToolDef {
            name: "list_worktrees".into(),
            description: "List every git worktree currently attached to this project. Returns \
                          the worktree name and the absolute path each is checked out into. \
                          Useful before spawning sub-agents — call this to find out which \
                          worktrees already exist so you don't double-create one, and to \
                          recover the absolute paths you can pass as `spawn_subagent`'s \
                          `project_root` field. Read-only; allowed in Chat / Plan modes.".into(),
            parameters: json!({
                "type": "object",
                "properties": {}
            }),
        },
        ToolDef {
            name: "exit_worktree".into(),
            description: "Prune a worktree previously created with `enter_worktree`. \
                          Removes the working directory contents and the `.git/worktrees/<name>` \
                          admin entry. Refuses by default if the worktree has uncommitted \
                          changes — pass `force: true` only when those changes are known \
                          disposable. The worktree's branch is left intact; commits made \
                          inside the worktree remain reachable via that branch.".into(),
            parameters: json!({
                "type": "object",
                "required": ["name"],
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Name of the worktree to remove (the same `name` passed \
                                        to `enter_worktree`)."
                    },
                    "force": {
                        "type": "boolean",
                        "description": "Default false. When true, discard any uncommitted \
                                        changes in the worktree before pruning. Use only \
                                        when the work is known-throwaway."
                    }
                }
            }),
        },
    ]
}

pub async fn execute(name: &str, params: Value, context: &ToolContext) -> Result<ToolOutput> {
    match name {
        "enter_worktree" => execute_enter_worktree(params, context).await,
        "exit_worktree" => execute_exit_worktree(params, context).await,
        "list_worktrees" => execute_list_worktrees(context).await,
        _ => Ok(ToolOutput {
            content: format!("Unknown worktree tool: {}", name),
            is_error: true,
            attachments: Vec::new(),
        }),
    }
}

async fn execute_list_worktrees(context: &ToolContext) -> Result<ToolOutput> {
    // No write side effect — safe in every permission mode including
    // Chat and Plan. The Global scope still blocks it (cross-project
    // worktree introspection isn't a useful pattern from the orchestrator).
    if context.is_global {
        return Ok(ToolOutput {
            content: "PERMISSION_DENIED: `list_worktrees` is blocked in the Global scope — \
                      call it from within a project task instead."
                .into(),
            is_error: true, attachments: Vec::new() });
    }
    let entries = match list_worktrees_for_project(&context.project_root) {
        Ok(v) => v,
        Err(e) => {
            return Ok(ToolOutput {
                content: format!("LIST_WORKTREES_FAILED: {}", e),
                is_error: true,
                attachments: Vec::new(),
            });
        }
    };
    if entries.is_empty() {
        return Ok(ToolOutput {
            content: "No worktrees attached. The main checkout is the only working tree. \
                      Call `enter_worktree` to create one."
                .into(),
            is_error: false, attachments: Vec::new() });
    }
    let mut body = format!("{} worktree(s) attached:\n", entries.len());
    for (name, path) in &entries {
        body.push_str(&format!("- {} → {}\n", name, path.display()));
    }
    body.push_str(
        "\nPass an absolute path above as `spawn_subagent`'s `project_root` field to run a \
         sub-agent inside that worktree.",
    );
    Ok(ToolOutput {
        content: body,
        is_error: false, attachments: Vec::new() })
}

/// Pure helper: open the project's git repo and return `(name, path)` for
/// every linked worktree. Returns `Ok(vec![])` when the project isn't a git
/// repo or has no worktrees, and a wrapped error when git2 fails (rare —
/// usually a permission issue or a corrupted .git dir).
fn list_worktrees_for_project(project_root: &Path) -> Result<Vec<(String, PathBuf)>> {
    let repo = match rustic_git::GitRepo::open(project_root) {
        Ok(r) => r,
        Err(_) => return Ok(Vec::new()),
    };
    let names = repo.worktrees()?;
    let mut out = Vec::with_capacity(names.len());
    for n in names {
        if let Some(path) = repo.worktree_path(&n) {
            out.push((n, path));
        }
    }
    Ok(out)
}

fn deny_in_chat_or_plan(context: &ToolContext, tool: &str) -> Option<ToolOutput> {
    if context.permissions() == PermissionLevel::Chat {
        return Some(ToolOutput {
            content: format!(
                "PERMISSION_DENIED: `{}` is not allowed in Chat mode — switch to \
                 ManualEdit or above to manage worktrees.",
                tool
            ),
            is_error: true,
            attachments: Vec::new(),
        });
    }
    if context.is_plan_mode {
        return Some(ToolOutput {
            content: format!(
                "PERMISSION_DENIED: `{}` is blocked in plan mode. Worktree management \
                 mutates the on-disk repository state — exit plan mode first.",
                tool
            ),
            is_error: true,
            attachments: Vec::new(),
        });
    }
    None
}

/// Validate `name` is a filesystem-safe slug. We allow lowercase ASCII
/// alphanumerics, `-`, `_`, and `.`; reject leading dots so worktree dirs
/// can't shadow `.git` / `.rustic` semantics by accident.
fn validate_name(name: &str) -> std::result::Result<(), ToolOutput> {
    if name.is_empty() {
        return Err(ToolOutput {
            content: "INVALID_NAME: worktree `name` must be a non-empty string.".into(),
            is_error: true, attachments: Vec::new() });
    }
    if name.len() > 64 {
        return Err(ToolOutput {
            content: "INVALID_NAME: worktree `name` must be 64 chars or fewer.".into(),
            is_error: true, attachments: Vec::new() });
    }
    if name.starts_with('.') {
        return Err(ToolOutput {
            content: "INVALID_NAME: worktree `name` must not start with '.'.".into(),
            is_error: true, attachments: Vec::new() });
    }
    let ok = name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.');
    if !ok {
        return Err(ToolOutput {
            content: "INVALID_NAME: worktree `name` may only contain ASCII alphanumerics, \
                      '-', '_', '.'."
                .into(),
            is_error: true, attachments: Vec::new() });
    }
    Ok(())
}

async fn execute_enter_worktree(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    if let Some(out) = deny_in_chat_or_plan(context, "enter_worktree") {
        return Ok(out);
    }
    if context.is_global {
        return Ok(ToolOutput {
            content: "PERMISSION_DENIED: `enter_worktree` is blocked in the Global scope. \
                      Use `spawn_subtask` to delegate to a project first."
                .into(),
            is_error: true, attachments: Vec::new() });
    }

    let name = params["name"].as_str().unwrap_or("").trim().to_string();
    if let Err(out) = validate_name(&name) {
        return Ok(out);
    }
    let branch = params
        .get("branch")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let worktree_root = context.project_root.join(".rustic").join("worktrees");
    let path: PathBuf = worktree_root.join(&name);

    if path.exists() {
        return Ok(ToolOutput {
            content: format!(
                "WORKTREE_EXISTS: '{}' already exists at '{}'. Pick a different name or \
                 call `exit_worktree` first to remove the old one.",
                name,
                path.display()
            ),
            is_error: true,
            attachments: Vec::new(),
        });
    }

    let project_root = context.project_root.clone();
    let path_for_thread = path.clone();
    let name_for_thread = name.clone();
    let branch_for_thread = branch.clone();
    let result = tokio::task::spawn_blocking(move || -> Result<PathBuf> {
        let repo = rustic_git::GitRepo::open(&project_root)?;
        let abs = repo.add_worktree(
            &name_for_thread,
            &path_for_thread,
            branch_for_thread.as_deref(),
        )?;
        Ok(abs)
    })
    .await;

    let abs_path = match result {
        Ok(Ok(p)) => p,
        Ok(Err(e)) => {
            return Ok(ToolOutput {
                content: format!(
                    "WORKTREE_ADD_FAILED: could not create worktree '{}' at '{}': {}",
                    name,
                    path.display(),
                    e
                ),
                is_error: true,
                attachments: Vec::new(),
            });
        }
        Err(e) => {
            return Ok(ToolOutput {
                content: format!("WORKTREE_ADD_PANICKED: {}", e),
                is_error: true,
                attachments: Vec::new(),
            });
        }
    };

    let branch_label = branch.as_deref().unwrap_or(name.as_str());
    Ok(ToolOutput {
        content: format!(
            "Worktree '{name}' created at {path}.\n\
             - branch: {branch}\n\
             - To run a sub-agent inside this worktree, call \
               `spawn_subagent` with `project_root: \"{path}\"` (the absolute path \
               above). The sub-agent will use that path as its root, letting it edit \
               files there without colliding with siblings working in the main checkout.\n\
             - When done, call `exit_worktree({{ name: \"{name}\" }})` to prune it.",
            name = name,
            path = abs_path.display(),
            branch = branch_label,
        ),
        is_error: false,
        attachments: Vec::new(),
    })
}

async fn execute_exit_worktree(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    if let Some(out) = deny_in_chat_or_plan(context, "exit_worktree") {
        return Ok(out);
    }
    if context.is_global {
        return Ok(ToolOutput {
            content: "PERMISSION_DENIED: `exit_worktree` is blocked in the Global scope."
                .into(),
            is_error: true, attachments: Vec::new() });
    }

    let name = params["name"].as_str().unwrap_or("").trim().to_string();
    if let Err(out) = validate_name(&name) {
        return Ok(out);
    }
    let force = params.get("force").and_then(|v| v.as_bool()).unwrap_or(false);

    let project_root = context.project_root.clone();
    let name_for_thread = name.clone();
    let result = tokio::task::spawn_blocking(move || -> Result<()> {
        let repo = rustic_git::GitRepo::open(&project_root)?;
        repo.remove_worktree(&name_for_thread, force)?;
        Ok(())
    })
    .await;

    match result {
        Ok(Ok(())) => Ok(ToolOutput {
            content: format!(
                "Worktree '{}' removed. Its branch (if any commits were made) is preserved \
                 in the main repository.",
                name
            ),
            is_error: false,
            attachments: Vec::new(),
        }),
        Ok(Err(e)) => Ok(ToolOutput {
            content: format!("WORKTREE_REMOVE_FAILED: {}", e),
            is_error: true,
            attachments: Vec::new(),
        }),
        Err(e) => Ok(ToolOutput {
            content: format!("WORKTREE_REMOVE_PANICKED: {}", e),
            is_error: true,
            attachments: Vec::new(),
        }),
    }
}

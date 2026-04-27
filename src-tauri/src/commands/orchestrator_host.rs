//! Host-side implementation of the `OrchestratorHost` trait.
//!
//! Bridges the rustic-agent crate's orchestrator tools to the app's
//! AppState, workspace, and DB. Keeps all Tauri/DB knowledge out of the
//! agent crate.

use crate::state::{AgentState, AgentTask, AppState};
use rustic_agent::{
    is_global_project_id, OrchestratorHost, OrchestratorMessage, OrchestratorProject,
    OrchestratorTaskFilter, OrchestratorTaskSummary, TaskInfo, TaskStatus,
};
use rustic_db::Database;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager};

pub struct TauriOrchestratorHost {
    app: AppHandle,
    agent: Arc<Mutex<AgentState>>,
    db: Arc<Mutex<Database>>,
}

impl TauriOrchestratorHost {
    pub fn new(app: AppHandle, agent: Arc<Mutex<AgentState>>, db: Arc<Mutex<Database>>) -> Self {
        Self { app, agent, db }
    }
}

impl OrchestratorHost for TauriOrchestratorHost {
    fn list_projects(&self) -> Result<Vec<OrchestratorProject>, String> {
        let state = self.app.state::<AppState>();
        let projects: Vec<_> = {
            let workspace = state.workspace.lock().map_err(|e| e.to_string())?;
            workspace
                .list_projects()
                .into_iter()
                .filter(|p| !is_global_project_id(&p.id))
                .collect()
        };

        tracing::warn!(
            "[orchestrator] list_projects: {} project(s) to walk",
            projects.len()
        );

        // Walk each project's tree off the workspace mutex — `generate_
        // file_tree` reads from disk and we don't want to hold the lock
        // across IO. Use a tight depth/entry cap per project: with many
        // projects the combined output adds up fast, and the orchestrator
        // only needs a structural overview (it can read_file / list_
        // directory when it wants detail). Gitignore-respecting so files
        // the user has excluded from version control stay hidden — common
        // bloat dirs (node_modules, target, .git, dist, etc.) are also
        // dropped regardless of gitignore.
        const ORCHESTRATOR_MAX_DEPTH: usize = 3;
        const ORCHESTRATOR_MAX_ENTRIES: usize = 120;

        Ok(projects
            .into_iter()
            .map(|p| {
                let start = std::time::Instant::now();
                let file_tree = rustic_agent::generate_file_tree_with_limits(
                    &p.root_path,
                    false,
                    ORCHESTRATOR_MAX_DEPTH,
                    ORCHESTRATOR_MAX_ENTRIES,
                );
                let elapsed = start.elapsed();
                tracing::warn!(
                    "[orchestrator] walked {:?} ({} entries, {} ms)",
                    p.root_path,
                    file_tree.lines().count(),
                    elapsed.as_millis()
                );
                OrchestratorProject {
                    file_tree,
                    id: p.id,
                    name: p.name,
                    root_path: p.root_path.to_string_lossy().to_string(),
                }
            })
            .collect())
    }

    fn list_tasks(
        &self,
        filter: OrchestratorTaskFilter,
    ) -> Result<Vec<OrchestratorTaskSummary>, String> {
        let db = self.db.lock().map_err(|e| e.to_string())?;

        let rows = if let Some(ref pid) = filter.project_id {
            db.list_tasks_for_project(pid).map_err(|e| e.to_string())?
        } else {
            db.list_all_tasks().map_err(|e| e.to_string())?
        };

        // Build a project_id → name lookup from the DB so the orchestrator
        // doesn't have to cross-reference itself.
        let project_rows = db.list_projects().map_err(|e| e.to_string())?;
        drop(db);

        let project_name_of = |pid: &str| {
            project_rows
                .iter()
                .find(|p| p.id == pid)
                .map(|p| p.name.clone())
                .unwrap_or_else(|| "(unknown)".to_string())
        };

        let mut out: Vec<OrchestratorTaskSummary> = rows
            .into_iter()
            // Never surface Global's own chats in results — the orchestrator
            // shouldn't be recursing into its own history.
            .filter(|t| !is_global_project_id(&t.project_id))
            .filter(|t| {
                filter
                    .status
                    .as_ref()
                    .map(|s| s.eq_ignore_ascii_case(&t.status))
                    .unwrap_or(true)
            })
            .map(|t| OrchestratorTaskSummary {
                task_id: t.id,
                project_name: project_name_of(&t.project_id),
                project_id: t.project_id,
                title: t.title,
                status: t.status,
                model: t.model,
                created_at: t.created_at,
                updated_at: t.updated_at,
            })
            .collect();

        if let Some(limit) = filter.limit {
            out.truncate(limit);
        }
        Ok(out)
    }

    fn read_task_history(&self, task_id: &str) -> Result<Vec<OrchestratorMessage>, String> {
        // Prefer in-memory if present and non-empty (covers the running-task case).
        {
            let agent = self.agent.lock().map_err(|e| e.to_string())?;
            if let Some(task) = agent.tasks.get(task_id) {
                if !task.messages.is_empty() {
                    return Ok(task
                        .messages
                        .iter()
                        .map(|m| OrchestratorMessage {
                            role: match m.role {
                                rustic_agent::Role::User => "user".to_string(),
                                rustic_agent::Role::Assistant => "assistant".to_string(),
                                rustic_agent::Role::System => "system".to_string(),
                            },
                            content_json: serde_json::to_string(&m.content)
                                .unwrap_or_else(|_| "[]".to_string()),
                        })
                        .collect());
                }
            }
        }

        let db = self.db.lock().map_err(|e| e.to_string())?;
        let rows = db
            .get_messages_for_task(task_id)
            .map_err(|e| e.to_string())?;
        Ok(rows
            .into_iter()
            .map(|r| OrchestratorMessage {
                role: r.role,
                content_json: r.content_json,
            })
            .collect())
    }

    fn spawn_subtask(
        &self,
        project_id: &str,
        title: Option<String>,
        prompt: String,
        parent_task_id: &str,
    ) -> Result<String, String> {
        if is_global_project_id(project_id) {
            return Err("Cannot spawn a sub-task inside the Global scope.".into());
        }

        // Validate project exists.
        let project = {
            let state = self.app.state::<AppState>();
            let workspace = state.workspace.lock().map_err(|e| e.to_string())?;
            workspace
                .list_projects()
                .into_iter()
                .find(|p| p.id == project_id)
                .ok_or_else(|| format!("Project not found: {}", project_id))?
        };

        // Create the task in-memory (mirrors the core of `create_task`).
        let task_id = uuid::Uuid::new_v4().to_string();
        let initial_title = title
            .as_ref()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                // Fall back to a prefix of the prompt — same rule the real
                // create-on-first-message path uses.
                let raw: String = prompt.chars().take(70).collect();
                raw.trim().to_string()
            });
        let initial_title = if initial_title.is_empty() {
            "Subtask".to_string()
        } else {
            initial_title
        };

        // Resolved provider + model, computed inside the lock and surfaced
        // out here so the event emit below can echo them to the frontend.
        let provider_key;
        let model_id;
        {
            let state = self.app.state::<AppState>();
            let mut agent = state.agent.lock().map_err(|e| e.to_string())?;

            // Inherit provider/model from the parent (orchestrator) task so
            // switching models on the global agent carries over to spawned
            // subtasks. Only fall back to ai_config defaults if the parent
            // TaskInfo is gone — shouldn't happen during a live spawn, but
            // the row still needs to be well-formed.
            let (pk, m) = {
                let parent = agent.tasks.get(parent_task_id).map(|t| {
                    (t.info.provider_type.clone(), t.info.model.clone())
                });
                if let Some((pk, m)) = parent.filter(|(pk, m)| !pk.is_empty() && !m.is_empty()) {
                    tracing::warn!(
                        "[orchestrator] spawn_subtask inheriting from parent {}: provider={} model={}",
                        parent_task_id, pk, m
                    );
                    (pk, m)
                } else {
                    tracing::warn!(
                        "[orchestrator] spawn_subtask: parent {} not found, falling back to ai_config defaults",
                        parent_task_id
                    );
                    let pt = agent
                        .ai_config
                        .default_provider
                        .clone()
                        .unwrap_or(rustic_agent::ProviderType::Claude);
                    let entry = agent
                        .ai_config
                        .providers
                        .iter()
                        .find(|p| p.provider_type == pt);
                    let key = entry
                        .map(|e| e.provider_key())
                        .unwrap_or_else(|| format!("{:?}", pt));
                    let m = entry
                        .map(|p| p.default_model.clone())
                        .unwrap_or_else(|| "claude-sonnet-4-20250514".to_string());
                    (key, m)
                }
            };
            provider_key = pk;
            model_id = m;

            let now = chrono::Utc::now().to_rfc3339();
            let info = TaskInfo {
                id: task_id.clone(),
                project_id: project.id.clone(),
                title: initial_title.clone(),
                status: TaskStatus::Completed,
                provider_type: provider_key.clone(),
                model: model_id.clone(),
                created_at: now.clone(),
                updated_at: now,
            };

            // Spawned subtasks always run in FullAuto without sensitive-file
            // access. The orchestrator is expected to fan work out without
            // human intervention; pausing a subtask on a permission prompt
            // would deadlock the parent (no one is watching that project's
            // chat panel). Sensitive-file reads still require an explicit
            // ManualEdit/AutoEdit override, which the user must grant by
            // hand in the subtask's panel.
            agent.tasks.insert(
                task_id.clone(),
                AgentTask {
                    info,
                    messages: Vec::new(),
                    permissions: rustic_agent::PermissionLevel::FullAuto,
                    sensitive_files_allowed: false,
                    shared_permissions: None,
                    cost: Default::default(),
                },
            );
        }

        // Surface the new task in the agent sidebar immediately. The
        // frontend listens for this event, inserts the task into its store,
        // and fires the initial send_message for it.
        let _ = self.app.emit(
            "orchestrator-spawned-task",
            serde_json::json!({
                "task_id": task_id,
                "project_id": project.id,
                "title": initial_title,
                "prompt": prompt,
                "model": model_id,
                "provider_type": provider_key,
                // Hint to the UI that this task is running in FullAuto with
                // sensitive-file reads disabled — keeps the permission pill
                // in the chat toolbar truthful.
                "permission_level": "FullAuto",
                "sensitive_files_allowed": false,
            }),
        );

        Ok(task_id)
    }
}

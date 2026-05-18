//! Harness send-message dispatch (Claude Code, Codex). Called by `send_message`
//! when `provider_type` is a harness key. Bypasses `TaskExecutor` — harnesses
//! own their own tool loop; running the executor would double-inject context.

use crate::commands::agent::{
    AgentCostUpdateEvent, AgentRequestUsageEvent, AgentStatusEvent, AgentStreamEvent,
    AgentSubagentCompletedEvent, AgentSubagentFailedEvent, AgentSubagentSpawnedEvent,
    AgentTaskCompleteEvent, AgentTitleChangedEvent, ImageAttachment,
};
use crate::state::{AgentTask, AppState};
use rustic_agent::{
    calculate_cost, harness::claude_code::ClaudeCodeHarness,
    harness::codex::CodexHarness, ContentBlock, Harness, HarnessEvent, HarnessImage,
    HarnessPermissionMode, HarnessSessionOpts, Message, PermissionLevel, Role,
    TaskStatus, TokenUsage,
};
use rustic_db::{MessageRow, TaskRow};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};

/// Entry point called from `send_message` when the task is bound to a harness
/// provider. Mirrors the in-line bookkeeping that the native path does (task
/// title, persistence, in-memory user-message append) and then hands the
/// actual streaming over to a background thread.
pub fn dispatch_harness_send(
    app: AppHandle,
    state: State<'_, AppState>,
    task_id: String,
    message: String,
    images: Option<Vec<ImageAttachment>>,
) -> Result<(), String> {
    let prep = {
        let mut agent = state.agent.lock().unwrap();
        let task = agent
            .tasks
            .get_mut(&task_id)
            .ok_or_else(|| format!("Task not found: {}", task_id))?;

        let has_user_text = task.messages.iter().any(|m| {
            matches!(m.role, Role::User)
                && m.content.iter().any(|c| matches!(c, ContentBlock::Text { .. }))
        });
        let is_first_message = !has_user_text;

        // Set the task title from the first user message — same shape as
        // the native path, so chat history rows look identical regardless of
        // which provider produced them.
        if is_first_message && (task.info.title.is_empty() || task.info.title == "New Task") {
            let raw: String = message.chars().take(70).collect();
            let safe: String = raw
                .chars()
                .map(|c| if "/\\:*?\"<>|\n\r\t".contains(c) { ' ' } else { c })
                .collect();
            let title = safe.trim().to_string();
            if !title.is_empty() {
                task.info.title = title.clone();
                {
                    let db = state.db.lock().unwrap();
                    let _ = db.update_task_title(&task_id, &title);
                }
                let _ = app.emit(
                    "agent-title-changed",
                    AgentTitleChangedEvent {
                        task_id: task_id.clone(),
                        title,
                    },
                );
            }
        }

        let task_project_id = task.info.project_id.clone();
        let task_provider_type = task.info.provider_type.clone();
        let task_model = task.info.model.clone();
        let task_permissions = task.permissions.clone();

        // Subscription harnesses (Claude Code / Codex) require a real
        // project root: the CLIs scope their session storage / memory by
        // cwd, so Global "no project" chats produce confused output that
        // looks at `~/.claude/projects/...` paths instead of the user's
        // code. The frontend disables Global+harness in the picker, but
        // we also guard it here so any task created before that landed
        // (or any future caller bypassing the UI) gets a clean error.
        if rustic_agent::is_global_project_id(&task_project_id) {
            return Err(
                "Claude Code and Codex don't support Global chats — pick a \
                 project from the Explorer (or switch to an API provider). \
                 The CLI scopes its session storage by project root."
                    .to_string(),
            );
        }

        // Find the project root — harness `cwd` is the project directory.
        let (project_root, project_name) = {
            let workspace = state.workspace.lock().unwrap();
            let proj = workspace
                .list_projects()
                .into_iter()
                .find(|p| p.id.to_string() == task_project_id)
                .ok_or_else(|| "Project not found".to_string())?;
            (proj.root_path.clone(), proj.name.clone())
        };

        // For project chats, cwd = project root. The earlier Global-only
        // home-dir swap is gone now that we hard-block Global at the top
        // of this function — there's no scenario where we want to spawn
        // a harness in `$HOME` anymore.
        let harness_cwd = project_root.clone();

        // Persist the task row on first send (deferred from create_task so
        // empty drafts don't pollute history).
        if is_first_message {
            let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
            let db = state.db.lock().unwrap();
            let actual_project_id = db
                .ensure_project(&rustic_db::models::ProjectRow {
                    id: task_project_id.clone(),
                    name: project_name,
                    root_path: project_root.to_string_lossy().to_string(),
                    created_at: now.clone(),
                    settings_json: None,
                })
                .map_err(|e| format!("Failed to persist project: {}", e))?;
            db.insert_task(&TaskRow {
                id: task_id.clone(),
                project_id: actual_project_id,
                title: task.info.title.clone(),
                status: format!("{:?}", task.info.status),
                provider_type: task_provider_type.clone(),
                model: task_model.clone(),
                created_at: now.clone(),
                updated_at: now,
                total_input_tokens: 0,
                total_output_tokens: 0,
                total_cache_read_tokens: 0,
                estimated_cost_usd: 0.0,
                turn_count: 0,
                harness_session_id: None,
            })
            .map_err(|e| format!("Failed to persist task: {}", e))?;
        }

        // Append the user message into the in-memory transcript. Images are
        // attached as base64 ContentBlocks so the harness writer can lift
        // them straight into a stream-json `image` block.
        let mut user_content = vec![ContentBlock::Text {
            text: message.clone(),
        }];
        if let Some(ref imgs) = images {
            for img in imgs {
                user_content.push(ContentBlock::Image {
                    media_type: img.media_type.clone(),
                    data: img.data.clone(),
                });
            }
        }
        task.messages.push(Message {
            role: Role::User,
            content: user_content,
        });
        task.info.status = TaskStatus::Running;
        // Persist the user message immediately so it survives a crash before
        // the turn completes. Without this, only previous turns are in the DB
        // and the current user message disappears from history on app restart.
        persist_task_messages(&state.db, &task_id, task);

        // Cancellation token — wired into the registry slot so a future
        // `abort_task` can flip it. Interrupt protocol on the CLI side
        // lands in chunk 4; for now we just plumb the token.
        let cancel_token = Arc::new(AtomicBool::new(false));
        agent
            .cancellation_tokens
            .insert(task_id.clone(), Arc::clone(&cancel_token));

        // Optional absolute path to the harness CLI binary, sourced from the
        // ProviderEntry's `base_url` field (we re-use that slot rather than
        // adding a dedicated column — the field is unused by harness
        // providers otherwise, and saves a DB migration). NULL/empty means
        // "use the binary name on PATH" (`claude` for ClaudeCode).
        //
        // Read here (after the last task-mutation use) so the immutable
        // borrow of `agent.ai_config` doesn't conflict with the earlier
        // `&mut agent.tasks` lifetime.
        let binary_path_override = agent
            .ai_config
            .find_by_key(&task_provider_type)
            .and_then(|entry| entry.base_url.clone())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .map(PathBuf::from);

        // MCP passthrough (plan §B.12). Resolve Rustic's user-scope
        // `mcp.json` so the Claude Code CLI can `--mcp-config` it. We pass
        // the path unconditionally and let the harness skip cleanly if the
        // file doesn't exist — that matches "user hasn't opened the MCP
        // panel yet" and "no servers configured" without distinguishing.
        // Project-scope `.mcp.json` is auto-discovered by the CLI from
        // `cwd`, no plumbing needed.
        let mcp_config_path = tauri::Manager::path(&app)
            .app_data_dir()
            .ok()
            .map(|d| d.join("mcp.json"));

        // Reasoning-effort tier from the per-project defaults
        // (`projects.settings_json.thinking_effort`). This is what the
        // chat-view writes whenever the user toggles the effort selector
        // in the agent-config popover (see `saveProjectDefaults` in
        // chat-view.js). Empty string and "off" both mean "no override —
        // let the model decide", same convention as the native pipeline.
        let thinking_effort: Option<String> = {
            let db = state.db.lock().unwrap();
            db.get_project(&task_project_id)
                .ok()
                .flatten()
                .and_then(|p| p.settings_json.clone())
                .and_then(|json| serde_json::from_str::<serde_json::Value>(&json).ok())
                .and_then(|v| {
                    v.get("thinking_effort")
                        .and_then(|e| e.as_str())
                        .map(|s| s.trim().to_string())
                })
                .filter(|s| !s.is_empty() && s != "off")
        };

        HarnessPrep {
            harness_cwd,
            permissions: task_permissions,
            cancel_token,
            user_text: message,
            user_images: images.unwrap_or_default(),
            binary_path_override,
            mcp_config_path,
            model: Some(task_model).filter(|s| !s.is_empty()),
            thinking_effort,
            provider_type: task_provider_type,
        }
    };

    let task_id_thread = task_id.clone();
    let app_thread = app.clone();
    let registry = Arc::clone(&state.harness_registry);
    let agent_arc = Arc::clone(&state.agent);
    let db_arc = Arc::clone(&state.db);
    let task_costs_arc = Arc::clone(&state.task_costs);

    std::thread::spawn(move || {
        let rt = match tokio::runtime::Runtime::new() {
            Ok(rt) => rt,
            Err(e) => {
                tracing::error!(error = %e, "harness runtime: tokio spawn failed");
                return;
            }
        };
        rt.block_on(run_harness_session(
            app_thread,
            task_id_thread,
            prep,
            registry,
            agent_arc,
            db_arc,
            task_costs_arc,
        ));
    });

    Ok(())
}

struct HarnessPrep {
    /// Working directory passed to the harness CLI on spawn. Equals the
    /// project root for project chats; swapped to the user's home dir for
    /// Global chats so Claude Code's Bash/Glob/Read tools see the user's
    /// actual workspace instead of the empty scratch dir.
    harness_cwd: PathBuf,
    permissions: PermissionLevel,
    cancel_token: Arc<AtomicBool>,
    user_text: String,
    user_images: Vec<ImageAttachment>,
    /// Optional absolute path to the CLI binary. Honored on first spawn
    /// (when the registry has no live session for this task); reused
    /// sessions ignore it.
    binary_path_override: Option<PathBuf>,
    /// Path to Rustic's user-scope `mcp.json`, forwarded as `--mcp-config`
    /// when the file exists (plan §B.12). `None` only if the app data dir
    /// could not be resolved.
    mcp_config_path: Option<PathBuf>,
    /// Model identifier the user picked in the agent-config dropdown.
    /// `None` means "let the CLI pick its default" — used as a fallback
    /// when the picker hasn't recorded a model yet (legacy task rows etc.).
    model: Option<String>,
    /// Reasoning-effort level pulled from the project defaults. `None`
    /// means "no override". Forwarded as `--effort` (Claude Code) /
    /// `config.model_reasoning_effort` (Codex) on session spawn.
    thinking_effort: Option<String>,
    /// Stored `provider_type` value (e.g. "ClaudeCode", "Codex"). Used
    /// to pick the right harness factory on spawn (plan §B.10).
    provider_type: String,
}

#[allow(clippy::too_many_arguments)]
async fn run_harness_session(
    app: AppHandle,
    task_id: String,
    prep: HarnessPrep,
    registry: Arc<rustic_agent::HarnessRegistry>,
    agent_arc: Arc<std::sync::Mutex<crate::state::AgentState>>,
    db_arc: Arc<std::sync::Mutex<rustic_db::Database>>,
    task_costs_arc: crate::state::TaskCostMap,
) {
    // Emit Running from the worker thread (matches the native path's race
    // mitigation — the WebView must see Running before any subsequent event).
    let _ = app.emit(
        "agent-task-status",
        AgentStatusEvent {
            task_id: task_id.clone(),
            status: TaskStatus::Running,
        },
    );

    // Get-or-spawn the session. One CLI process per task. If we already have
    // a live session (resume / mid-conversation message) we reuse it; the
    // CLI handles queueing if the previous turn is still streaming.
    let session = match registry.get(&task_id).await {
        Some(s) => s,
        None => {
            // Look up the previously-recorded session id so we can pass
            // `--resume <id>` and pick up where the user left off across
            // app restarts or idle reaps. NULL on first send for a brand-
            // new task; populated by the SessionReady persistence below.
            let resume_session_id = {
                let db = db_arc.lock().ok();
                db.and_then(|db| db.get_task(&task_id).ok().flatten())
                    .and_then(|row| row.harness_session_id)
                    .filter(|s| !s.is_empty())
            };

            let opts = HarnessSessionOpts {
                cwd: prep.harness_cwd.clone(),
                // Chunk 3b restored: the user's task-permission level now
                // controls the CLI's `--permission-mode` flag, and the
                // `can_use_tool` control_request flow surfaces prompts
                // through the existing `agent-permission-request` event.
                permission_mode: map_permission_mode(prep.permissions.clone()),
                resume_session_id,
                binary_path_override: prep.binary_path_override.clone(),
                mcp_config_path: prep.mcp_config_path.clone(),
                model: prep.model.clone(),
                thinking_effort: prep.thinking_effort.clone(),
            };

            // Pick the right harness factory based on the task's saved
            // provider_type (plan §B.10). Codex returns a clear error
            // until the JSON-RPC port lands; that error reaches the user
            // via the `emit_failure` branch below.
            let harness: Box<dyn Harness> = match prep.provider_type.as_str() {
                "ClaudeCode" => Box::new(ClaudeCodeHarness::new()),
                "Codex" => Box::new(CodexHarness::new()),
                other => {
                    emit_failure(
                        &app,
                        &task_id,
                        format!(
                            "Unknown harness provider_type: '{other}'. \
                             Re-enable the provider in Settings → AI Providers."
                        ),
                    );
                    return;
                }
            };
            match harness.start_session(opts).await {
                Ok(sess) => {
                    registry.insert(task_id.clone(), sess.clone()).await;
                    sess
                }
                Err(e) => {
                    // Phrase the install/signin hint per provider so the user
                    // gets the right CLI name in the error bubble. Codex
                    // currently always errors here (skeleton-only — see
                    // `harness/codex.rs`); once the port lands, this branch
                    // surfaces real start-session failures.
                    let hint = match prep.provider_type.as_str() {
                        "Codex" => "Codex's harness implementation is still in progress (plan §B.10).",
                        _ => "Make sure the `claude` CLI is installed and signed in (run `claude` once in a terminal).",
                    };
                    let label = match prep.provider_type.as_str() {
                        "Codex" => "Codex",
                        _ => "Claude Code",
                    };
                    emit_failure(
                        &app,
                        &task_id,
                        format!("Could not start {label}: {e:#}.\n{hint}"),
                    );
                    return;
                }
            }
        }
    };

    // Take the event receiver. If something else already took it (impossible
    // for chunk 2 since this is the only consumer, but defensive), fail
    // loudly — silent loss of the stream is the worst-case bug.
    let mut event_rx = match session.take_event_rx().await {
        Some(rx) => rx,
        None => {
            emit_failure(
                &app,
                &task_id,
                "Harness session event channel was already taken — internal bug.".into(),
            );
            return;
        }
    };

    // Translate user_images and write the message envelope.
    let images: Vec<HarnessImage> = prep
        .user_images
        .into_iter()
        .map(|img| HarnessImage {
            media_type: img.media_type,
            data: img.data,
        })
        .collect();
    if let Err(e) = session.send_user_message(prep.user_text, images).await {
        emit_failure(
            &app,
            &task_id,
            format!("Failed to send message to Claude Code: {e:#}"),
        );
        return;
    }

    // Drive the event loop. We assemble the turn's transcript as we go so
    // reload-after-restart shows tool cards (chunk 6 fix), not just text.
    //
    // Shape we build matches the Anthropic API's interleaved blocks (which is
    // what `chat-view.js` already knows how to render):
    //   Assistant: [Text, ToolUse]
    //   User:      [ToolResult]
    //   Assistant: [Text, ToolUse]
    //   User:      [ToolResult]
    //   Assistant: [Text]
    let mut turn_messages: Vec<Message> = Vec::new();
    // P0.8: also track the CLI's own cost (when reported) and auth mode so
    // the post-turn cost emit can use the trustworthy number instead of a
    // local recompute that misses multi-model usage.
    let mut last_usage: Option<(u32, u32, u32, u32)> = None;
    let mut cli_reported_cost: Option<f64> = None;
    // CLI-reported model name + auth mode, captured from `system:init`. The
    // fallback (prep.model, no auth tag) kicks in when these are None —
    // typically for Codex sessions or older Claude Code versions.
    let mut cli_model: Option<String> = None;
    let mut cli_auth_mode: Option<String> = None;
    let mut turn_complete = false;
    let mut error_message: Option<String> = None;
    // Map of `Task` tool_use_id → slugified agent_id, populated when we see
    // a Task tool_use and consumed when its tool_result arrives. Lets us
    // emit `agent-subagent-completed` / `agent-subagent-failed` in addition
    // to the regular `agent-tool-result` so the existing subagent card
    // (src/components/agent/chat-view.js renderSubagentCard) lights up.
    let mut task_subagents: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    // Tracked so we can detect a `--resume <id>` failure: the CLI exits very
    // early without ever emitting `system:init` when the saved session id
    // refers to a conversation that no longer exists on disk. We treat that
    // case as recoverable by clearing the bad id and asking the user to retry.
    let mut saw_session_ready = false;

    while let Some(ev) = event_rx.recv().await {
        // Honour cancellation between events. We persist whatever partial
        // turn we have so the user keeps the agent text and tool cards they
        // saw before hitting Stop — matches Claude Code's own UI which
        // doesn't blank the conversation on interrupt.
        if prep.cancel_token.load(Ordering::SeqCst) {
            if !turn_messages.is_empty() {
                if let Ok(mut agent) = agent_arc.lock() {
                    if let Some(task) = agent.tasks.get_mut(&task_id) {
                        task.messages.extend(turn_messages);
                        persist_task_messages(&db_arc, &task_id, task);
                    }
                }
            }
            let _ = app.emit(
                "agent-task-status",
                AgentStatusEvent {
                    task_id: task_id.clone(),
                    status: TaskStatus::Cancelled,
                },
            );
            return;
        }

        match ev {
            HarnessEvent::SessionReady {
                session_id,
                model,
                auth_mode,
            } => {
                // Persist the CLI's session id so a future restart can pass
                // `--resume <id>` and restore the conversation history that
                // the CLI itself stores under `~/.claude/projects/`.
                saw_session_ready = true;
                // P0.8: stash the CLI-reported model + auth mode so the cost
                // emit below can prefer them over what the caller passed in.
                if model.is_some() {
                    cli_model = model.clone();
                }
                if auth_mode.is_some() {
                    cli_auth_mode = auth_mode.clone();
                }
                tracing::debug!(
                    task = %task_id,
                    session_id = %session_id,
                    "harness session ready"
                );
                if let Ok(db) = db_arc.lock() {
                    if let Err(e) = db.update_task_harness_session_id(&task_id, &session_id) {
                        tracing::warn!(
                            task = %task_id,
                            error = %e,
                            "failed to persist harness_session_id"
                        );
                    }
                }
            }
            HarnessEvent::TextDelta { text } => {
                append_assistant_text(&mut turn_messages, &text);
                let _ = app.emit(
                    "agent-stream",
                    AgentStreamEvent {
                        task_id: task_id.clone(),
                        text,
                    },
                );
            }
            HarnessEvent::ThinkingDelta { text } => {
                // The native pipeline emits these as `agent-thinking-delta`
                // and the chat view already renders them inside a collapsible
                // pill. Same shape works here — Claude Code only emits
                // thinking deltas if the user has thinking enabled in their
                // CLI config; otherwise this branch never fires.
                let _ = app.emit(
                    "agent-thinking-delta",
                    crate::commands::agent::AgentThinkingDeltaEvent {
                        task_id: task_id.clone(),
                        text,
                    },
                );
            }
            HarnessEvent::ToolUse {
                tool_use_id,
                name,
                input,
                diff_payload: _, // chunk 3c renders diffs frontend-side from input
            } => {
                append_assistant_tool_use(
                    &mut turn_messages,
                    tool_use_id.clone(),
                    name.clone(),
                    input.clone(),
                );
                // Claude Code's `Task` tool spawns a subagent. Translate to
                // the existing subagent event flow so the card renders with
                // a streaming-style header + final-answer button instead of
                // a generic JSON-dump tool card. The card derives its
                // identity by slugifying `description` (matching
                // `slugifyAgentName` in chat-view.js); we mirror that slug
                // here so backend and frontend agree on `agent_id`.
                if name == "Task" {
                    let description = input
                        .get("description")
                        .and_then(|v| v.as_str())
                        .unwrap_or("subagent");
                    let agent_id = slugify_agent_name(description);
                    let prompt = input
                        .get("prompt")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let model = input
                        .get("subagent_type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("claude-code")
                        .to_string();
                    task_subagents.insert(tool_use_id.clone(), agent_id.clone());
                    let _ = app.emit(
                        "agent-subagent-spawned",
                        AgentSubagentSpawnedEvent {
                            task_id: task_id.clone(),
                            agent_id,
                            model,
                            prompt,
                        },
                    );
                }
                let _ = app.emit(
                    "agent-tool-use",
                    crate::commands::agent::AgentToolUseEvent {
                        task_id: task_id.clone(),
                        tool_use_id,
                        tool_name: name,
                        tool_input: input,
                    },
                );
            }
            HarnessEvent::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                append_user_tool_result(
                    &mut turn_messages,
                    tool_use_id.clone(),
                    content.clone(),
                    is_error,
                );
                // If this result belongs to a Task tool_use, fan out to the
                // subagent card lifecycle as well. We don't get streamed
                // sub-agent text from Claude Code (it only emits the final
                // tool_result), so the card transitions straight from
                // `running` (spawn) to `completed`/`failed` here, with the
                // tool_result content as the summary.
                if let Some(agent_id) = task_subagents.remove(&tool_use_id) {
                    if is_error {
                        let _ = app.emit(
                            "agent-subagent-failed",
                            AgentSubagentFailedEvent {
                                task_id: task_id.clone(),
                                agent_id,
                                error: content.clone(),
                            },
                        );
                    } else {
                        let _ = app.emit(
                            "agent-subagent-completed",
                            AgentSubagentCompletedEvent {
                                task_id: task_id.clone(),
                                agent_id,
                                // Harness mode doesn't expose the sub-agent's
                                // resolved model on the result envelope —
                                // leave blank and let the in-flight Spawned
                                // event continue to drive the badge.
                                model: String::new(),
                                summary: content.clone(),
                            },
                        );
                    }
                }
                let _ = app.emit(
                    "agent-tool-result",
                    crate::commands::agent::AgentToolResultEvent {
                        task_id: task_id.clone(),
                        tool_use_id,
                        output: content,
                        is_error,
                    },
                );
                // Persist tool-call history after each complete tool-use +
                // tool-result pair so the DB reflects progress even if the
                // app crashes before TurnComplete. On restart the frontend
                // can show all tool cards rather than blank history.
                snapshot_turn_to_db(&db_arc, &agent_arc, &task_id, &turn_messages);
            }
            HarnessEvent::PermissionRequest {
                request_id,
                tool_use_id,
                tool_name,
                input,
            } => {
                // Translate the harness-shaped event into Rustic's existing
                // `agent-permission-request` envelope so the chat-view
                // permission card lights up the same way it does for
                // native-provider prompts. The card distinguishes between
                // the two only by the new "Allow for session" button.
                //
                // We pass the JSON-pretty-printed tool input as the preview
                // so the user can read what the tool wants to do before
                // clicking. For `Bash` specifically the input has a
                // `command` field; we surface that directly because it's
                // the sensitive bit users actually need to see.
                let preview = build_permission_preview(&tool_name, &input);
                let _ = app.emit(
                    "agent-permission-request",
                    crate::commands::agent::AgentPermissionRequestEvent {
                        task_id: task_id.clone(),
                        request_id: request_id.clone(),
                        operation: tool_name.clone(),
                        description: build_permission_description(&tool_name, &input),
                        preview,
                    },
                );
                // Stash the request id alongside the harness session in the
                // registry so `respond_to_permission` can find it. We don't
                // need a separate map — `harness_registry.get(task_id)` plus
                // the session's internal pending-request map already covers
                // the lookup.
                let _ = (request_id, tool_use_id);
            }
            HarnessEvent::UserQuestion { request_id, question, choices } => {
                let _ = app.emit(
                    "agent-question-request",
                    crate::commands::agent::AgentQuestionRequestEvent {
                        task_id: task_id.clone(),
                        request_id,
                        question,
                        choices,
                    },
                );
            }
            // P0.9 fix #8: typed approval request — currently fires for
            // `exit_plan_mode`. Frontend renders a plan-review card; the
            // user's Approve/Deny routes back through the existing
            // permission path (same wire as can_use_tool).
            HarnessEvent::ApprovalRequest {
                request_id,
                tool_use_id,
                kind,
                payload,
            } => {
                let _ = app.emit(
                    "agent-approval-request",
                    serde_json::json!({
                        "task_id": task_id.clone(),
                        "request_id": request_id,
                        "tool_use_id": tool_use_id,
                        "kind": kind,
                        "payload": payload,
                    }),
                );
            }
            // P0.9 fix #8: typed MCP elicitation prompt. Frontend either
            // renders a schema-driven form or falls back to a free-text
            // dialog with the schema shown for context. Responses route
            // through respond_to_question as a JSON-stringified object.
            HarnessEvent::McpElicit {
                request_id,
                message,
                schema,
            } => {
                let _ = app.emit(
                    "agent-mcp-elicit",
                    serde_json::json!({
                        "task_id": task_id.clone(),
                        "request_id": request_id,
                        "message": message,
                        "schema": schema,
                    }),
                );
            }
            // P0.9 catch-all: any envelope the translator couldn't fully type
            // is surfaced here so the user sees something instead of the
            // agent silently stalling. The frontend renders a generic
            // "harness is asking — here's the raw payload, type a reply"
            // dialog; the user's answer is routed back via the existing
            // `respond_to_question` path if a request_id is known.
            HarnessEvent::UnknownPrompt {
                envelope_type,
                request_id,
                summary,
                raw,
            } => {
                let _ = app.emit(
                    "agent-unknown-prompt",
                    serde_json::json!({
                        "task_id": task_id.clone(),
                        "envelope_type": envelope_type,
                        "request_id": request_id,
                        "summary": summary,
                        "raw": raw,
                    }),
                );
            }
            HarnessEvent::Usage {
                input_tokens,
                output_tokens,
                cache_read_tokens,
                cache_write_tokens,
                rate_limit: _,
                cli_reported_cost_usd,
            } => {
                last_usage = Some((
                    input_tokens,
                    output_tokens,
                    cache_read_tokens,
                    cache_write_tokens,
                ));
                // P0.8: when the CLI sends its own total cost (already summed
                // across all models the turn touched), prefer it over local
                // recompute. Stash here and read at emit time.
                if let Some(c) = cli_reported_cost_usd {
                    cli_reported_cost = Some(c);
                }
            }
            HarnessEvent::TurnComplete => {
                turn_complete = true;
                // Flip the UI to `Completed` immediately so the spinner stops
                // the moment the harness signals end-of-turn. The cleanup
                // below (registry remove, agent-lock + DB persistence of every
                // message in the turn, cost emit, second status update) still
                // runs but no longer keeps the user staring at "Working...".
                // The frontend treats repeat Completed flips as idempotent —
                // the canonical second emit at the end of this function still
                // fires for any late-arriving subscribers.
                let _ = app.emit(
                    "agent-task-status",
                    AgentStatusEvent {
                        task_id: task_id.clone(),
                        status: TaskStatus::Completed,
                    },
                );
                break;
            }
            HarnessEvent::Error { message } => {
                error_message = Some(message);
                break;
            }
        }
    }

    // mpsc closed without TurnComplete → CLI died without a result envelope.
    if !turn_complete && error_message.is_none() {
        let stderr = session.stderr_tail().await;
        let trimmed = stderr.trim();
        let detail = if trimmed.is_empty() {
            "Claude Code exited without finishing the turn (empty stderr).".to_string()
        } else {
            // Limit to ~1000 chars in the user-facing message; the full tail
            // is in the tracing log.
            let snippet: String = trimmed.chars().rev().take(1000).collect::<String>()
                .chars()
                .rev()
                .collect();
            format!("Claude Code exited unexpectedly. Last stderr:\n{}", snippet)
        };
        tracing::warn!(
            task = %task_id,
            saw_session_ready,
            "harness CLI exited before TurnComplete"
        );

        // Specific recovery for `--resume` failures: if the CLI never even
        // emitted `system:init`, the saved session id is almost certainly
        // stale (CLI version change, manually wiped `~/.claude/projects/`,
        // etc.). Clear it from the DB so the next send starts a fresh
        // session. The user just retries.
        if !saw_session_ready {
            if let Ok(db) = db_arc.lock() {
                if let Err(e) = db.update_task_harness_session_id(&task_id, "") {
                    tracing::warn!(task = %task_id, error = %e, "could not clear stale session id");
                }
            }
            error_message = Some(format!(
                "{detail}\n\nThe saved session id may have expired. Send your message \
                 again to start a fresh session."
            ));
        } else {
            error_message = Some(detail);
        }

        registry.remove(&task_id).await;
    }

    // Each turn consumes the event receiver; drop the slot so the next send
    // gets a fresh spawn with --resume <id> for conversation continuity.
    if turn_complete {
        registry.remove(&task_id).await;
    }

    if !turn_messages.is_empty() {
        if let Ok(mut agent) = agent_arc.lock() {
            if let Some(task) = agent.tasks.get_mut(&task_id) {
                task.messages.extend(turn_messages);
                persist_task_messages(&db_arc, &task_id, task);
            }
        }
    }

    // Surface a per-token estimate even in subscription mode so the UI shows
    // meaningful token bars. Cost priority: CLI-reported → local recompute
    // against CLI model → user-picked model → $0.
    if let Some((it, ot, crt, cwt)) = last_usage {
        let resolved_model: Option<&str> = cli_model
            .as_deref()
            .filter(|s| !s.is_empty())
            .or_else(|| prep.model.as_deref().filter(|s| !s.is_empty()));
        let estimated_cost = if let Some(c) = cli_reported_cost {
            c
        } else if let Some(model) = resolved_model {
            calculate_cost(
                model,
                &TokenUsage {
                    input_tokens: it,
                    output_tokens: ot,
                    cache_read_tokens: crt,
                    cache_write_tokens: cwt,
                },
            )
        } else {
            tracing::warn!(
                task = %task_id,
                "harness cost: no model id available — emitting $0 for this turn (P0.8)"
            );
            0.0
        };
        // Cost source tag for the UI: "billed_api" → real money is leaving
        // your account this turn; "estimated_subscription" → your subscription
        // covers it and the dollar number is our best estimate of what an
        // API-equivalent run would cost. Frontend renders the appropriate
        // suffix (" (API)" / " (sub estimate)") next to the cost figure.
        let cost_kind = match cli_auth_mode.as_deref() {
            Some(s) if s.eq_ignore_ascii_case("ANTHROPIC_API_KEY") => "billed_api",
            Some(_) => "estimated_subscription",
            None if cli_reported_cost.is_some() => "billed_unknown",
            None => "estimated_local",
        };
        let _ = app.emit(
            "agent-request-usage",
            AgentRequestUsageEvent {
                task_id: task_id.clone(),
                input_tokens: it,
                output_tokens: ot,
                cache_read_tokens: crt,
                cache_write_tokens: cwt,
                cost_usd: estimated_cost,
            },
        );
        // P0.8: also publish a sidecar event carrying the source tag so the
        // frontend cost panel can show " (API)" / " (sub estimate)" without
        // having to re-derive auth mode from anywhere else.
        let _ = app.emit(
            "agent-cost-source",
            serde_json::json!({
                "task_id": task_id.clone(),
                "cost_kind": cost_kind,
                "model": resolved_model,
                "auth_mode": cli_auth_mode,
            }),
        );
        // Also emit a cumulative CostUpdate so the running totals visible
        // in the chat header advance even for subscription tasks.
        let cumulative = {
            let mut map = task_costs_arc.lock().expect("task_costs poisoned");
            let entry = map.entry(task_id.clone()).or_default();
            entry.total_input_tokens += u64::from(it);
            entry.total_output_tokens += u64::from(ot);
            entry.total_cache_read_tokens += u64::from(crt);
            entry.total_cache_write_tokens += u64::from(cwt);
            entry.estimated_cost_usd += estimated_cost;
            entry.turn_count += 1;
            entry.clone()
        };
        let _ = app.emit(
            "agent-cost-update",
            AgentCostUpdateEvent {
                task_id: task_id.clone(),
                cost: cumulative,
            },
        );
    }

    let final_status = match (turn_complete, &error_message) {
        (_, Some(_)) => TaskStatus::Failed,
        (true, None) => TaskStatus::Completed,
        (false, None) => TaskStatus::Failed,
    };

    if let Ok(mut agent) = agent_arc.lock() {
        if let Some(task) = agent.tasks.get_mut(&task_id) {
            task.info.status = final_status.clone();
        }
    }
    if let Ok(db) = db_arc.lock() {
        let _ = db.update_task_status(&task_id, &format!("{:?}", final_status));
    }

    let _ = app.emit(
        "agent-task-status",
        AgentStatusEvent {
            task_id: task_id.clone(),
            status: final_status.clone(),
        },
    );

    if let Some(msg) = error_message {
        emit_failure(&app, &task_id, msg);
    } else {
        let _ = app.emit(
            "agent-task-complete",
            AgentTaskCompleteEvent {
                task_id: task_id.clone(),
                summary: None,
            },
        );
    }
}

/// Map Rustic's permission level (used for native API providers) onto the
/// Claude Code `--permission-mode` flag the CLI expects. The mapping table
/// matches plan §5; sensitive-files allowed isn't surfaced here because
/// `bypassPermissions` already covers that case.
fn map_permission_mode(level: PermissionLevel) -> HarnessPermissionMode {
    match level {
        PermissionLevel::FullAuto => HarnessPermissionMode::AcceptEdits,
        PermissionLevel::AutoEdit => HarnessPermissionMode::AcceptEdits,
        PermissionLevel::ManualEdit => HarnessPermissionMode::Supervised,
        PermissionLevel::Chat => HarnessPermissionMode::ReadOnly,
    }
}

fn build_permission_description(tool_name: &str, input: &serde_json::Value) -> String {
    match tool_name {
        "Bash" => "Run shell command".to_string(),
        "Edit" | "MultiEdit" => input
            .get("file_path")
            .and_then(|v| v.as_str())
            .map(|p| format!("Edit: {p}"))
            .unwrap_or_else(|| "Edit file".to_string()),
        "Write" => input
            .get("file_path")
            .and_then(|v| v.as_str())
            .map(|p| format!("Write: {p}"))
            .unwrap_or_else(|| "Write file".to_string()),
        "Read" => input
            .get("file_path")
            .and_then(|v| v.as_str())
            .map(|p| format!("Read: {p}"))
            .unwrap_or_else(|| "Read file".to_string()),
        other => format!("Run {other}"),
    }
}

/// Build the secondary preview line for the card. For `Bash` we surface the
/// raw command — same treatment Rustic gives its native `RunCommand`
/// permission preview, untruncated to defeat prompt-injection-via-truncation
/// (a long benign prefix hiding a `&& rm -rf` tail).
fn build_permission_preview(tool_name: &str, input: &serde_json::Value) -> Option<String> {
    if tool_name == "Bash" {
        if let Some(cmd) = input.get("command").and_then(|v| v.as_str()) {
            return Some(cmd.to_string());
        }
    }
    // Fallback: pretty-print the tool input so the user can audit it.
    serde_json::to_string_pretty(input).ok()
}

/// Append a streamed assistant text delta to the turn transcript. If the
/// last message is already an Assistant whose trailing block is a `Text`, we
/// concatenate (so per-character deltas don't blow up into one block per
/// character). Otherwise we open a fresh Assistant message or push a new
/// `Text` block onto the existing one.
fn append_assistant_text(messages: &mut Vec<Message>, text: &str) {
    if !matches!(messages.last(), Some(m) if matches!(m.role, Role::Assistant)) {
        messages.push(Message {
            role: Role::Assistant,
            content: Vec::new(),
        });
    }
    let assistant = messages.last_mut().expect("just pushed");
    match assistant.content.last_mut() {
        Some(ContentBlock::Text { text: existing }) => existing.push_str(text),
        _ => assistant.content.push(ContentBlock::Text {
            text: text.to_string(),
        }),
    }
}

fn append_assistant_tool_use(
    messages: &mut Vec<Message>,
    id: String,
    name: String,
    input: serde_json::Value,
) {
    if !matches!(messages.last(), Some(m) if matches!(m.role, Role::Assistant)) {
        messages.push(Message {
            role: Role::Assistant,
            content: Vec::new(),
        });
    }
    let assistant = messages.last_mut().expect("just pushed");
    assistant.content.push(ContentBlock::ToolUse {
        id,
        name,
        input,
        thought_signature: None,
    });
}

fn append_user_tool_result(
    messages: &mut Vec<Message>,
    tool_use_id: String,
    content: String,
    is_error: bool,
) {
    if !matches!(messages.last(), Some(m) if matches!(m.role, Role::User)) {
        messages.push(Message {
            role: Role::User,
            content: Vec::new(),
        });
    }
    let user = messages.last_mut().expect("just pushed");
    user.content.push(ContentBlock::ToolResult {
        tool_use_id,
        content,
        is_error,
    });
}

fn emit_failure(app: &AppHandle, task_id: &str, message: String) {
    // We re-use `agent-stream` to surface the error inline so the chat shows
    // the failure reason without needing a new event type. Chunk 3 will add
    // a dedicated `agent-task-error` event for cleaner UX.
    let _ = app.emit(
        "agent-stream",
        AgentStreamEvent {
            task_id: task_id.to_string(),
            text: format!("\n\n**Error:** {message}\n"),
        },
    );
    let _ = app.emit(
        "agent-task-status",
        AgentStatusEvent {
            task_id: task_id.to_string(),
            status: TaskStatus::Failed,
        },
    );
}

fn persist_messages_slice(
    db_arc: &Arc<std::sync::Mutex<rustic_db::Database>>,
    task_id: &str,
    messages: &[Message],
) {
    let Ok(db) = db_arc.lock() else { return };
    let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let mut rows = Vec::with_capacity(messages.len());
    for (i, msg) in messages.iter().enumerate() {
        let role = match &msg.role {
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::System => "system",
        };
        let content_json = match serde_json::to_string(&msg.content) {
            Ok(s) => s,
            Err(_) => continue,
        };
        rows.push(MessageRow {
            id: format!("{}-{}", task_id, i),
            task_id: task_id.to_string(),
            role: role.to_string(),
            content_json,
            created_at: now.clone(),
            sort_order: i as i64,
            turn_usage_json: None,
        });
    }
    // Wrapped in a transaction so a mid-write crash doesn't leave the
    // messages table empty after DELETE committed but before INSERTs ran.
    let _ = db.replace_messages_for_task(task_id, &rows);
}

fn persist_task_messages(
    db_arc: &Arc<std::sync::Mutex<rustic_db::Database>>,
    task_id: &str,
    task: &AgentTask,
) {
    persist_messages_slice(db_arc, task_id, &task.messages);
}

/// Snapshot the in-progress turn to the DB by combining the task's committed
/// history with the current turn's accumulated messages. Called after each
/// complete tool-use/result pair so tool-call history survives a crash.
fn snapshot_turn_to_db(
    db_arc: &Arc<std::sync::Mutex<rustic_db::Database>>,
    agent_arc: &Arc<std::sync::Mutex<crate::state::AgentState>>,
    task_id: &str,
    turn_messages: &[Message],
) {
    let base: Vec<Message> = {
        let Ok(agent) = agent_arc.lock() else { return };
        let Some(task) = agent.tasks.get(task_id) else { return };
        task.messages.clone()
    };
    let mut combined = base;
    combined.extend_from_slice(turn_messages);
    persist_messages_slice(db_arc, task_id, &combined);
}

/// Must match `slugifyAgentName` in chat-view.js exactly (no-collapsing quirk
/// included) so backend and frontend agree on agent_id.
fn slugify_agent_name(name: &str) -> String {
    let lowered = name.to_ascii_lowercase();
    let mut out: String = lowered
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let leading = out.chars().take_while(|c| *c == '-').count();
    out.drain(..leading);
    while out.ends_with('-') {
        out.pop();
    }
    if out.len() > 30 {
        out.truncate(30);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::slugify_agent_name;

    #[test]
    fn slugify_matches_frontend_helper() {
        // Mirrors the JS slugifyAgentName: each non-alphanum becomes a hyphen
        // (no run collapsing), then leading/trailing hyphens trim, then 30-cap.
        assert_eq!(slugify_agent_name("Investigate Auth Bug"), "investigate-auth-bug");
        assert_eq!(slugify_agent_name("  Find &  fix bug  "), "find----fix-bug");
        assert_eq!(slugify_agent_name("///---///"), "");
        // 30-char cap (counted in raw bytes, matching JS slice(0, 30)).
        let long = "a-very-long-description-that-exceeds-thirty-chars";
        let expected: String = long.chars().take(30).collect();
        assert_eq!(slugify_agent_name(long), expected);
    }
}

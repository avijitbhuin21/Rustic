mod commands;
mod logging;
mod path_scope;
mod secrets;
mod state;
mod watcher;

use tauri::{Emitter, Manager, WindowEvent};

use state::AppState;

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            // Second instance was launched. Forward any path argument to the
            // existing window and bring it to the foreground so the user
            // doesn't end up with a dropped command.
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.set_focus();
                let path_arg: Option<String> = args
                    .iter()
                    .skip(1)
                    .find(|a| !a.starts_with('-'))
                    .map(|s| s.to_string());
                if let Some(p) = path_arg {
                    let _ = window.emit("rustic:open-path", p);
                }
            }
        }))
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                // Defer to the frontend so it can prompt about dirty buffers.
                // The frontend either calls `confirm_quit` (which uses
                // app.exit and bypasses this handler) or does nothing.
                api.prevent_close();
                let _ = window.emit("rustic:close-requested", ());
            }
        })
        .setup(|app| {
            // Startup is allowed to fail loudly — but we use `Box<dyn Error>`
            // back to Tauri (which shows a native error dialog and exits)
            // instead of `panic!`, which dumps a useless backtrace into
            // stderr that the user never sees.
            let app_data_dir = app.path().app_data_dir()
                .map_err(|e| format!("Cannot resolve app data directory: {}", e))?;

            // Initialise logging FIRST so every subsequent step's tracing
            // events make it into the rotating log file. In a release build
            // (`windows_subsystem = "windows"`) this is the only place a
            // panic / startup-failure message is going to be readable.
            if let Err(e) = logging::init(&app_data_dir) {
                eprintln!("[startup] failed to initialise logging: {}", e);
            }

            let db_path = app_data_dir.join("rustic.db");

            let global_root = app_data_dir.join("global_scope");
            std::fs::create_dir_all(&global_root).ok();

            let db = rustic_db::Database::new(&db_path).map_err(|e| {
                tracing::error!(error = %e, db_path = %db_path.display(), "database init failed");
                format!(
                    "Could not open the Rustic database at {}.\n\n\
                     Reason: {}\n\n\
                     If the file looks corrupt, you can move it aside and \
                     restart — Rustic will create a fresh database. Backups \
                     of past schema migrations live next to it as \
                     `rustic.db.bak.<timestamp>`.",
                    db_path.display(),
                    e,
                )
            })?;

            // Register the Global pseudo-project in the DB so the FK on
            // `tasks.project_id` succeeds and `workspace.list_projects()`
            // can resolve it like any other project.
            {
                let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
                let _ = db.insert_project(&rustic_db::models::ProjectRow {
                    id: rustic_agent::GLOBAL_PROJECT_ID.to_string(),
                    name: "Global".to_string(),
                    root_path: global_root.to_string_lossy().to_string(),
                    created_at: now,
                    settings_json: None,
                });
            }

            let app_state = AppState::new(db);

            // Restore persisted AI config (API keys, models) and tool config (web_search/fetch toggles).
            //
            // API keys live in the OS keychain (since the secrets-migration
            // patch). The on-disk `ai_config` JSON has empty `api_key` fields;
            // we hydrate them from the keychain at startup so the agent loop
            // can read them like before.
            //
            // For backwards-compat: if SQLite still contains a non-empty
            // `api_key` (legacy install), migrate it into the keychain on
            // first launch and blank out the SQLite copy.
            {
                let db = app_state.db.lock().unwrap();
                if let Ok(Some(json)) = db.get_setting("ai_config") {
                    if let Ok(mut config) = serde_json::from_str::<rustic_agent::AiConfig>(&json) {
                        let mut migrated = false;
                        tracing::info!(
                            providers_in_sqlite = config.providers.len(),
                            "[secrets] hydrating provider keys from keychain"
                        );
                        for entry in config.providers.iter_mut() {
                            let provider_str = entry.provider_type.as_str();
                            let acct = secrets::provider_account(provider_str, entry.name.as_deref());

                            if !entry.api_key.is_empty() {
                                // Legacy plaintext key in SQLite — migrate.
                                match secrets::set(&acct, &entry.api_key) {
                                    Ok(()) => {
                                        entry.api_key.clear();
                                        migrated = true;
                                        tracing::info!(account = %acct, "[secrets] migrated legacy plaintext key to keychain");
                                    }
                                    Err(e) => {
                                        // Leave the plaintext key in place — better
                                        // for the user than silently losing it.
                                        tracing::warn!(account = %acct, error = %e, "[secrets] migration failed; key stays in SQLite");
                                    }
                                }
                            } else {
                                // Hydrate from keychain into the in-memory copy.
                                // CRITICAL: distinguish "not found" (Ok(None) — fine, user
                                // hasn't configured this provider) from "transient error"
                                // (Err(..) — keychain hiccup). The previous code lumped
                                // both into a silent skip, which made transient errors
                                // look identical to a missing key, leaving the user with
                                // a re-auth prompt for a provider they thought was set up.
                                match secrets::get(&acct) {
                                    Ok(Some(secret)) => {
                                        entry.api_key = secret;
                                        tracing::info!(account = %acct, "[secrets] hydrated key from keychain");
                                    }
                                    Ok(None) => {
                                        tracing::info!(account = %acct, "[secrets] no keychain entry — provider not configured");
                                    }
                                    Err(e) => {
                                        tracing::error!(account = %acct, error = %e, "[secrets] keychain GET FAILED — key not hydrated; provider will appear unconfigured this session");
                                    }
                                }
                            }
                        }

                        if migrated {
                            // Persist the redacted JSON so we don't run the
                            // legacy migration again next launch.
                            let mut redacted = config.clone();
                            for entry in redacted.providers.iter_mut() {
                                entry.api_key.clear();
                            }
                            if let Ok(redacted_json) = serde_json::to_string(&redacted) {
                                let _ = db.set_setting("ai_config", &redacted_json);
                            }
                        }

                        app_state.agent.lock().unwrap().ai_config = config;
                    }
                }
                if let Ok(Some(json)) = db.get_setting("tool_config") {
                    if let Ok(config) = serde_json::from_str(&json) {
                        app_state.agent.lock().unwrap().tool_config = config;
                    }
                }
            }

            // Hydrate the GitHub token from the OS keychain (if previously
            // stored via git_set_token / github_poll_token).
            if let Ok(Some(tok)) = secrets::get(commands::git::GIT_TOKEN_ACCOUNT) {
                *app_state.git_token.lock().unwrap() = Some(tok);
            }

            app.manage(app_state);

            // Create default ~/projects directory so git clone has a sensible home.
            if let Ok(home) = app.path().home_dir() {
                std::fs::create_dir_all(home.join("projects")).ok();
            }

            // Seed built-in default workflows into ~/.rustic/workflows/.
            // Idempotent; respects user deletions via a .seeded-defaults marker.
            rustic_agent::seed_default_workflows();

            // Load persisted projects into the in-memory workspace and
            // start file watchers for them. The Global pseudo-project is
            // loaded too so send_message's project lookup can resolve it,
            // but we skip the file watcher for it (its root is internal).
            {
                let state = app.state::<AppState>();
                let projects = {
                    let db = state.db.lock().unwrap();
                    db.list_projects().unwrap_or_default()
                };
                {
                    let mut workspace = state.workspace.lock().unwrap();
                    for row in &projects {
                        let path = std::path::PathBuf::from(&row.root_path);
                        // Use existing row id (don't let Project::new generate a new one).
                        if !workspace.projects.iter().any(|p| p.id == row.id) {
                            let mut project = rustic_core::workspace::project::Project::new(path);
                            project.id = row.id.clone();
                            project.name = row.name.clone();
                            workspace.projects.push(project);
                        }
                    }
                }
                let mut watcher = state.file_watcher.lock().unwrap();
                for project in &projects {
                    if project.id == rustic_agent::GLOBAL_PROJECT_ID {
                        continue;
                    }
                    watcher.watch_project(&project.root_path, app.handle().clone());
                }
                drop(watcher);

                // Reconcile any orphan blob files left behind by a previous
                // crash. Cheap: stat-walks `{app_data}/file-history/blobs/`
                // once per project. Skipped silently if the dir doesn't
                // exist yet (first run after the migration).
                let project_roots: Vec<String> = projects
                    .iter()
                    .filter(|p| p.id != rustic_agent::GLOBAL_PROJECT_ID)
                    .map(|p| p.root_path.clone())
                    .collect();
                crate::commands::file_history::reconcile_all_projects(
                    &state,
                    app.handle(),
                    &project_roots,
                );
            }

            // Idle reaper for harness CLI processes (plan §B.5). Every 60s,
            // drop any session whose last_active is older than 15 minutes.
            // Each `claude` child holds ~150–300 MB of Node memory; users
            // who leave many tasks open would otherwise pay for all of them
            // simultaneously. Resume on next message-send is automatic via
            // the persisted `harness_session_id` + `--resume <id>` (chunk 4b).
            {
                let registry = app.state::<AppState>().harness_registry.clone();
                tauri::async_runtime::spawn(async move {
                    let mut interval =
                        tokio::time::interval(std::time::Duration::from_secs(60));
                    // First tick fires immediately — skip it so we don't
                    // reap on startup (no sessions to reap, but it also
                    // saves us a no-op log line).
                    interval.tick().await;
                    let threshold = std::time::Duration::from_secs(15 * 60);
                    loop {
                        interval.tick().await;
                        registry.reap_idle(threshold).await;
                    }
                });
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app::confirm_quit,
            commands::app::get_logs_dir,
            commands::app::list_log_files,
            commands::app::read_log_file,
            commands::workspace::add_project,
            commands::workspace::remove_project,
            commands::workspace::list_projects,
            commands::file_tree::read_dir,
            commands::file_tree::list_project_files,
            commands::file_tree::read_file_content,
            commands::file_tree::create_file,
            commands::file_tree::create_folder,
            commands::file_tree::rename_entry,
            commands::file_tree::delete_entry,
            commands::file_tree::copy_entry,
            commands::file_tree::stat_path,
            commands::file_tree::read_clipboard_files,
            commands::file_tree::write_clipboard_files,
            commands::file_tree::reveal_in_file_manager,




            commands::editor::open_file,
            commands::editor::open_scratch_buffer,
            commands::editor::get_visible_lines,
            commands::editor::highlight_buffer,
            commands::editor::highlight_range,
            commands::editor::edit_buffer,
            commands::editor::format_buffer,
            commands::editor::save_file,
            commands::editor::buffer_external_change,
            commands::editor::reload_buffer,
            commands::editor::undo_edit,
            commands::editor::redo_edit,
            commands::editor::close_buffer,
            commands::terminal::create_terminal,
            commands::terminal::write_terminal,
            commands::terminal::resize_terminal,
            commands::terminal::close_terminal,
            commands::terminal::list_terminals,
            commands::terminal::detect_shells,
            commands::search::start_search,
            commands::search::cancel_search,
            commands::search::replace_in_file,
            commands::git::git_status,
            commands::git::git_stage,
            commands::git::git_unstage,
            commands::git::git_commit,
            commands::git::git_discard,
            commands::git::git_diff,
            commands::git::git_diff_staged,
            commands::git::git_branches,
            commands::git::git_init,
            commands::git::git_push,
            commands::git::git_pull,
            commands::git::git_fetch,
            commands::git::git_ahead_behind,
            commands::git::git_checkout_branch,
            commands::git::git_create_branch,
            commands::git::git_rebase,
            commands::git::git_rebase_continue,
            commands::git::git_rebase_abort,
            commands::git::git_get_conflicts,
            commands::git::git_resolve_conflict,
            commands::git::git_merge_commit,
            commands::git::git_set_token,
            commands::git::git_get_token,
            commands::git::git_add_to_gitignore,
            commands::git::git_add_remote,
            commands::git::git_get_remote_url,
            commands::git::get_default_projects_dir,
            commands::git::git_clone,
            commands::git::git_log,
            commands::git::git_commit_files,
            commands::git::git_commit_file_diff,
            commands::git::git_unpushed_commits,
            commands::git::git_undo_last_commit,
            commands::git::github_device_code,
            commands::git::github_poll_token,
            commands::git::github_get_user,
            commands::agent::create_task,
            commands::agent::send_message,
            commands::agent::list_tasks,
            commands::agent::get_task_messages,
            commands::agent::get_task_todos,
            commands::agent::get_subagent_records,
            commands::agent::delete_task,
            commands::agent::delete_tasks_for_project,
            commands::agent::truncate_task_messages,
            commands::agent::rename_task,
            commands::agent::set_ai_provider,
            commands::agent::get_ai_config,
            commands::agent::set_subagent_config,
            commands::agent::clear_subagent_config,
            commands::agent::set_model_capabilities,
            commands::agent::get_model_capabilities,
            commands::agent::remove_ai_provider,
            commands::agent::get_tool_config,
            commands::agent::set_tool_config,
            commands::agent::fetch_ai_models,
            commands::agent::list_known_models,
            commands::agent::set_permissions,
            commands::agent::set_task_permissions,
            commands::agent::read_mcp_json,
            commands::agent::save_mcp_json,
            commands::agent::remove_mcp_server,
            commands::agent::list_mcp_servers,
            commands::agent::test_mcp_server,
            commands::agent::abort_task,
            commands::agent::probe_harness_auth,
            commands::agent::list_claude_code_slash_commands,
            commands::agent::list_claude_code_models,
            commands::agent::list_codex_models,
            commands::agent::respond_to_permission,
            commands::agent::set_task_sensitive_access,
            commands::agent::get_task_cost,
            commands::agent::harness_active_task_ids,
            commands::agent::notify_input_queued,
            commands::agent::notify_input_delivered,
            commands::agent::get_memory,
            commands::agent::clear_memory,
            commands::agent::switch_model,
            commands::agent::get_project_defaults,
            commands::agent::save_project_defaults,
            commands::skills::list_skills,
            commands::skills::get_skill_body,
            commands::skills::create_skill,
            commands::skills::update_skill,
            commands::skills::delete_skill,
            commands::skills::list_repo_skills,
            commands::skills::preview_repo_skill,
            commands::skills::install_repo_skills,
            commands::workflows::list_workflows,
            commands::workflows::get_workflow_body,
            commands::workflows::create_workflow,
            commands::workflows::update_workflow,
            commands::workflows::delete_workflow,
            commands::workflows::list_repo_workflows,
            commands::workflows::preview_repo_workflow,
            commands::workflows::install_repo_workflows,
            commands::rules::list_rules,
            commands::rules::get_rule_body,
            commands::rules::create_rule,
            commands::rules::update_rule,
            commands::rules::delete_rule,
            commands::rules::set_rule_activation,
            commands::settings::get_settings,
            commands::settings::update_settings,
            commands::settings::get_active_theme,
            commands::settings::list_themes,
            commands::settings::import_theme,
            commands::settings::import_keybindings,
            commands::settings::detect_vscode_keybindings,
            commands::preview::read_file_base64,
            commands::preview::write_file_base64,
            commands::preview::read_hex_chunk,
            commands::preview::get_file_size,
            commands::file_history::fh_list_files,
            commands::file_history::fh_file_diff,
            commands::file_history::fh_revert,
            commands::file_history::fh_revert_from_message,
            commands::file_history::fh_revert_task,
            commands::file_history::fh_plan_revert_from_message,
            commands::file_history::fh_plan_revert_task,
            commands::file_history::fh_list_snapshots,
            commands::file_history::fh_list_task_net_changes,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

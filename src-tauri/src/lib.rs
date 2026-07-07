mod app_icon;
mod app_paths;
mod commands;
mod logging;
mod path_scope;
mod secrets;
mod state;
mod sync_ext;
mod transport;
mod watcher;

use crate::sync_ext::MutexExt;
use std::sync::Arc;
use tauri::{Emitter, Manager, WindowEvent};

use state::AppState;

pub fn run() {
    #[allow(unused_mut)] // Used in release builds for single-instance plugin
    let mut builder = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init());

    // Single-instance is RELEASE-ONLY. The lock keys off the bundle
    // identifier, which dev and production share — with it installed, starting
    // `bun tauri dev` while the installed app is running just focuses the
    // production window and the dev instance exits ("the port is taken").
    // Skipping it in debug lets a dev build launch independently; combined with
    // the `-dev` app-data dir (see app_paths) the two run fully isolated.
    #[cfg(not(debug_assertions))]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
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
        }));
    }

    builder
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                // Defer to frontend: may prompt about dirty buffers.
                // Frontend calls `confirm_quit` (app.exit) or does nothing.
                api.prevent_close();
                let _ = window.emit("rustic:close-requested", ());
            }
        })
        .setup(|app| {
            // Use Box<dyn Error> → Tauri shows a native error dialog (not panic).
            let app_data_dir = crate::app_paths::app_data_dir(app.handle())
                .map_err(|e| format!("Cannot resolve app data directory: {}", e))?;

            // Init logging first — in windows_subsystem="windows" builds this is
            // the only readable location for a startup failure.
            if let Err(e) = logging::init(&app_data_dir) {
                eprintln!("[startup] failed to initialise logging: {}", e);
            }

            // Load the persisted FreeBuff token pool so multi-account failover
            // is live from the first request, before Settings is ever opened.
            commands::agent::hydrate_freebuff_pool(&app_data_dir);

            let db_path = app_data_dir.join("rustic.db");

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

            let app_state = AppState::new(db);

            // F-10: gate project-scope .mcp.json auto-load on content-hash consent.
            {
                let mcp_arc = Arc::clone(&app_state.agent.lock_safe().mcp_manager);
                let consent_path = app_data_dir.join("mcp_consent.json");
                let mut mcp = mcp_arc.lock_safe();
                mcp.set_consent_path(consent_path);
            }

            // Restore AI config and hydrate API keys from the OS keychain.
            // Migrate any legacy plaintext keys found in SQLite to the keychain.
            {
                let db = app_state.db.lock_safe();
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
                                // Distinguish Ok(None) (not configured) from Err (keychain hiccup);
                                // previous code lumped both, masking keychain failures.
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
                            let mut redacted = config.clone();
                            for entry in redacted.providers.iter_mut() {
                                entry.api_key.clear();
                            }
                            if let Ok(redacted_json) = serde_json::to_string(&redacted) {
                                let _ = db.set_setting("ai_config", &redacted_json);
                            }
                        }

                        app_state.agent.lock_safe().ai_config = config;
                    }
                }
                if let Ok(Some(json)) = db.get_setting("tool_config") {
                    if let Ok(config) = serde_json::from_str(&json) {
                        app_state.agent.lock_safe().tool_config = config;
                    }
                }
            }

            if let Ok(Some(tok)) = secrets::get(commands::git::GIT_TOKEN_ACCOUNT) {
                *app_state.git_token.lock_safe() = Some(tok);
            }

            app.manage(app_state);

            if let Ok(home) = app.path().home_dir() {
                std::fs::create_dir_all(home.join("projects")).ok();
            }

            rustic_agent::seed_default_workflows();

            {
                let state = app.state::<AppState>();
                let projects = {
                    let db = state.db.lock_safe();
                    db.list_projects().unwrap_or_default()
                };
                {
                    let mut workspace = state.workspace.lock_safe();
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
                let t_watch = std::time::Instant::now();
                let mut watcher = state.file_watcher.lock_safe();
                for project in &projects {
                    let emitter: std::sync::Arc<dyn rustic_app::EventEmitter> =
                        std::sync::Arc::new(crate::transport::TauriEmitter::new(app.handle().clone()));
                    watcher.watch_project(
                        &project.root_path,
                        emitter,
                        Some(state.workspace_services.clone()),
                    );
                }
                drop(watcher);
                tracing::info!(target: "rustic::timing", projects = projects.len(), elapsed_ms = t_watch.elapsed().as_millis() as u64, "startup: project watchers registered");

                let project_roots: Vec<String> = projects
                    .iter()
                    .map(|p| p.root_path.clone())
                    .collect();
                let t_reconcile = std::time::Instant::now();
                crate::commands::file_history::reconcile_all_projects(
                    &state,
                    app.handle(),
                    &project_roots,
                );
                tracing::info!(target: "rustic::timing", elapsed_ms = t_reconcile.elapsed().as_millis() as u64, "startup: file-history reconcile_all_projects");

                let t_blob = std::time::Instant::now();
                crate::commands::file_history::cleanup_legacy_blob_store(app.handle());
                tracing::info!(target: "rustic::timing", elapsed_ms = t_blob.elapsed().as_millis() as u64, "startup: legacy blob-store cleanup");
            }

            if let Some(window) = app.get_webview_window("main") {
                app_icon::apply(&window);
            }

            tracing::info!(target: "rustic::timing", "startup: setup complete — IPC now serving");

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app::confirm_quit,
            commands::app::log_frontend_error,
            commands::app::get_logs_dir,
            commands::app::list_log_files,
            commands::app::read_log_file,
            commands::workspace::add_project,
            commands::workspace::remove_project,
            commands::workspace::list_projects,
            commands::workspace::reorder_projects,
            commands::workspace::list_project_worktrees,
            commands::file_tree::read_dir,
            commands::file_tree::list_project_files,
            commands::file_tree::read_file_content,
            commands::file_tree::create_file,
            commands::file_tree::create_folder,
            commands::file_tree::rename_entry,
            commands::file_tree::delete_entry,
            commands::file_tree::copy_entry,
            commands::file_tree::move_entry,
            commands::file_tree::stat_path,
            commands::file_tree::read_clipboard_files,
            commands::file_tree::write_clipboard_files,
            commands::file_tree::begin_drop_upload,
            commands::file_tree::write_upload_chunk,
            commands::file_tree::paste_clipboard_image_into,
            commands::file_tree::save_pasted_image_base64,
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
            commands::terminal::read_terminal_screen,
            commands::terminal::read_terminal_buffer,
            commands::terminal::read_terminal_scrollback,
            commands::terminal::detect_shells,
            commands::search::start_search,
            commands::search::cancel_search,
            commands::search::replace_in_file,
            commands::search::replace_all_in_files,
            commands::git::git_check_available,
            commands::git::git_status,
            commands::git::git_stage,
            commands::git::git_unstage,
            commands::git::git_commit,
            commands::git::git_discard,
            commands::git::git_stage_all,
            commands::git::git_unstage_all,
            commands::git::git_discard_all,
            commands::git::git_diff,
            commands::git::git_diff_staged,
            commands::git::git_branches,
            commands::git::git_init,
            commands::git::git_push,
            commands::git::git_publish_branch,
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
            commands::git::git_is_repo,
            commands::git::github_create_repo,
            commands::git::github_device_code,
            commands::git::github_poll_token,
            commands::git::github_get_user,
            commands::agent::create_task,
            commands::agent::send_message,
            commands::agent::list_tasks,
            commands::agent::get_task_messages,
            commands::agent::repair_task_history,
            commands::agent::get_task_todos,
            commands::agent::get_subagent_records,
            commands::agent::delete_task,
            commands::agent::delete_tasks_for_project,
            commands::agent::truncate_task_messages,
            commands::agent::rename_task,
            commands::agent::set_task_pinned,
            commands::agent::set_task_goal,
            commands::agent::set_ai_provider,
            commands::agent::get_ai_config,
            commands::agent::detect_freebuff,
            commands::agent::freebuff_list_tokens,
            commands::agent::freebuff_add_current_login,
            commands::agent::freebuff_add_tokens,
            commands::agent::freebuff_remove_token,
            commands::agent::set_subagent_config,
            commands::agent::clear_subagent_config,
            commands::agent::set_audio_input_config,
            commands::agent::clear_audio_input_config,
            commands::agent::transcribe_audio,
            commands::agent::set_source_control_config,
            commands::agent::clear_source_control_config,
            commands::agent::generate_commit_message,
            commands::agent::set_model_capabilities,
            commands::agent::get_model_capabilities,
            commands::agent::set_openrouter_provider_allowlist,
            commands::agent::get_openrouter_provider_allowlist,
            commands::agent::remove_ai_provider,
            commands::agent::get_tool_config,
            commands::agent::set_tool_config,
            commands::agent::fetch_ai_models,
            commands::agent::list_known_models,
            commands::agent::fetch_openrouter_model_specs,
            commands::agent::fetch_openrouter_providers,
            commands::agent::set_permissions,
            commands::agent::set_task_permissions,
            commands::agent::read_mcp_json,
            commands::agent::save_mcp_json,
            commands::agent::remove_mcp_server,
            commands::agent::list_mcp_servers,
            commands::agent::list_mcp_server_tools,
            commands::agent::test_mcp_server,
            commands::agent::get_pending_mcp_consent,
            commands::agent::approve_mcp_project_consent,
            commands::agent::revoke_mcp_project_consent,
            commands::agent::abort_task,
            commands::agent::respond_to_permission,
            commands::agent::set_task_sensitive_access,
            commands::agent::set_task_plan_mode,
            commands::agent::respond_to_ask_user,
            commands::agent::respond_to_ceiling_breach,
            commands::agent::set_budget_settings,
            commands::agent::get_budget_settings,
            commands::agent::set_subagent_concurrency_cap,
            commands::agent::get_subagent_concurrency_cap,
            commands::agent::get_task_cost,
            commands::agent::notify_input_queued,
            commands::agent::notify_input_delivered,
            commands::agent::get_memory,
            commands::agent::clear_memory,
            commands::agent::switch_model,
        commands::agent::set_task_thinking_tier,
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
            commands::rules::set_rule_projects,
            commands::settings::get_settings,
            commands::settings::update_settings,
            commands::settings::get_active_theme,
            commands::settings::list_themes,
            commands::settings::import_theme,
            commands::settings::import_theme_json,
            commands::settings::get_theme,
            commands::settings::delete_theme,
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
            commands::file_history::fh_list_task_paths,
            commands::file_history::fh_revert_path,
            commands::formatters::formatter_registry,
            commands::formatters::formatter_list,
            commands::formatters::formatter_install,
            commands::formatters::formatter_update,
            commands::formatters::formatter_check_update,
            commands::formatters::formatter_uninstall,
            commands::formatters::formatter_add_custom,
            commands::formatters::formatter_update_custom,
            commands::formatters::formatter_remove_custom,
            commands::formatters::formatter_format,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

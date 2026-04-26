mod commands;
mod state;
mod watcher;

use tauri::Manager;

use state::AppState;

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        .setup(|app| {
            let app_data_dir = app.path().app_data_dir()
                .expect("Failed to resolve app data directory");
            let db_path = app_data_dir.join("rustic.db");
            // Project snapshots live alongside the DB so deleting app data
            // wipes them too. Laid out as <root>/<task_id>/<checkpoint_id>/.
            let snapshot_root = app_data_dir.join("checkpoint_snapshots");
            std::fs::create_dir_all(&snapshot_root).ok();

            // Dedicated directory for the Global orchestrator scope. Serves
            // as a real project root so memory/snapshot/prompt code paths
            // don't have to be hardened against a non-path project_root.
            let global_root = app_data_dir.join("global_scope");
            std::fs::create_dir_all(&global_root).ok();

            let db = rustic_db::Database::new(&db_path)
                .expect("Failed to initialize database");

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

            let app_state = AppState::new(db, snapshot_root);

            // Restore persisted AI config (API keys, models) and tool config (web_search/fetch toggles).
            {
                let db = app_state.db.lock().unwrap();
                if let Ok(Some(json)) = db.get_setting("ai_config") {
                    if let Ok(config) = serde_json::from_str(&json) {
                        app_state.agent.lock().unwrap().ai_config = config;
                    }
                }
                if let Ok(Some(json)) = db.get_setting("tool_config") {
                    if let Ok(config) = serde_json::from_str(&json) {
                        app_state.agent.lock().unwrap().tool_config = config;
                    }
                }
            }

            app.manage(app_state);

            // Create default ~/projects directory so git clone has a sensible home.
            if let Ok(home) = app.path().home_dir() {
                std::fs::create_dir_all(home.join("projects")).ok();
            }

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
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
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
            commands::editor::undo_edit,
            commands::editor::redo_edit,
            commands::editor::close_buffer,
            commands::terminal::create_terminal,
            commands::terminal::write_terminal,
            commands::terminal::resize_terminal,
            commands::terminal::close_terminal,
            commands::terminal::list_terminals,
            commands::terminal::detect_shells,
            commands::search::search_in_project,
            commands::search::search_global,
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
            commands::agent::get_subagent_records,
            commands::agent::delete_task,
            commands::agent::delete_tasks_for_project,
            commands::agent::rename_task,
            commands::agent::set_ai_provider,
            commands::agent::get_ai_config,
            commands::agent::remove_ai_provider,
            commands::agent::get_tool_config,
            commands::agent::set_tool_config,
            commands::agent::fetch_ai_models,
            commands::agent::set_permissions,
            commands::agent::set_task_permissions,
            commands::agent::read_mcp_json,
            commands::agent::save_mcp_json,
            commands::agent::remove_mcp_server,
            commands::agent::list_mcp_servers,
            commands::agent::test_mcp_server,
            commands::agent::abort_task,
            commands::agent::respond_to_permission,
            commands::agent::respond_to_question,
            commands::agent::set_task_sensitive_access,
            commands::agent::get_task_cost,
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
            commands::checkpoint::list_checkpoints,
            commands::checkpoint::revert_to_checkpoint,
            commands::checkpoint::preview_checkpoint,
            commands::checkpoint::get_checkpoint_diff,
            commands::checkpoint::truncate_task_messages,
            commands::settings::get_settings,
            commands::settings::update_settings,
            commands::settings::get_active_theme,
            commands::settings::list_themes,
            commands::settings::import_theme,
            commands::settings::import_keybindings,
            commands::settings::detect_vscode_keybindings,
            commands::lsp::lsp_notify_open,
            commands::lsp::lsp_notify_change,
            commands::lsp::lsp_notify_save,
            commands::lsp::lsp_notify_close,
            commands::lsp::get_completions,
            commands::lsp::get_hover,
            commands::lsp::goto_definition,
            commands::lsp::format_document,
            commands::preview::read_file_base64,
            commands::preview::read_hex_chunk,
            commands::preview::get_file_size,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

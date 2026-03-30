mod commands;
mod state;

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

            let db = rustic_db::Database::new(&db_path)
                .expect("Failed to initialize database");

            let app_state = AppState::new(db);

            // Restore persisted AI config (API keys, models)
            {
                let db = app_state.db.lock().unwrap();
                if let Ok(Some(json)) = db.get_setting("ai_config") {
                    if let Ok(config) = serde_json::from_str(&json) {
                        app_state.agent.lock().unwrap().ai_config = config;
                    }
                }
            }

            app.manage(app_state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::workspace::add_project,
            commands::workspace::remove_project,
            commands::workspace::list_projects,
            commands::file_tree::read_dir,
            commands::file_tree::read_file_content,
            commands::file_tree::create_file,
            commands::file_tree::create_folder,
            commands::file_tree::rename_entry,
            commands::file_tree::delete_entry,
            commands::file_tree::reveal_in_file_manager,
            commands::editor::open_file,
            commands::editor::get_visible_lines,
            commands::editor::highlight_buffer,
            commands::editor::edit_buffer,
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
            commands::git::git_log,
            commands::git::git_commit_files,
            commands::git::git_commit_file_diff,
            commands::git::github_device_code,
            commands::git::github_poll_token,
            commands::git::github_get_user,
            commands::agent::create_task,
            commands::agent::send_message,
            commands::agent::list_tasks,
            commands::agent::get_task_messages,
            commands::agent::delete_task,
            commands::agent::rename_task,
            commands::agent::set_ai_provider,
            commands::agent::get_ai_config,
            commands::agent::fetch_ai_models,
            commands::agent::set_permissions,
            commands::agent::set_task_permissions,
            commands::agent::add_mcp_server,
            commands::agent::remove_mcp_server,
            commands::agent::list_mcp_servers,
            commands::agent::test_mcp_server,
            commands::agent::abort_task,
            commands::agent::respond_to_permission,
            commands::agent::set_task_sensitive_access,
            commands::agent::get_task_cost,
            commands::agent::extend_turn_budget,
            commands::agent::get_memory,
            commands::agent::clear_memory,
            commands::agent::switch_model,
            commands::agent::import_mcp_json,
            commands::skills::list_skills,
            commands::skills::get_skill_body,
            commands::skills::create_skill,
            commands::skills::delete_skill,
            commands::skills::install_skill,
            commands::workflows::list_workflows,
            commands::workflows::get_workflow_body,
            commands::workflows::create_workflow,
            commands::workflows::delete_workflow,
            commands::checkpoint::list_checkpoints,
            commands::checkpoint::revert_to_checkpoint,
            commands::checkpoint::preview_checkpoint,
            commands::checkpoint::get_checkpoint_diff,
            commands::settings::get_settings,
            commands::settings::update_settings,
            commands::settings::get_active_theme,
            commands::settings::list_themes,
            commands::settings::import_theme,
            commands::settings::import_keybindings,
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

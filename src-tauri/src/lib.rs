pub mod fonts;
pub mod fsx;
pub mod git;
pub mod indexer;
pub mod pty;
pub mod sessions;
pub mod usage;
pub mod watcher;
pub mod winstate;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .manage(pty::PtyManager::default())
        .manage(watcher::WatchState::default())
        .invoke_handler(tauri::generate_handler![
            pty::pty_spawn,
            pty::pty_write,
            pty::pty_resize,
            pty::pty_kill,
            sessions::list_sessions,
            sessions::session_status,
            sessions::session_preview,
            sessions::session_delete,
            sessions::trash_list,
            sessions::trash_restore,
            sessions::trash_delete,
            sessions::trash_empty,
            sessions::session_tasks,
            sessions::session_agents,
            sessions::session_artifacts,
            sessions::running_session_ids,
            sessions::bg_agent_session_ids,
            sessions::unstoppable_session_ids,
            sessions::session_migrated_to,
            sessions::ui_log,
            sessions::live_session_ids,
            sessions::stop_session,
            sessions::resolve_resumable_id,
            sessions::session_fork,
            sessions::materialize_fork,
            sessions::claude_permission_mode,
            sessions::claude_model_default,
            sessions::restore_claude_model_default,
            sessions::session_model,
            usage::usage_limits,
            fonts::list_fonts,
            fonts::font_packages,
            fonts::install_font_package,
            fonts::install_font_files,
            watcher::watch_project,
            fsx::list_dir,
            fsx::open_path,
            fsx::list_projects,
            indexer::reindex_sessions,
            indexer::search_sessions,
            git::git_repo_state,
            git::git_status,
            git::git_branches,
            git::git_branch_files,
            git::git_branch_log,
            git::git_log,
            git::git_diff_file,
            git::git_commit_diff,
        ])
        .setup(|app| {
            // Push sessions-list refreshes when Claude's transcripts change
            // (new/cleared/forked sessions) instead of waiting for the 30s poll.
            let _ = watcher::watch_claude_projects(app.handle().clone());

            // Ask for the saved size less whatever this desktop's decorations
            // add to it. Runs after the plugin's own restore, so it wins.
            winstate::correct_restored_size(app.handle());
            // Then measure what actually landed, once the compositor has
            // settled the surface, and remember it for next launch.
            {
                let h = app.handle().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(2500));
                    let inner = h.clone();
                    let _ = h.run_on_main_thread(move || winstate::learn_drift(&inner));
                });
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

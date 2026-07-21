pub mod fsx;
pub mod git;
pub mod indexer;
pub mod pty;
pub mod sessions;
pub mod watcher;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
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
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

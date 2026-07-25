pub mod fsx;
pub mod git;
pub mod indexer;
pub mod pty;
pub mod sessions;
pub mod usage;
pub mod watcher;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // WebKitGTK on Wayland ships a DMABUF renderer path that frequently fails
    // to flush partial frames — the terminal shows stale cells / torn glyphs
    // until something forces a surface reconfigure (the old resize-jiggle
    // crutch). Disabling just that path (compositing/GPU stays on, so the
    // WebGL terminal renderer still works) makes the webview repaint reliably.
    // Must be set before the webview/GTK initializes.
    #[cfg(target_os = "linux")]
    {
        if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
            std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
        }
    }
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
            sessions::running_session_ids,
            sessions::resolve_resumable_id,
            usage::usage_limits,
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
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

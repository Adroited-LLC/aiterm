mod fsx;
mod git;
mod pty;
mod sessions;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(pty::PtyManager::default())
        .invoke_handler(tauri::generate_handler![
            pty::pty_spawn,
            pty::pty_write,
            pty::pty_resize,
            pty::pty_kill,
            sessions::list_sessions,
            fsx::list_dir,
            git::git_repo_state,
            git::git_status,
            git::git_branches,
            git::git_log,
            git::git_diff_file,
            git::git_commit_diff,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

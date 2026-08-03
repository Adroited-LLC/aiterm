pub mod agents;
pub mod cache;
pub mod chat;
pub mod diag;
pub mod fonts;
pub mod fsx;
pub mod git;
pub mod hooklink;
pub mod indexer;
pub mod launch;
pub mod opencode;
pub mod providers;
pub mod rendercost;
pub mod pty;
pub mod sessions;
pub mod taskbar;
pub mod trace;
pub mod tray;
pub mod usage;
pub mod watcher;
pub mod winstate;

/// Run a blocking body on the async runtime's blocking pool. Tauri executes
/// non-async commands on the GTK main thread, where every millisecond is a
/// frame — a command routed through here costs the main loop nothing.
pub async fn run_blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .expect("blocking command panicked")
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    trace::init();
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .manage(pty::PtyManager::default())
        .manage(watcher::WatchState::default())
        // Wrapped so a debug build logs every IPC call before it dispatches.
        // In release `log_invokes` is the identity function and the generated
        // handler is passed straight through — see `trace.rs`.
        .invoke_handler(trace::log_invokes(tauri::generate_handler![
            pty::pty_spawn,
            pty::pty_write,
            pty::pty_resize,
            pty::pty_kill,
            agents::detect_agents,
            agents::agent_caps,
            rendercost::renderer_probe,
            taskbar::taskbar_badge,
            tray::tray_alerts,
            agents::adopt_agent_session,
            diag::diag_log_path,
            diag::diag_log_tail,
            diag::diag_environment,
            agents::agent_choices,
            launch::resolve_launch,
            providers::providers_list,
            providers::provider_save,
            providers::provider_delete,
            providers::provider_models,
            providers::provider_model_cards,
            providers::provider_startup_set,
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
            sessions::session_moved_to,
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
            hooklink::drain_session_events,
            trace::trace_set,
            trace::trace_status,
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
        ]))
        .setup(|app| {
            // First line of every log: which build this is and what launched
            // it. The crash that took an hour to pin down last night was an
            // aiterm started from inside another one's process tree, and the
            // parent pid is the whole of that story.
            crate::diag!(
                "start",
                "aiterm {} pid={} ppid={}",
                env!("CARGO_PKG_VERSION"),
                std::process::id(),
                std::fs::read_to_string("/proc/self/status")
                    .ok()
                    .and_then(|s| s
                        .lines()
                        .find_map(|l| l.strip_prefix("PPid:").map(|v| v.trim().to_string())))
                    .unwrap_or_else(|| "?".into())
            );

            // The settings file claude launches load their SessionStart hook
            // from. Every launch, because it embeds this binary's path.
            hooklink::install();

            // Push sessions-list refreshes when an agent's transcripts change
            // (new/cleared/forked sessions) instead of waiting for the 30s poll.
            if let Err(e) = watcher::watch_claude_projects(app.handle().clone()) {
                crate::diag!("start", "transcript watcher not running: {e}");
            }

            // Ask for the saved size less whatever this desktop's decorations
            // add to it. Runs after the plugin's own restore, so it wins.
            // The tray is where a waiting session can be reached from while
            // aiterm is behind something. Failing to create one is not worth
            // refusing to start over.
            if let Err(e) = tray::init(app.handle()) {
                crate::diag!("tray", "could not create tray icon: {e}");
            }
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

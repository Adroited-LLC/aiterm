use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::path::Path;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, State};

/// One active project watcher; replacing it drops the old one, which
/// disconnects its channel and lets the old pump thread exit.
#[derive(Default)]
pub struct WatchState(pub Mutex<Option<RecommendedWatcher>>);

#[derive(Clone, serde::Serialize)]
struct FsChanged {
    git: bool,
    tree: bool,
}

/// Heavy churn dirs that would flood events and add no signal.
const SKIP: &[&str] = &[
    "/node_modules/",
    "/target/",
    "/.git/objects/",
    "/dist/",
    "/build/",
    "/.venv/",
    "/__pycache__/",
    "/.cache/",
    "/.mypy_cache/",
];

#[tauri::command]
pub fn watch_project(
    app: AppHandle,
    state: State<'_, WatchState>,
    path: String,
) -> Result<(), String> {
    let (tx, rx) = std::sync::mpsc::channel::<notify::Result<notify::Event>>();
    let mut watcher = notify::recommended_watcher(move |res| {
        let _ = tx.send(res);
    })
    .map_err(|e| e.to_string())?;
    watcher
        .watch(Path::new(&path), RecursiveMode::Recursive)
        .map_err(|e| e.to_string())?;
    *state.0.lock().unwrap() = Some(watcher);

    std::thread::spawn(move || loop {
        let first = match rx.recv() {
            Ok(v) => v,
            Err(_) => break, // watcher replaced or app shutting down
        };
        let mut git = false;
        let mut tree = false;
        let mut note = |ev: notify::Result<notify::Event>| {
            if let Ok(ev) = ev {
                for p in &ev.paths {
                    let s = p.to_string_lossy();
                    if SKIP.iter().any(|k| s.contains(k)) {
                        continue;
                    }
                    if s.contains("/.git/") {
                        git = true;
                    } else {
                        tree = true;
                    }
                }
            }
        };
        note(first);
        // Debounce: fold bursts (builds, checkouts) into one event, firing
        // after 400ms of quiet.
        while let Ok(ev) = rx.recv_timeout(std::time::Duration::from_millis(400)) {
            note(ev);
        }
        if git || tree {
            let _ = app.emit("fs://changed", FsChanged { git, tree });
        }
    });
    Ok(())
}

/// Watch `~/.claude/projects` recursively and emit `sessions://changed` after
/// writes settle, so the sessions list refreshes promptly on /clear, forks, or
/// new transcripts instead of waiting for the 30s poll.
///
/// The watcher handle is moved into the pump thread's closure so it lives for
/// the whole app lifetime; when the thread ends (never, in practice) the
/// watcher drops with it. This is the simplest correct way to keep it alive
/// without adding managed state.
pub fn watch_claude_projects(app: AppHandle) -> Result<(), String> {
    let home = dirs::home_dir().ok_or_else(|| "no home dir".to_string())?;
    // Every agent's transcript directory, not just claude's. The sidebar lists
    // all of them, so watching one meant a Codex session simply did not appear
    // until the 30s poll came round or something else forced a refresh — which
    // reads as aiterm having lost it.
    let dirs = [home.join(".claude/projects"), home.join(".codex/sessions")];
    // Nothing to watch yet; not an error (a directory appears once its agent
    // has run at least once, and most machines will not have all of them).
    let present: Vec<_> = dirs.iter().filter(|d| d.exists()).collect();
    if present.is_empty() {
        return Ok(());
    }

    let (tx, rx) = std::sync::mpsc::channel::<notify::Result<notify::Event>>();
    let mut watcher = notify::recommended_watcher(move |res| {
        let _ = tx.send(res);
    })
    .map_err(|e| e.to_string())?;
    for dir in present {
        watcher
            .watch(dir, RecursiveMode::Recursive)
            .map_err(|e| e.to_string())?;
    }

    std::thread::spawn(move || {
        // Keep the watcher alive for the lifetime of this thread.
        let _watcher = watcher;
        loop {
            // Block until the first event of a burst.
            if rx.recv().is_err() {
                break; // channel closed (app shutting down)
            }
            // Debounce: transcript appends arrive in bursts; drain until 1500ms
            // of quiet, then emit once. Avoids firing on every append.
            while rx
                .recv_timeout(std::time::Duration::from_millis(1500))
                .is_ok()
            {}
            let _ = app.emit("sessions://changed", ());
        }
    });
    Ok(())
}

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

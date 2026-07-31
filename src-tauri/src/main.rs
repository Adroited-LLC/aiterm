// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // `aiterm --hook-report` is not the app: it is the one-shot helper a
    // Claude Code SessionStart hook runs to tell the running aiterm which
    // session just started in which process. Dispatch before Tauri exists —
    // it must cost milliseconds and never touch a display.
    if std::env::args().nth(1).as_deref() == Some("--hook-report") {
        aiterm_lib::hooklink::hook_report();
        return;
    }
    aiterm_lib::run()
}

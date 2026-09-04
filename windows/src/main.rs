#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod bridge;
use aiterm_wsl_protocol::{Event, Request};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tauri::{ipc::Channel, Manager, State};

#[derive(Clone, Default)]
struct Terminal(Arc<Mutex<HashMap<String, Arc<Mutex<Option<bridge::Session>>>>>>);

#[tauri::command]
async fn start_terminal(
    id: String,
    cols: u16,
    rows: u16,
    cwd: Option<String>,
    command: Option<String>,
    events: Channel<Event>,
    state: State<'_, Terminal>,
) -> Result<bridge::Workspace, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let slot = Arc::new(Mutex::new(None));
        {
            let mut sessions = state.0.lock().map_err(|e| e.to_string())?;
            if sessions.contains_key(&id) {
                return Err("Terminal already exists".into());
            }
            sessions.insert(id.clone(), slot.clone());
        }
        // Only this terminal waits on startup; existing terminals remain usable.
        let mut owned = slot.lock().map_err(|e| e.to_string())?;
        let result = bridge::start(cols, rows, cwd, command, move |event| {
            events.send(event).is_ok()
        });
        let (new_session, workspace) = match result {
            Ok(value) => value,
            Err(error) => {
                state.0.lock().map_err(|e| e.to_string())?.remove(&id);
                return Err(error);
            }
        };
        *owned = Some(new_session);
        Ok(workspace)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn terminal_request(
    id: String,
    request: Request,
    state: State<'_, Terminal>,
) -> Result<(), String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        if matches!(request, Request::Close) {
            let slot = state.0.lock().map_err(|e| e.to_string())?.remove(&id);
            if let Some(slot) = slot {
                slot.lock().map_err(|e| e.to_string())?.take();
            }
            return Ok(());
        }
        if matches!(request, Request::Start { .. }) {
            return Err("Use start_terminal".into());
        }
        let slot = state
            .0
            .lock()
            .map_err(|e| e.to_string())?
            .get(&id)
            .cloned()
            .ok_or("Terminal is not connected")?;
        let mut session = slot.lock().map_err(|e| e.to_string())?;
        session
            .as_mut()
            .ok_or("Terminal is not connected")?
            .send(&request)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn workspace_rpc(
    command: String,
    args: serde_json::Value,
) -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || bridge::rpc(command, args))
        .await
        .map_err(|e| e.to_string())?
}

fn main() {
    // Exercises the exact Windows -> WSL -> Linux PTY path without requiring a
    // logged-in graphical session. Used by the build validation script.
    if std::env::args().any(|arg| arg == "--smoke-test") {
        let result = smoke_test();
        if let Some(path) = std::env::var_os("AITERM_SMOKE_REPORT") {
            let _ = std::fs::write(
                path,
                match &result {
                    Ok(()) => "PASS\n".into(),
                    Err(e) => format!("FAIL: {e}\n"),
                },
            );
        }
        std::process::exit(if result.is_ok() { 0 } else { 1 });
    }
    tauri::Builder::default()
        .manage(Terminal::default())
        .invoke_handler(tauri::generate_handler![
            start_terminal,
            terminal_request,
            workspace_rpc
        ])
        .on_window_event(|window, event| {
            if matches!(event, tauri::WindowEvent::Destroyed) {
                window.state::<Terminal>().0.lock().unwrap().clear();
            }
        })
        .run(tauri::generate_context!())
        .expect("Could not start aiterm");
}

fn smoke_test() -> Result<(), String> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    let (tx, rx) = std::sync::mpsc::channel();
    let workspace = bridge::rpc("workspace".into(), serde_json::json!({}))?;
    let home = workspace["home"].as_str().ok_or("Workspace home missing")?;
    let entries = bridge::rpc("list_dir".into(), serde_json::json!({"path":home}))?;
    if !entries.is_array() {
        return Err("Directory listing failed".into());
    }
    let (mut session, _) = bridge::start(100, 32, Some(home.into()), None, move |event| {
        tx.send(event).is_ok()
    })?;
    session.send(&Request::Input { data: STANDARD.encode("printf '\\101\\111\\124\\105\\122\\115\\137\\127\\123\\114\\137\\117\\113\\n'; stty size; exit 7\n") })?;
    let mut output = Vec::new();
    loop {
        match rx
            .recv_timeout(std::time::Duration::from_secs(20))
            .map_err(|e| e.to_string())?
        {
            Event::Output { sequence, data } => {
                output.extend(STANDARD.decode(data).map_err(|e| e.to_string())?);
                session.send(&Request::Ack { sequence })?;
            }
            Event::Exit { code, .. } => {
                let output = String::from_utf8_lossy(&output);
                return if code == Some(7)
                    && output.contains("AITERM_WSL_OK")
                    && output.contains("32 100")
                {
                    Ok(())
                } else {
                    Err(format!("Unexpected terminal result: {code:?}: {output}"))
                };
            }
            Event::Error { message } => return Err(message),
            Event::Ready { .. } => {}
        }
    }
}

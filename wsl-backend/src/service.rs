use crate::runtime::AppHandle;
include!(concat!(env!("OUT_DIR"), "/commands.rs"));

pub fn run() -> Result<(), String> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    runtime.block_on(async {
        crate::runtime::install_sink(Box::new(|event| {
            aiterm_wsl_protocol::write_service_frame(&mut std::io::stdout().lock(), &event).map_err(|e| e.to_string())
        }));
        crate::trace::init();
        let pty = crate::pty::PtyManager::default();
        let tabs = std::sync::Arc::new(crate::tabs::TabRegistry::new(pty.clone()));
        let services = crate::services::ApplicationServices::default();
        let app = AppHandle::default()
            .manage(pty).manage(tabs.clone())
            .manage(std::sync::Arc::new(crate::spine::Spine::new()))
            .manage(services.clone())
            .manage(crate::changes::ChangeLedger::default())
            .manage(crate::watcher::WatchState::default())
            .manage(crate::remote::RemoteState::default());
        crate::tabs::start_desktop_registry_bridge(app.clone(), tabs.clone())?;
        crate::hooklink::install();
        crate::hooklink::start_hook_drain(app.clone());
        crate::watcher::watch_claude_projects(app.clone())?;
        crate::changes::start(&app);
        // Remote access retains its existing opt-in and saved startup preference.
        crate::remote::start_on_launch(app.clone(), tabs.clone(), services);
        crate::runtime::send(serde_json::json!({"type":"ready","version":aiterm_wsl_protocol::VERSION}))?;
        let handle = tokio::runtime::Handle::current();
        let input = tokio::task::spawn_blocking(move || {
            let workers = std::sync::Arc::new(tokio::sync::Semaphore::new(64));
            let mut stdin = std::io::stdin().lock();
            while let Some(request) = aiterm_wsl_protocol::read_service_frame::<serde_json::Value>(&mut stdin).map_err(|e| e.to_string())? {
                if request["type"] == "close" { break; }
                if request["type"] == "ack" || request["type"] == "channel_close" {
                    crate::runtime::ipc::control(&request);
                    continue;
                }
                let id = request["id"].as_u64().ok_or("missing request id")?;
                let Ok(permit) = workers.clone().try_acquire_owned() else {
                    crate::runtime::send(serde_json::json!({"type":"reply","id":id,"error":"Workspace is busy; retry this operation."}))?;
                    continue;
                };
                let command = request["command"].as_str().ok_or("missing command")?.to_owned();
                let args = request["args"].clone();
                let app = app.clone();
                let handle = handle.clone();
                tokio::task::spawn_blocking(move || {
                    let _permit = permit;
                    let reply = handle.block_on(dispatch(&command, args, app));
                    let response = match reply { Ok(value) => serde_json::json!({"type":"reply","id":id,"value":value}), Err(error) => serde_json::json!({"type":"reply","id":id,"error":error}) };
                    if let Err(error) = crate::runtime::send(response) {
                        let _ = crate::runtime::send(serde_json::json!({"type":"reply","id":id,"error":error}));
                    }
                });
            }
            Ok::<(),String>(())
        }).await.map_err(|e| e.to_string())?;
        crate::runtime::ipc::close_all();
        for tab in tabs.list() { let _ = tabs.close(tab.id()); }
        input
    })
}

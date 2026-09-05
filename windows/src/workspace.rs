//! One persistent Linux application service, owned by this desktop process.
use aiterm_wsl_protocol::{read_service_frame as read_frame, write_service_frame as write_frame};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    io::BufReader,
    process::{Child, ChildStdin, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc, Arc, Mutex,
    },
    time::Duration,
};
use tauri::{
    ipc::{Channel, InvokeResponseBody, JavaScriptChannelId},
    Emitter, Manager,
};

type Reply = Result<Value, Value>;
#[derive(Default)]
pub struct Workspace(pub Mutex<Option<Arc<Service>>>);
pub struct Service {
    child: Mutex<Child>,
    input: Arc<Mutex<Option<ChildStdin>>>,
    pending: Arc<Mutex<HashMap<u64, mpsc::SyncSender<Reply>>>>,
    closed: Arc<AtomicBool>,
    next: AtomicU64,
}
impl Service {
    fn start(app: tauri::AppHandle) -> Result<Self, String> {
        let (distribution, digest) = crate::bridge::prepare()?;
        app.asset_protocol_scope()
            .allow_directory(format!("\\\\wsl.localhost\\{distribution}\\"), true)
            .map_err(|e| e.to_string())?;
        let launch = format!("exec \"$HOME/.local/share/aiterm/backends/{digest}\" --service");
        let mut child = crate::bridge::wsl()
            .args(["-d", &distribution, "--exec", "sh", "-lc", &launch])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| e.to_string())?;
        let output = child.stdout.take().unwrap();
        let input = Arc::new(Mutex::new(child.stdin.take()));
        let pending = Arc::new(Mutex::new(HashMap::<u64, mpsc::SyncSender<Reply>>::new()));
        let closed = Arc::new(AtomicBool::new(false));
        let service = Self {
            child: Mutex::new(child),
            input: input.clone(),
            pending: pending.clone(),
            closed: closed.clone(),
            next: AtomicU64::new(1),
        };
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let mut reader = BufReader::new(output);
            let mut channels: HashMap<u32, Channel<InvokeResponseBody>> = HashMap::new();
            while let Ok(Some(frame)) = read_frame::<Value>(&mut reader) {
                match frame["type"].as_str() {
                    Some("ready") => {
                        let _ = ready_tx.try_send(frame["version"] == aiterm_wsl_protocol::VERSION);
                    }
                    Some("reply") => {
                        if let Some(tx) = frame["id"]
                            .as_u64()
                            .and_then(|id| pending.lock().unwrap().remove(&id))
                        {
                            let _ = tx.send(if let Some(e) = frame.get("error") {
                                Err(e.clone())
                            } else {
                                Ok(frame["value"].clone())
                            });
                        }
                    }
                    Some("event") => {
                        if let Some(name) = frame["name"].as_str() {
                            let _ = app.emit(name, frame["payload"].clone());
                        }
                    }
                    Some("channel_end") => {
                        if let Some(id) = frame["id"].as_u64() {
                            channels.remove(&(id as u32));
                        }
                    }
                    Some("channel") => {
                        if let (Some(id), Some(data)) =
                            (frame["id"].as_u64(), frame["data"].as_str())
                        {
                            let id = id as u32;
                            if let std::collections::hash_map::Entry::Vacant(entry) =
                                channels.entry(id)
                            {
                                if let (Ok(js), Some(webview)) = (
                                    format!("__CHANNEL__:{id}").parse::<JavaScriptChannelId>(),
                                    app.get_webview_window("main"),
                                ) {
                                    entry.insert(js.channel_on(webview.as_ref().clone()));
                                }
                            }
                            if let (Some(channel), Ok(bytes)) =
                                (channels.get(&id), STANDARD.decode(data))
                            {
                                if channel.send(InvokeResponseBody::Raw(bytes)).is_err() {
                                    channels.remove(&id);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            closed.store(true, Ordering::Release);
            for (_, tx) in pending.lock().unwrap().drain() {
                let _ = tx.send(Err(json!(
                    "The WSL connection ended. Restart aiterm to reconnect."
                )));
            }
            let _ = app.emit("workspace://disconnected", ());
        });
        if ready_rx.recv_timeout(Duration::from_secs(60)) != Ok(true) {
            return Err(
                "The Linux workspace did not become ready. Open your WSL distribution and retry."
                    .into(),
            );
        }
        Ok(service)
    }
    pub fn call(&self, command: String, args: Value) -> Reply {
        if self.closed.load(Ordering::Acquire) {
            return Err(json!(
                "The WSL connection ended. Restart aiterm to reconnect."
            ));
        }
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::sync_channel(1);
        self.pending.lock().unwrap().insert(id, tx);
        let sent = self.send(json!({"type":"call","id":id,"command":command,"args":args}));
        if let Err(error) = sent {
            self.pending.lock().unwrap().remove(&id);
            return Err(json!(error));
        }
        let reply = rx
            .recv_timeout(Duration::from_secs(300))
            .map_err(|_| json!("The Linux operation timed out."));
        self.pending.lock().unwrap().remove(&id);
        reply?
    }
    pub fn send(&self, frame: Value) -> Result<(), String> {
        write_frame(
            self.input
                .lock()
                .unwrap()
                .as_mut()
                .ok_or("Workspace closed")?,
            &frame,
        )
        .map_err(|e| e.to_string())
    }
}
impl Drop for Service {
    fn drop(&mut self) {
        self.input.lock().unwrap().take();
        let child = self.child.get_mut().unwrap();
        for _ in 0..100 {
            if matches!(child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = child.kill();
        let _ = child.wait();
    }
}
impl Workspace {
    pub fn get(&self, app: tauri::AppHandle) -> Result<Arc<Service>, String> {
        let mut slot = self.0.lock().unwrap();
        if slot.is_none() {
            *slot = Some(Arc::new(Service::start(app)?));
        }
        Ok(slot.as_ref().unwrap().clone())
    }
}

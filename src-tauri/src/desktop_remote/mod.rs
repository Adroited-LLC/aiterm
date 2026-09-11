//! Desktop client of the existing device gateway. Network, keys, recovery and
//! terminal state stay in Rust; the renderer receives a display-only projection.
mod store;
mod transport;
use crate::{
    remote::{
        terminal::{
            TransferAssembler, TransferChunk, TransferKind, TransferPayload, TransferStatus,
        },
        PairingUri,
    },
    tabs::{AttachmentId, TabId},
    terminal::model::ScreenSnapshot,
};
use futures_util::{SinkExt, StreamExt};
use p256::ecdsa::{signature::Signer, Signature, SigningKey};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::{Duration, Instant},
};
use store::{Desktop, DesktopView};
use tokio::sync::{mpsc, oneshot, Notify};
use tokio_tungstenite::tungstenite::Message;

#[derive(Clone, Serialize, Default)]
pub struct ClientView {
    pub revision: u64,
    pub connection: String,
    pub desktop_id: Option<String>,
    pub error: Option<String>,
    pub tabs: Vec<Value>,
    pub sessions: Vec<Value>,
    pub selected_tab: Option<String>,
    pub has_focus: bool,
    pub screen: Option<ScreenSnapshot>,
    pub scrollback: Vec<crate::terminal::model::ScreenRow>,
    #[serde(skip)]
    epoch: u64,
}
struct Shared {
    view: Mutex<ClientView>,
    changed: Notify,
    pairing_cancel: Notify,
    generation: AtomicU64,
    task: tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
    sender: Mutex<Option<mpsc::Sender<Job>>>,
    store_lock: tokio::sync::Mutex<()>,
}
impl Shared {
    fn publish(&self, generation: u64, update: impl FnOnce(&mut ClientView)) {
        let mut view = self.view.lock().unwrap();
        if self.generation.load(Ordering::SeqCst) != generation {
            return;
        }
        update(&mut view);
        view.revision = view.revision.wrapping_add(1);
        drop(view);
        self.changed.notify_waiters();
    }
}
fn shared() -> Arc<Shared> {
    static INSTANCE: OnceLock<Arc<Shared>> = OnceLock::new();
    INSTANCE
        .get_or_init(|| {
            Arc::new(Shared {
                view: Mutex::new(ClientView {
                    connection: "disconnected".into(),
                    ..Default::default()
                }),
                changed: Notify::new(),
                pairing_cancel: Notify::new(),
                generation: AtomicU64::new(0),
                task: tokio::sync::Mutex::new(None),
                sender: Mutex::new(None),
                store_lock: tokio::sync::Mutex::new(()),
            })
        })
        .clone()
}

#[cfg_attr(not(aiterm_headless), tauri::command)]
pub async fn desktop_client_list() -> Result<Vec<DesktopView>, String> {
    Ok(store::load()?.iter().map(Desktop::view).collect())
}
#[cfg_attr(not(aiterm_headless), tauri::command)]
pub async fn desktop_client_watch(after: Option<u64>) -> ClientView {
    let shared = shared();
    let changed = shared.changed.notified();
    tokio::pin!(changed);
    changed.as_mut().enable();
    if after == Some(shared.view.lock().unwrap().revision) {
        let _ = tokio::time::timeout(Duration::from_secs(20), changed).await;
    }
    let snapshot = shared.view.lock().unwrap().clone();
    snapshot
}
#[cfg_attr(not(aiterm_headless), tauri::command)]
pub async fn desktop_client_pair(path: String, name: String) -> Result<DesktopView, String> {
    let shared = shared();
    let cancelled = shared.pairing_cancel.notified();
    tokio::pin!(cancelled);
    cancelled.as_mut().enable();
    tokio::select! {
        // Keep the full operation inside the Windows workspace RPC deadline.
        result = tokio::time::timeout(Duration::from_secs(270), pair(path, name)) =>
            result.map_err(|_| "Pairing timed out. Open a new pairing file".to_string())?,
        _ = cancelled => Err("Pairing cancelled".into()),
    }
}
#[cfg_attr(not(aiterm_headless), tauri::command)]
pub async fn desktop_client_cancel_pairing() {
    shared().pairing_cancel.notify_waiters();
}
async fn pair(path: String, name: String) -> Result<DesktopView, String> {
    if name.trim().is_empty() || name.chars().count() > 64 {
        return Err("Enter a device name of 1–64 characters".into());
    }
    let shared = shared();
    let _guard = shared
        .store_lock
        .try_lock()
        .map_err(|_| "Another desktop change is in progress")?;
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    use std::io::Read;
    let mut bytes = Vec::new();
    file.take(32769)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 32768 {
        return Err("Pairing file is too large".into());
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| "Invalid pairing file")?;
    let invite = PairingUri::parse(text.trim()).ok_or("This is not an AiTerm pairing file")?;
    let mut desktop = Desktop::from_invite(&invite)?;
    enroll(&mut desktop, &invite, &name).await?;
    let mut desktops = store::load()?;
    // Re-pairing the same server updates its identity without duplicate menu entries.
    if let Some(old) = desktops
        .iter()
        .find(|d| d.fingerprint == desktop.fingerprint)
    {
        desktop.id = old.id.clone();
    }
    desktops.retain(|d| d.id != desktop.id);
    desktops.push(desktop.clone());
    store::save(&desktops)?;
    Ok(desktop.view())
}
async fn enroll(desktop: &mut Desktop, invite: &PairingUri, name: &str) -> Result<(), String> {
    let mut socket = transport::open(desktop).await?;
    tokio::time::timeout(Duration::from_secs(10), transport::challenge(&mut socket))
        .await
        .map_err(|_| "Pairing handshake timed out")??;
    let key = SigningKey::from_slice(&desktop.key).map_err(|_| "Invalid device key")?;
    #[derive(Serialize)]
    struct Pair {
        kind: &'static str,
        device_name: String,
        #[serde(with = "serde_bytes")]
        enrollment_secret: Vec<u8>,
        #[serde(with = "serde_bytes")]
        public_key: Vec<u8>,
        #[serde(with = "serde_bytes")]
        relay_authority_public_key: Vec<u8>,
        #[serde(with = "serde_bytes")]
        relay_signature_der: Vec<u8>,
    }
    let (authority, signature) = if let Some(digest) = &invite.relay_authorization_digest {
        let authority = store::relay_key()?;
        let signature: Signature = authority.sign(digest);
        (
            authority
                .verifying_key()
                .to_encoded_point(true)
                .as_bytes()
                .to_vec(),
            signature.to_der().as_bytes().to_vec(),
        )
    } else {
        (vec![], vec![])
    };
    transport::send(
        &mut socket,
        &Pair {
            kind: "pair.request",
            device_name: name.trim().into(),
            enrollment_secret: invite.secret.clone(),
            public_key: key
                .verifying_key()
                .to_encoded_point(true)
                .as_bytes()
                .to_vec(),
            relay_authority_public_key: authority,
            relay_signature_der: signature,
        },
    )
    .await?;
    let approved = tokio::time::timeout(Duration::from_secs(305), async {
        loop {
            let reply: transport::Hello =
                transport::decode(&transport::receive(&mut socket).await?)?;
            match reply.kind.as_str() {
                "pair.pending" => {}
                "pair.approved" if !reply.device_id.is_empty() => return Ok(reply.device_id),
                "pair.denied" => {
                    return Err("Pairing was declined on the other desktop".to_string())
                }
                "pair.expired" => return Err("Pairing expired. Export a new pairing file".into()),
                _ => return Err("Unexpected pairing response".into()),
            }
        }
    })
    .await
    .map_err(|_| "Pairing expired. Export a new pairing file")??;
    desktop.device_id = approved;
    let _ = socket.close(None).await;
    Ok(())
}
#[cfg_attr(not(aiterm_headless), tauri::command)]
pub async fn desktop_client_forget(id: String) -> Result<(), String> {
    let shared = shared();
    let _guard = shared.store_lock.lock().await;
    let connected = shared.view.lock().unwrap().desktop_id.as_deref() == Some(&id);
    if connected {
        desktop_client_disconnect().await;
    }
    let mut list = store::load()?;
    list.retain(|d| d.id != id);
    store::save(&list)
}
#[cfg_attr(not(aiterm_headless), tauri::command)]
pub async fn desktop_client_disconnect() {
    let shared = shared();
    let mut task = shared.task.lock().await;
    let generation = shared.generation.fetch_add(1, Ordering::SeqCst) + 1;
    if let Some(old) = task.take() {
        old.abort();
        let _ = old.await;
    }
    *shared.sender.lock().unwrap() = None;
    shared.publish(generation, |v| {
        let revision = v.revision;
        *v = ClientView {
            revision,
            connection: "disconnected".into(),
            ..Default::default()
        };
    });
}
#[cfg_attr(not(aiterm_headless), tauri::command)]
pub async fn desktop_client_connect(id: String) -> Result<(), String> {
    let desktop = store::load()?
        .into_iter()
        .find(|d| d.id == id)
        .ok_or("Saved desktop not found")?;
    let shared = shared();
    let mut task = shared.task.lock().await;
    if let Some(old) = task.take() {
        old.abort();
        let _ = old.await;
    }
    let generation = shared.generation.fetch_add(1, Ordering::SeqCst) + 1;
    let (tx, rx) = mpsc::channel(32);
    *shared.sender.lock().unwrap() = Some(tx);
    shared.publish(generation, |v| {
        let revision = v.revision;
        *v = ClientView {
            revision,
            connection: "connecting".into(),
            desktop_id: Some(id),
            ..Default::default()
        };
    });
    *task = Some(tokio::spawn(connection_loop(
        shared.clone(),
        generation,
        desktop,
        rx,
    )));
    Ok(())
}
#[derive(Clone)]
enum Action {
    Attach(String),
    Focus,
    Input(String),
    Resize(u16, u16),
    Scrollback(usize),
}
struct Job {
    generation: u64,
    target: Option<String>,
    epoch: u64,
    action: Action,
    reply: oneshot::Sender<Result<(), String>>,
}
async fn dispatch(action: Action) -> Result<(), String> {
    let shared = shared();
    let (epoch, generation, target) = {
        let view = shared.view.lock().unwrap();
        if view.connection != "connected" {
            return Err("Desktop is not connected".into());
        }
        (
            view.epoch,
            shared.generation.load(Ordering::SeqCst),
            view.selected_tab.clone(),
        )
    };
    let tx = shared
        .sender
        .lock()
        .unwrap()
        .clone()
        .ok_or("Desktop is not connected")?;
    let (reply, result) = oneshot::channel();
    tx.try_send(Job {
        generation,
        target,
        epoch,
        action,
        reply,
    })
    .map_err(|_| "Desktop input queue is full")?;
    tokio::time::timeout(Duration::from_secs(15), result)
        .await
        .map_err(|_| "Desktop request timed out")?
        .map_err(|_| "Desktop connection changed")?
}
#[cfg_attr(not(aiterm_headless), tauri::command)]
pub async fn desktop_client_attach(tab_id: String) -> Result<(), String> {
    dispatch(Action::Attach(tab_id)).await
}
#[cfg_attr(not(aiterm_headless), tauri::command)]
pub async fn desktop_client_focus() -> Result<(), String> {
    dispatch(Action::Focus).await
}
#[cfg_attr(not(aiterm_headless), tauri::command)]
pub async fn desktop_client_input(data: String) -> Result<(), String> {
    if data.is_empty() || data.len() > 65536 {
        return Err("Terminal input is empty or too large".into());
    }
    dispatch(Action::Input(data)).await
}
#[cfg_attr(not(aiterm_headless), tauri::command)]
pub async fn desktop_client_resize(cols: u16, rows: u16) -> Result<(), String> {
    crate::remote::model::TerminalSize::try_new(cols, rows).map_err(|e| e.to_string())?;
    dispatch(Action::Resize(cols, rows)).await
}
#[cfg_attr(not(aiterm_headless), tauri::command)]
pub async fn desktop_client_scrollback(offset: usize) -> Result<(), String> {
    dispatch(Action::Scrollback(offset)).await
}

#[derive(Serialize, Deserialize)]
struct Frame {
    version: u16,
    request_id: u64,
    kind: String,
    #[serde(with = "serde_bytes")]
    payload: Vec<u8>,
}
struct Pending {
    kind: String,
    started: Instant,
    job: Option<Job>,
}
struct Connection {
    socket: transport::Socket,
    next_id: u64,
    pending: HashMap<u64, Pending>,
    attachment: Option<(String, String)>,
    assembler: Option<TransferAssembler>,
    shared: Arc<Shared>,
    generation: u64,
    epoch: u64,
}
impl Connection {
    async fn request(
        &mut self,
        kind: &str,
        payload: Vec<u8>,
        job: Option<Job>,
    ) -> Result<(), String> {
        self.next_id += 1;
        let id = self.next_id;
        self.pending.insert(
            id,
            Pending {
                kind: kind.into(),
                started: Instant::now(),
                job,
            },
        );
        transport::send(
            &mut self.socket,
            &Frame {
                version: 1,
                request_id: id,
                kind: kind.into(),
                payload,
            },
        )
        .await
    }
    async fn refresh(&mut self) -> Result<(), String> {
        for kind in ["tab.list", "session.roster"] {
            if !self.pending.values().any(|p| p.kind == kind) {
                self.request(kind, vec![], None).await?;
            }
        }
        Ok(())
    }
    async fn attach(&mut self, tab: String, job: Option<Job>) -> Result<(), String> {
        if let Some((old, id)) = self.attachment.take() {
            self.request(
                "terminal.detach",
                transport::encode(&json!({"tab_id":old,"attachment_id":id}))?,
                None,
            )
            .await?;
        }
        self.assembler = None;
        self.shared.publish(self.generation, |v| {
            v.selected_tab = Some(tab.clone());
            v.screen = None;
            v.scrollback.clear();
            v.has_focus = false;
        });
        self.request(
            "terminal.attach",
            transport::encode(&json!({"tab_id":tab}))?,
            job,
        )
        .await
    }
    async fn job(&mut self, job: Job) -> Result<(), String> {
        if job.reply.is_closed() {
            return Ok(());
        }
        if job.epoch != self.epoch || job.generation != self.generation {
            let _ = job
                .reply
                .send(Err("Connection changed; input was not replayed".into()));
            return Ok(());
        }
        if let Action::Attach(tab) = &job.action {
            return self.attach(tab.clone(), Some(job)).await;
        }
        if job.target != self.shared.view.lock().unwrap().selected_tab {
            let _ = job.reply.send(Err(
                "Selected session changed; input was not replayed".into()
            ));
            return Ok(());
        }
        let Some((tab, id)) = self.attachment.clone() else {
            let _ = job.reply.send(Err("Select a live session first".into()));
            return Ok(());
        };
        let focused = self.shared.view.lock().unwrap().has_focus;
        if matches!(job.action, Action::Input(_) | Action::Resize(..)) && !focused {
            let _ = job
                .reply
                .send(Err("Take control of this terminal before typing".into()));
            return Ok(());
        }
        let (kind, payload) = match &job.action {
            Action::Focus => {
                let size = self
                    .shared
                    .view
                    .lock()
                    .unwrap()
                    .screen
                    .as_ref()
                    .map(|s| (s.cols(), s.rows()))
                    .unwrap_or((100, 30));
                (
                    "terminal.focus",
                    transport::encode(
                        &json!({"tab_id":tab,"attachment_id":id,"size":{"cols":size.0,"rows":size.1}}),
                    )?,
                )
            }
            Action::Input(data) => {
                #[derive(Serialize)]
                struct Input {
                    tab_id: String,
                    attachment_id: String,
                    #[serde(with = "serde_bytes")]
                    data: Vec<u8>,
                }
                (
                    "terminal.input",
                    transport::encode(&Input {
                        tab_id: tab,
                        attachment_id: id,
                        data: data.as_bytes().to_vec(),
                    })?,
                )
            }
            Action::Resize(cols, rows) => (
                "terminal.resize",
                transport::encode(
                    &json!({"tab_id":tab,"attachment_id":id,"size":{"cols":cols,"rows":rows}}),
                )?,
            ),
            Action::Scrollback(offset) => (
                "terminal.scrollback",
                transport::encode(
                    &json!({"tab_id":tab,"attachment_id":id,"offset":offset,"count":100}),
                )?,
            ),
            Action::Attach(_) => unreachable!(),
        };
        self.request(kind, payload, Some(job)).await
    }
    fn frame(&mut self, bytes: &[u8]) -> Result<(), String> {
        let frame: Frame = transport::decode(bytes)?;
        if frame.version != 1 {
            return Err("Unsupported remote protocol".into());
        }
        if frame.kind.starts_with("terminal.")
            && matches!(
                frame.kind.as_str(),
                "terminal.snapshot" | "terminal.diff" | "terminal.scrollback"
            )
        {
            let chunk: TransferChunk = transport::decode(&frame.payload)?;
            let expected = match chunk.kind {
                TransferKind::Snapshot => "terminal.snapshot",
                TransferKind::Diff => "terminal.diff",
                TransferKind::Scrollback => "terminal.scrollback",
            };
            if chunk.request_id != frame.request_id || expected != frame.kind {
                return Err("Mismatched terminal transfer envelope".into());
            }
            if self.attachment.as_ref().is_none_or(|(tab, id)| {
                chunk.tab_id.as_str() != tab
                    || chunk.attachment_id.as_ref().map(AttachmentId::as_str) != Some(id.as_str())
            }) {
                return Ok(());
            }
            let assembler = self
                .assembler
                .as_mut()
                .ok_or("Missing terminal transfer state")?;
            match assembler.accept("desktop-client", chunk) {
                TransferStatus::Pending => {}
                TransferStatus::Recover => {
                    return Err("Terminal stream needs a fresh snapshot".into())
                }
                TransferStatus::Complete(complete) => {
                    let token = complete.token();
                    let mut applied = true;
                    self.shared
                        .publish(self.generation, |v| match complete.into_payload() {
                            TransferPayload::Snapshot(screen) => v.screen = Some(screen),
                            TransferPayload::Diff(diff) => {
                                applied = v.screen.as_mut().is_some_and(|s| s.apply(diff).is_ok());
                            }
                            TransferPayload::Scrollback(rows) => {
                                v.scrollback = rows;
                            }
                        });
                    if !applied {
                        let _ = assembler.reject_applied(token);
                        return Err("Terminal revision changed".into());
                    }
                    assembler.commit_applied(token).map_err(|e| e.to_string())?;
                    if frame.kind == "terminal.scrollback" {
                        if let Some(pending) = self.pending.remove(&frame.request_id) {
                            if pending.kind != frame.kind {
                                return Err("Mismatched scrollback response".into());
                            }
                            if let Some(job) = pending.job {
                                let _ = job.reply.send(Ok(()));
                            }
                        }
                    }
                }
            }
            return Ok(());
        }
        if frame.request_id == 0
            && !matches!(
                frame.kind.as_str(),
                "terminal.focus_changed"
                    | "terminal.exited"
                    | "terminal.attachment_closed"
                    | "auth.revoked"
                    | "auth.denied"
            )
        {
            return Ok(());
        }
        let payload: Value = if frame.payload.is_empty() {
            Value::Null
        } else {
            transport::decode(&frame.payload)?
        };
        if frame.kind == "auth.revoked" || frame.kind == "auth.denied" {
            return Err("Device access denied. Pair with this desktop again.".into());
        }
        let pending = self.pending.remove(&frame.request_id);
        if frame.kind == "error" {
            let message = payload["message"]
                .as_str()
                .unwrap_or("Remote request failed")
                .to_string();
            if let Some(Pending { job: Some(job), .. }) = pending {
                let _ = job.reply.send(Err(message));
            }
            return Ok(());
        }
        if let Some(ref pending) = pending {
            if pending.kind != frame.kind {
                return Err("Mismatched remote response".into());
            }
        }
        match frame.kind.as_str() {
            "tab.list" => self.shared.publish(self.generation, |v| {
                v.tabs = payload["tabs"].as_array().cloned().unwrap_or_default()
            }),
            "session.roster" => self.shared.publish(self.generation, |v| {
                v.sessions = payload["sessions"].as_array().cloned().unwrap_or_default()
            }),
            "terminal.attach" => {
                let tab = payload["tab_id"]
                    .as_str()
                    .ok_or("Invalid terminal attachment")?
                    .to_string();
                let id = payload["attachment_id"]
                    .as_str()
                    .ok_or("Invalid terminal attachment")?
                    .to_string();
                if self.shared.view.lock().unwrap().selected_tab.as_deref() == Some(&tab) {
                    self.assembler = Some(
                        TransferAssembler::new("desktop-client", TabId::from_raw(&tab))
                            .bind_attachment(
                                serde_json::from_value(Value::String(id.clone()))
                                    .map_err(|_| "Invalid attachment identity")?,
                            ),
                    );
                    self.attachment = Some((tab, id));
                    self.shared.publish(self.generation, |v| {
                        v.has_focus = payload["has_focus"].as_bool().unwrap_or(false)
                    });
                }
            }
            "terminal.focus_changed" => {
                if payload.get("focus").is_some() {
                    self.update_focus(&payload);
                }
            }
            "terminal.exited" | "terminal.attachment_closed" => {
                if self.matches_attachment(&payload) {
                    self.shared.publish(self.generation, |v| {
                        v.has_focus = false;
                        v.error = Some("This session has ended".into());
                    });
                }
            }
            _ => {}
        }
        if let Some(Pending { job: Some(job), .. }) = pending {
            let _ = job.reply.send(Ok(()));
        }
        Ok(())
    }
    fn matches_attachment(&self, value: &Value) -> bool {
        self.attachment.as_ref().is_some_and(|(t, a)| {
            value["tab_id"].as_str() == Some(t.as_str())
                && value["attachment_id"].as_str() == Some(a.as_str())
        })
    }
    fn update_focus(&self, value: &Value) {
        if self.matches_attachment(value) {
            self.shared
                .publish(self.generation, |v| v.has_focus = value["focus"] == "self");
        }
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        for (_, pending) in self.pending.drain() {
            if let Some(job) = pending.job {
                let _ = job
                    .reply
                    .send(Err("Connection changed; input was not replayed".into()));
            }
        }
    }
}
async fn connection_loop(
    shared: Arc<Shared>,
    generation: u64,
    desktop: Desktop,
    mut receiver: mpsc::Receiver<Job>,
) {
    let mut attempt = 0u64;
    loop {
        attempt += 1;
        let epoch = attempt;
        shared.publish(generation, |v| {
            v.connection = if attempt == 1 {
                "connecting"
            } else {
                "reconnecting"
            }
            .into();
            v.has_focus = false;
            v.screen = None;
            v.epoch = epoch;
        });
        let result: Result<(), String> = async {
            let mut socket = transport::open(&desktop).await?;
            transport::authenticate(&mut socket, &desktop).await?;
            let selected = shared.view.lock().unwrap().selected_tab.clone();
            shared.publish(generation, |v| { v.connection = "connected".into(); v.error = None; });
            let mut connection = Connection {
                socket, next_id: 0, pending: HashMap::new(), attachment: None,
                assembler: None, shared: shared.clone(), generation, epoch,
            };
            connection.refresh().await?;
            if let Some(tab) = selected { connection.attach(tab, None).await?; }
            let mut refresh = tokio::time::interval(Duration::from_secs(4));
            let mut heartbeat = tokio::time::interval(Duration::from_secs(10));
            let mut last_frame = Instant::now();
            loop {
                tokio::select! {
                    message = connection.socket.next() => {
                        let message = message.ok_or("Desktop disconnected")?.map_err(|e| e.to_string())?;
                        last_frame = Instant::now();
                        match message {
                            Message::Binary(bytes) => connection.frame(&bytes)?,
                            Message::Ping(_) => {
                                tokio::time::timeout(Duration::from_secs(10), connection.socket.flush())
                                    .await.map_err(|_| "Desktop write timed out")?.map_err(|e| e.to_string())?;
                            }
                            Message::Pong(_) => {},
                            Message::Close(_) => return Err("Desktop disconnected".into()),
                            _ => return Err("Unexpected remote frame".into()),
                        }
                    },
                    job = receiver.recv() => match job {
                        Some(job) => connection.job(job).await?,
                        None => return Ok(()),
                    },
                    _ = refresh.tick() => connection.refresh().await?,
                    _ = heartbeat.tick() => {
                        if last_frame.elapsed() > Duration::from_secs(35)
                            || connection.pending.values().any(|p| p.started.elapsed() > Duration::from_secs(15)) {
                            return Err("Desktop stopped responding".into());
                        }
                        if connection.assembler.as_mut().is_some_and(|a| a.expire_at(Instant::now())) {
                            return Err("Terminal transfer timed out".into());
                        }
                        tokio::time::timeout(Duration::from_secs(10), connection.socket.send(Message::Ping(vec![].into())))
                            .await.map_err(|_| "Desktop stopped accepting data")?.map_err(|e| e.to_string())?;
                    },
                }
            }
        }.await;
        match result {
            Ok(()) => return,
            Err(error) => {
                let denied =
                    error.contains("access denied") || error.contains("fingerprint changed");
                shared.publish(generation, |v| {
                    v.connection = if denied {
                        "disconnected"
                    } else {
                        "reconnecting"
                    }
                    .into();
                    v.error = Some(error);
                    v.has_focus = false;
                    v.screen = None;
                });
                while let Ok(job) = receiver.try_recv() {
                    let _ = job
                        .reply
                        .send(Err("Desktop disconnected; input was not replayed".into()));
                }
                if denied {
                    return;
                }
                tokio::time::sleep(Duration::from_secs(attempt.min(5))).await;
            }
        }
    }
}

#[cfg(test)]
mod tests;

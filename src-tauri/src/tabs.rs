use crate::pty::{PtyManager, PtySink, PtySpawnSpec};
use crate::remote::model::TerminalSize;
use crate::terminal::model::{ScreenDiff, ScreenRow, ScreenSnapshot};
use crate::terminal::screen::ScreenModel;
use portable_pty::PtySize;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::sync::mpsc::{self, Receiver, RecvError, RecvTimeoutError, SyncSender, TryRecvError};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use uuid::Uuid;

const DEFAULT_QUEUE_CAPACITY: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TabId(String);

impl TabId {
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for TabId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for TabId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AttachmentId(String);

impl AttachmentId {
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for AttachmentId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for AttachmentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AttachmentKind {
    Desktop,
    Remote,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TabLaunch {
    title: String,
    cwd: Option<String>,
    command: Option<String>,
    session_id: Option<String>,
    resumed_id: Option<String>,
    agent_id: Option<String>,
    slot_id: String,
    fresh: bool,
    env_provider: Option<String>,
    env_model: Option<String>,
    size: TerminalSize,
}

impl TabLaunch {
    pub fn new(title: impl Into<String>, slot_id: impl Into<String>, size: TerminalSize) -> Self {
        Self {
            title: title.into(),
            cwd: None,
            command: None,
            session_id: None,
            resumed_id: None,
            agent_id: None,
            slot_id: slot_id.into(),
            fresh: false,
            env_provider: None,
            env_model: None,
            size,
        }
    }

    pub fn with_cwd(mut self, cwd: impl Into<String>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    pub fn with_command(mut self, command: impl Into<String>) -> Self {
        self.command = Some(command.into());
        self
    }

    pub fn with_session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    pub fn with_resumed_id(mut self, resumed_id: impl Into<String>) -> Self {
        self.resumed_id = Some(resumed_id.into());
        self
    }

    pub fn with_agent_id(mut self, agent_id: impl Into<String>) -> Self {
        self.agent_id = Some(agent_id.into());
        self
    }

    pub fn with_fresh(mut self, fresh: bool) -> Self {
        self.fresh = fresh;
        self
    }

    pub fn with_environment(
        mut self,
        provider: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        self.env_provider = Some(provider.into());
        self.env_model = Some(model.into());
        self
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TabUpdate {
    title: Option<String>,
    session_id: Option<String>,
    resumed_id: Option<String>,
    agent_id: Option<String>,
    slot_id: Option<String>,
    fresh: Option<bool>,
}

impl TabUpdate {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    pub fn resumed_id(mut self, resumed_id: impl Into<String>) -> Self {
        self.resumed_id = Some(resumed_id.into());
        self
    }

    pub fn agent_id(mut self, agent_id: impl Into<String>) -> Self {
        self.agent_id = Some(agent_id.into());
        self
    }

    pub fn slot_id(mut self, slot_id: impl Into<String>) -> Self {
        self.slot_id = Some(slot_id.into());
        self
    }

    pub fn fresh(mut self, fresh: bool) -> Self {
        self.fresh = Some(fresh);
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TabState {
    Running,
    Exited,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TabExit {
    code: Option<u32>,
    signal: Option<String>,
    requested: bool,
}

impl TabExit {
    pub fn code(&self) -> Option<u32> {
        self.code
    }

    pub fn signal(&self) -> Option<&str> {
        self.signal.as_deref()
    }

    pub fn requested(&self) -> bool {
        self.requested
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TabDescriptor {
    id: TabId,
    title: String,
    cwd: Option<String>,
    command: Option<String>,
    session_id: Option<String>,
    resumed_id: Option<String>,
    agent_id: Option<String>,
    slot_id: String,
    fresh: bool,
    env_provider: Option<String>,
    env_model: Option<String>,
    size: TerminalSize,
    input_owner: Option<AttachmentId>,
    state: TabState,
    exit: Option<TabExit>,
}

impl TabDescriptor {
    pub fn id(&self) -> &TabId {
        &self.id
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn cwd(&self) -> Option<&str> {
        self.cwd.as_deref()
    }

    pub fn command(&self) -> Option<&str> {
        self.command.as_deref()
    }

    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    pub fn resumed_id(&self) -> Option<&str> {
        self.resumed_id.as_deref()
    }

    pub fn agent_id(&self) -> Option<&str> {
        self.agent_id.as_deref()
    }

    pub fn slot_id(&self) -> &str {
        &self.slot_id
    }

    pub fn fresh(&self) -> bool {
        self.fresh
    }

    pub fn env_provider(&self) -> Option<&str> {
        self.env_provider.as_deref()
    }

    pub fn env_model(&self) -> Option<&str> {
        self.env_model.as_deref()
    }

    pub fn size(&self) -> TerminalSize {
        self.size
    }

    pub fn input_owner(&self) -> Option<&AttachmentId> {
        self.input_owner.as_ref()
    }

    pub fn state(&self) -> &TabState {
        &self.state
    }

    pub fn exit(&self) -> Option<&TabExit> {
        self.exit.as_ref()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TabEvent {
    Raw(Vec<u8>),
    Snapshot(ScreenSnapshot),
    Diff(ScreenDiff),
    FocusChanged {
        owner: Option<AttachmentId>,
        size: TerminalSize,
    },
    Metadata(TabDescriptor),
    Title(String),
    Bell,
    Exited(TabExit),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabError {
    code: &'static str,
    message: String,
}

impl TabError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn code(&self) -> &'static str {
        self.code
    }
}

impl fmt::Display for TabError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for TabError {}

pub trait PtyBackend: Send + Sync + 'static {
    fn spawn(&self, spec: PtySpawnSpec, sink: Arc<dyn PtySink>) -> Result<u32, String>;
    fn write(&self, id: u32, bytes: &[u8]) -> Result<(), String>;
    fn resize(&self, id: u32, cols: u16, rows: u16) -> Result<(), String>;
    fn kill(&self, id: u32);
    fn pty_for_descendant(&self, pid: u32) -> Option<u32>;
}

impl PtyBackend for PtyManager {
    fn spawn(&self, spec: PtySpawnSpec, sink: Arc<dyn PtySink>) -> Result<u32, String> {
        PtyManager::spawn(self, spec, sink)
    }

    fn write(&self, id: u32, bytes: &[u8]) -> Result<(), String> {
        PtyManager::write(self, id, bytes)
    }

    fn resize(&self, id: u32, cols: u16, rows: u16) -> Result<(), String> {
        PtyManager::resize(self, id, cols, rows)
    }

    fn kill(&self, id: u32) {
        PtyManager::kill(self, id)
    }

    fn pty_for_descendant(&self, pid: u32) -> Option<u32> {
        PtyManager::pty_for_descendant(self, pid)
    }
}

pub struct TabAttachment {
    pub id: AttachmentId,
    pub events: TabEventReceiver,
}

impl fmt::Debug for TabAttachment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TabAttachment")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

pub struct TabEventReceiver {
    receiver: Receiver<TabEvent>,
    registry: Weak<RegistryInner>,
    tab_id: TabId,
    attachment_id: AttachmentId,
}

impl fmt::Debug for TabEventReceiver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TabEventReceiver")
            .field("tab_id", &self.tab_id)
            .field("attachment_id", &self.attachment_id)
            .finish_non_exhaustive()
    }
}

impl TabEventReceiver {
    pub fn recv(&self) -> Result<TabEvent, RecvError> {
        if let Some(snapshot) = self.take_recovery() {
            return Ok(snapshot);
        }
        let event = self.receiver.recv()?;
        Ok(self.take_recovery().unwrap_or(event))
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Result<TabEvent, RecvTimeoutError> {
        if let Some(snapshot) = self.take_recovery() {
            return Ok(snapshot);
        }
        let event = self.receiver.recv_timeout(timeout)?;
        Ok(self.take_recovery().unwrap_or(event))
    }

    pub fn try_recv(&self) -> Result<TabEvent, TryRecvError> {
        if let Some(snapshot) = self.take_recovery() {
            return Ok(snapshot);
        }
        let event = self.receiver.try_recv()?;
        Ok(self.take_recovery().unwrap_or(event))
    }

    fn take_recovery(&self) -> Option<TabEvent> {
        let snapshot = self
            .registry
            .upgrade()?
            .take_recovery(&self.tab_id, &self.attachment_id)?;
        while self.receiver.try_recv().is_ok() {}
        Some(TabEvent::Snapshot(snapshot))
    }
}

impl Drop for TabEventReceiver {
    fn drop(&mut self) {
        if let Some(registry) = self.registry.upgrade() {
            registry.detach(&self.tab_id, &self.attachment_id);
        }
    }
}

#[derive(Clone)]
pub struct TabRegistry {
    inner: Arc<RegistryInner>,
}

impl Default for TabRegistry {
    fn default() -> Self {
        Self::new(PtyManager::default())
    }
}

impl TabRegistry {
    pub fn new(manager: PtyManager) -> Self {
        Self::with_backend(Arc::new(manager))
    }

    pub fn with_backend(backend: Arc<dyn PtyBackend>) -> Self {
        Self::with_backend_and_queue_capacity(backend, DEFAULT_QUEUE_CAPACITY)
    }

    pub fn with_backend_and_queue_capacity(
        backend: Arc<dyn PtyBackend>,
        queue_capacity: usize,
    ) -> Self {
        Self {
            inner: Arc::new(RegistryInner {
                backend,
                maps: Mutex::new(RegistryMaps::default()),
                queue_capacity: queue_capacity.max(1),
            }),
        }
    }

    pub fn open(&self, launch: TabLaunch) -> Result<TabId, TabError> {
        let id = TabId::new();
        let descriptor = TabDescriptor {
            id: id.clone(),
            title: launch.title,
            cwd: launch.cwd,
            command: launch.command,
            session_id: launch.session_id,
            resumed_id: launch.resumed_id,
            agent_id: launch.agent_id,
            slot_id: launch.slot_id,
            fresh: launch.fresh,
            env_provider: launch.env_provider,
            env_model: launch.env_model,
            size: launch.size,
            input_owner: None,
            state: TabState::Running,
            exit: None,
        };
        let spec = PtySpawnSpec {
            cwd: descriptor.cwd.clone(),
            command: descriptor.command.clone(),
            size: PtySize {
                rows: descriptor.size.rows(),
                cols: descriptor.size.cols(),
                pixel_width: 0,
                pixel_height: 0,
            },
            env_provider: descriptor.env_provider.clone(),
            env_model: descriptor.env_model.clone(),
        };
        let tab = Arc::new(Mutex::new(LiveTab {
            descriptor,
            screen: ScreenModel::new(launch.size),
            pty_id: None,
            attachments: HashMap::new(),
            pending_replies: Vec::new(),
            exit_notified: false,
        }));

        {
            let mut maps = self.inner.maps.lock().unwrap();
            let slot = tab.lock().unwrap().descriptor.slot_id.clone();
            if maps.by_slot.contains_key(&slot) {
                return Err(TabError::new(
                    "tab.slot_in_use",
                    "another tab already owns this slot",
                ));
            }
            maps.by_slot.insert(slot, id.clone());
            maps.order.push(id.clone());
            maps.by_id.insert(id.clone(), tab.clone());
        }

        let sink = Arc::new(TabSink {
            registry: Arc::downgrade(&self.inner),
            tab: Arc::downgrade(&tab),
        });
        let pty_id = match self.inner.backend.spawn(spec, sink) {
            Ok(pty_id) => pty_id,
            Err(error) => {
                let slot_id = tab.lock().unwrap().descriptor.slot_id.clone();
                self.inner.remove_tab(&id, &slot_id, None);
                return Err(TabError::new("tab.spawn_failed", error));
            }
        };

        let pending_replies = {
            let mut live = tab.lock().unwrap();
            if live.descriptor.state == TabState::Running {
                live.pty_id = Some(pty_id);
                let mut maps = self.inner.maps.lock().unwrap();
                maps.by_pty.insert(pty_id, id.clone());
            }
            std::mem::take(&mut live.pending_replies)
        };
        for reply in pending_replies {
            let _ = self.inner.backend.write(pty_id, &reply);
        }

        Ok(id)
    }

    pub fn list(&self) -> Vec<TabDescriptor> {
        let tabs = {
            let maps = self.inner.maps.lock().unwrap();
            maps.order
                .iter()
                .filter_map(|id| maps.by_id.get(id).cloned())
                .collect::<Vec<_>>()
        };
        tabs.into_iter()
            .map(|tab| tab.lock().unwrap().descriptor.clone())
            .collect()
    }

    pub fn get(&self, id: &TabId) -> Result<TabDescriptor, TabError> {
        let tab = self.inner.tab(id)?;
        let descriptor = tab.lock().unwrap().descriptor.clone();
        Ok(descriptor)
    }

    pub fn update(&self, id: &TabId, update: TabUpdate) -> Result<TabDescriptor, TabError> {
        let tab = self.inner.tab(id)?;
        let (descriptor, dispatches) = {
            let mut live = tab.lock().unwrap();
            if live.descriptor.state != TabState::Running {
                return Err(TabError::new("tab.closed", "the tab has exited"));
            }
            if let Some(slot) = update.slot_id {
                let mut maps = self.inner.maps.lock().unwrap();
                if maps.by_slot.get(&slot).is_some_and(|owner| owner != id) {
                    return Err(TabError::new(
                        "tab.slot_in_use",
                        "another tab already owns this slot",
                    ));
                }
                maps.by_slot.remove(&live.descriptor.slot_id);
                maps.by_slot.insert(slot.clone(), id.clone());
                live.descriptor.slot_id = slot;
            }
            if let Some(title) = update.title {
                live.descriptor.title = title;
            }
            if let Some(session_id) = update.session_id {
                live.descriptor.session_id = Some(session_id);
            }
            if let Some(resumed_id) = update.resumed_id {
                live.descriptor.resumed_id = Some(resumed_id);
            }
            if let Some(agent_id) = update.agent_id {
                live.descriptor.agent_id = Some(agent_id);
            }
            if let Some(fresh) = update.fresh {
                live.descriptor.fresh = fresh;
            }
            let descriptor = live.descriptor.clone();
            let dispatches = live.dispatches_to_all(TabEvent::Metadata(descriptor.clone()));
            (descriptor, dispatches)
        };
        dispatch(&tab, dispatches);
        Ok(descriptor)
    }

    pub fn rekey_session(
        &self,
        id: &TabId,
        session_id: impl Into<String>,
    ) -> Result<TabDescriptor, TabError> {
        let session_id = session_id.into();
        self.update(
            id,
            TabUpdate::new()
                .session_id(session_id.clone())
                .slot_id(session_id),
        )
    }

    pub fn attach(&self, id: &TabId, kind: AttachmentKind) -> Result<TabAttachment, TabError> {
        let tab = self.inner.tab(id)?;
        let attachment_id = AttachmentId::new();
        let (sender, receiver) = mpsc::sync_channel(self.inner.queue_capacity);
        let dispatches = {
            let mut live = tab.lock().unwrap();
            live.attachments.insert(
                attachment_id.clone(),
                AttachmentState {
                    kind,
                    sender: sender.clone(),
                    recover_snapshot: false,
                },
            );
            let mut dispatches = Vec::new();
            if kind == AttachmentKind::Remote {
                dispatches.push(Dispatch::new(
                    attachment_id.clone(),
                    kind,
                    sender,
                    TabEvent::Snapshot(live.screen.snapshot(id.as_str())),
                ));
            }
            if kind == AttachmentKind::Desktop && live.descriptor.input_owner.is_none() {
                live.descriptor.input_owner = Some(attachment_id.clone());
                dispatches.extend(live.dispatches_to_all(TabEvent::FocusChanged {
                    owner: Some(attachment_id.clone()),
                    size: live.descriptor.size,
                }));
            }
            dispatches
        };
        dispatch(&tab, dispatches);
        Ok(TabAttachment {
            id: attachment_id.clone(),
            events: TabEventReceiver {
                receiver,
                registry: Arc::downgrade(&self.inner),
                tab_id: id.clone(),
                attachment_id,
            },
        })
    }

    pub fn snapshot(&self, id: &TabId) -> Result<ScreenSnapshot, TabError> {
        let tab = self.inner.tab(id)?;
        let snapshot = tab.lock().unwrap().screen.snapshot(id.as_str());
        Ok(snapshot)
    }

    pub fn scrollback(
        &self,
        id: &TabId,
        offset: usize,
        count: usize,
    ) -> Result<Vec<ScreenRow>, TabError> {
        let tab = self.inner.tab(id)?;
        let page = tab.lock().unwrap().screen.scrollback_page(offset, count);
        Ok(page)
    }

    pub fn input(
        &self,
        id: &TabId,
        attachment: &AttachmentId,
        bytes: &[u8],
    ) -> Result<(), TabError> {
        let tab = self.inner.tab(id)?;
        let live = tab.lock().unwrap();
        live.authorize_owner(attachment)?;
        let pty_id = live.live_pty()?;
        self.inner
            .backend
            .write(pty_id, bytes)
            .map_err(|error| TabError::new("terminal.write_failed", error))
    }

    pub fn resize(
        &self,
        id: &TabId,
        attachment: &AttachmentId,
        size: TerminalSize,
    ) -> Result<(), TabError> {
        let tab = self.inner.tab(id)?;
        let dispatches = {
            let mut live = tab.lock().unwrap();
            live.authorize_owner(attachment)?;
            let pty_id = live.live_pty()?;
            self.inner
                .backend
                .resize(pty_id, size.cols(), size.rows())
                .map_err(|error| TabError::new("terminal.resize_failed", error))?;
            live.resize(id, size)
        };
        dispatch(&tab, dispatches);
        Ok(())
    }

    pub fn take_focus(
        &self,
        id: &TabId,
        attachment: &AttachmentId,
        size: TerminalSize,
    ) -> Result<(), TabError> {
        let tab = self.inner.tab(id)?;
        let dispatches = {
            let mut live = tab.lock().unwrap();
            if !live.attachments.contains_key(attachment) {
                return Err(TabError::new(
                    "terminal.attachment_not_found",
                    "the attachment does not belong to this tab",
                ));
            }
            let pty_id = live.live_pty()?;
            self.inner
                .backend
                .resize(pty_id, size.cols(), size.rows())
                .map_err(|error| TabError::new("terminal.resize_failed", error))?;
            live.descriptor.input_owner = Some(attachment.clone());
            let mut dispatches = live.resize(id, size);
            dispatches.extend(live.dispatches_to_all(TabEvent::FocusChanged {
                owner: Some(attachment.clone()),
                size,
            }));
            dispatches
        };
        dispatch(&tab, dispatches);
        Ok(())
    }

    pub fn close(&self, id: &TabId) -> Result<(), TabError> {
        let tab = self.inner.tab(id)?;
        let (pty_id, slot_id, dispatches) = {
            let mut live = tab.lock().unwrap();
            let pty_id = live.pty_id.take();
            let slot_id = live.descriptor.slot_id.clone();
            let dispatches = live.mark_exited(None, None, true);
            (pty_id, slot_id, dispatches)
        };
        self.inner.remove_tab(id, &slot_id, pty_id);
        dispatch(&tab, dispatches);
        if let Some(pty_id) = pty_id {
            self.inner.backend.kill(pty_id);
        }
        Ok(())
    }

    pub fn tab_for_descendant(&self, pid: u32) -> Option<TabId> {
        let pty_id = self.inner.backend.pty_for_descendant(pid)?;
        self.inner.maps.lock().ok()?.by_pty.get(&pty_id).cloned()
    }
}

struct RegistryInner {
    backend: Arc<dyn PtyBackend>,
    maps: Mutex<RegistryMaps>,
    queue_capacity: usize,
}

#[derive(Default)]
struct RegistryMaps {
    by_id: HashMap<TabId, Arc<Mutex<LiveTab>>>,
    by_slot: HashMap<String, TabId>,
    by_pty: HashMap<u32, TabId>,
    order: Vec<TabId>,
}

impl RegistryInner {
    fn tab(&self, id: &TabId) -> Result<Arc<Mutex<LiveTab>>, TabError> {
        self.maps
            .lock()
            .unwrap()
            .by_id
            .get(id)
            .cloned()
            .ok_or_else(|| TabError::new("tab.not_found", "unknown tab id"))
    }

    fn remove_tab(&self, id: &TabId, slot_id: &str, pty_id: Option<u32>) {
        let mut maps = self.maps.lock().unwrap();
        maps.by_id.remove(id);
        if maps.by_slot.get(slot_id) == Some(id) {
            maps.by_slot.remove(slot_id);
        }
        if let Some(pty_id) = pty_id {
            maps.by_pty.remove(&pty_id);
        }
        maps.order.retain(|candidate| candidate != id);
        maps.by_pty.retain(|_, tab_id| tab_id != id);
    }

    fn detach(&self, tab_id: &TabId, attachment_id: &AttachmentId) {
        let Ok(tab) = self.tab(tab_id) else {
            return;
        };
        let dispatches = {
            let mut live = tab.lock().unwrap();
            if live.attachments.remove(attachment_id).is_none() {
                return;
            }
            if live.descriptor.input_owner.as_ref() == Some(attachment_id) {
                live.descriptor.input_owner = None;
                live.dispatches_to_all(TabEvent::FocusChanged {
                    owner: None,
                    size: live.descriptor.size,
                })
            } else {
                Vec::new()
            }
        };
        dispatch(&tab, dispatches);
    }

    fn take_recovery(
        &self,
        tab_id: &TabId,
        attachment_id: &AttachmentId,
    ) -> Option<ScreenSnapshot> {
        let tab = self.tab(tab_id).ok()?;
        let mut live = tab.lock().ok()?;
        let attachment = live.attachments.get_mut(attachment_id)?;
        if attachment.kind != AttachmentKind::Remote || !attachment.recover_snapshot {
            return None;
        }
        attachment.recover_snapshot = false;
        Some(live.screen.snapshot(tab_id.as_str()))
    }
}

struct LiveTab {
    descriptor: TabDescriptor,
    screen: ScreenModel,
    pty_id: Option<u32>,
    attachments: HashMap<AttachmentId, AttachmentState>,
    pending_replies: Vec<Vec<u8>>,
    exit_notified: bool,
}

impl LiveTab {
    fn live_pty(&self) -> Result<u32, TabError> {
        if self.descriptor.state != TabState::Running {
            return Err(TabError::new("tab.closed", "the tab has exited"));
        }
        self.pty_id
            .ok_or_else(|| TabError::new("tab.not_ready", "the PTY is still starting"))
    }

    fn authorize_owner(&self, attachment: &AttachmentId) -> Result<(), TabError> {
        if !self.attachments.contains_key(attachment) {
            return Err(TabError::new(
                "terminal.attachment_not_found",
                "the attachment does not belong to this tab",
            ));
        }
        if self.descriptor.input_owner.as_ref() != Some(attachment) {
            return Err(TabError::new(
                "terminal.input_not_owned",
                "another attachment owns terminal input and resize",
            ));
        }
        Ok(())
    }

    fn dispatches_to_all(&self, event: TabEvent) -> Vec<Dispatch> {
        self.attachments
            .iter()
            .map(|(id, attachment)| {
                Dispatch::new(
                    id.clone(),
                    attachment.kind,
                    attachment.sender.clone(),
                    event.clone(),
                )
            })
            .collect()
    }

    fn dispatches_to_kind(&self, kind: AttachmentKind, event: TabEvent) -> Vec<Dispatch> {
        self.attachments
            .iter()
            .filter(|(_, attachment)| {
                attachment.kind == kind
                    && !(kind == AttachmentKind::Remote && attachment.recover_snapshot)
            })
            .map(|(id, attachment)| {
                Dispatch::new(id.clone(), kind, attachment.sender.clone(), event.clone())
            })
            .collect()
    }

    fn resize(&mut self, id: &TabId, size: TerminalSize) -> Vec<Dispatch> {
        self.descriptor.size = size;
        self.screen.resize(size);
        let snapshot = self.screen.snapshot(id.as_str());
        self.dispatches_to_kind(AttachmentKind::Remote, TabEvent::Snapshot(snapshot))
    }

    fn mark_exited(
        &mut self,
        code: Option<u32>,
        signal: Option<String>,
        requested: bool,
    ) -> Vec<Dispatch> {
        if self.exit_notified {
            return Vec::new();
        }
        self.exit_notified = true;
        self.descriptor.state = TabState::Exited;
        self.descriptor.input_owner = None;
        let exit = TabExit {
            code,
            signal,
            requested,
        };
        self.descriptor.exit = Some(exit.clone());
        self.dispatches_to_all(TabEvent::Exited(exit))
    }
}

struct AttachmentState {
    kind: AttachmentKind,
    sender: SyncSender<TabEvent>,
    recover_snapshot: bool,
}

struct Dispatch {
    attachment_id: AttachmentId,
    kind: AttachmentKind,
    sender: SyncSender<TabEvent>,
    event: TabEvent,
}

impl Dispatch {
    fn new(
        attachment_id: AttachmentId,
        kind: AttachmentKind,
        sender: SyncSender<TabEvent>,
        event: TabEvent,
    ) -> Self {
        Self {
            attachment_id,
            kind,
            sender,
            event,
        }
    }
}

fn dispatch(tab: &Arc<Mutex<LiveTab>>, dispatches: Vec<Dispatch>) {
    for item in dispatches {
        match item.sender.try_send(item.event) {
            Ok(()) => {}
            Err(mpsc::TrySendError::Full(_)) if item.kind == AttachmentKind::Remote => {
                if let Ok(mut live) = tab.lock() {
                    if let Some(attachment) = live.attachments.get_mut(&item.attachment_id) {
                        attachment.recover_snapshot = true;
                    }
                }
            }
            Err(mpsc::TrySendError::Full(_)) => {}
            Err(mpsc::TrySendError::Disconnected(_)) => {
                if let Ok(mut live) = tab.lock() {
                    let was_owner =
                        live.descriptor.input_owner.as_ref() == Some(&item.attachment_id);
                    live.attachments.remove(&item.attachment_id);
                    if was_owner {
                        live.descriptor.input_owner = None;
                    }
                }
            }
        }
    }
}

struct TabSink {
    registry: Weak<RegistryInner>,
    tab: Weak<Mutex<LiveTab>>,
}

impl PtySink for TabSink {
    fn output(&self, pty_id: u32, bytes: &[u8]) {
        let (Some(registry), Some(tab)) = (self.registry.upgrade(), self.tab.upgrade()) else {
            return;
        };
        let (dispatches, replies) = {
            let mut live = tab.lock().unwrap();
            if live.descriptor.state != TabState::Running {
                return;
            }
            let mut dispatches =
                live.dispatches_to_kind(AttachmentKind::Desktop, TabEvent::Raw(bytes.to_vec()));
            let damage = live.screen.process(bytes);
            if let Some(diff) = damage.diff {
                dispatches
                    .extend(live.dispatches_to_kind(AttachmentKind::Remote, TabEvent::Diff(diff)));
            }
            if let Some(title) = damage.title {
                live.descriptor.title = title.clone();
                dispatches.extend(live.dispatches_to_all(TabEvent::Title(title)));
            }
            if damage.bell {
                dispatches.extend(live.dispatches_to_all(TabEvent::Bell));
            }
            let desktop_owns = live
                .descriptor
                .input_owner
                .as_ref()
                .and_then(|owner| live.attachments.get(owner))
                .is_some_and(|attachment| attachment.kind == AttachmentKind::Desktop);
            let replies = if desktop_owns {
                Vec::new()
            } else if live.pty_id.is_some() {
                damage.replies
            } else {
                live.pending_replies.extend(damage.replies);
                Vec::new()
            };
            (dispatches, replies)
        };
        dispatch(&tab, dispatches);
        for reply in replies {
            let _ = registry.backend.write(pty_id, &reply);
        }
    }

    fn exited(&self, pty_id: u32, code: Option<u32>, signal: Option<&str>) {
        let (Some(registry), Some(tab)) = (self.registry.upgrade(), self.tab.upgrade()) else {
            return;
        };
        let dispatches = {
            let mut live = tab.lock().unwrap();
            live.pty_id = None;
            live.mark_exited(code, signal.map(str::to_owned), false)
        };
        registry.maps.lock().unwrap().by_pty.remove(&pty_id);
        dispatch(&tab, dispatches);
    }
}

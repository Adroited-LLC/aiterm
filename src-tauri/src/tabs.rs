use crate::pty::{PtyManager, PtySink, PtySpawnSpec};
use crate::remote::model::TerminalSize;
use crate::terminal::model::{ScreenDiff, ScreenRow, ScreenSnapshot};
use crate::terminal::screen::ScreenModel;
use portable_pty::PtySize;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::{RecvError, RecvTimeoutError, TryRecvError};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, Instant};
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Emitter, State};
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
    #[serde(default)]
    fresh: bool,
    env_provider: Option<String>,
    env_model: Option<String>,
    size: TerminalSize,
    #[serde(skip)]
    desktop_pending: bool,
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
            desktop_pending: false,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum ControlKind {
    Focus,
    Metadata,
    Title,
    Bell,
    Exited,
}

struct QueuedEvent {
    sequence: u64,
    event: TabEvent,
}

struct MailboxState {
    next_sequence: u64,
    screen: VecDeque<QueuedEvent>,
    raw: VecDeque<QueuedEvent>,
    controls: HashMap<ControlKind, QueuedEvent>,
    raw_cancelled: bool,
    receiver_closed: bool,
    producer_closed: bool,
}

impl Default for MailboxState {
    fn default() -> Self {
        Self {
            next_sequence: 0,
            screen: VecDeque::new(),
            raw: VecDeque::new(),
            controls: HashMap::new(),
            raw_cancelled: false,
            receiver_closed: false,
            producer_closed: false,
        }
    }
}

struct EventMailbox {
    kind: AttachmentKind,
    capacity: usize,
    state: Mutex<MailboxState>,
    changed: Condvar,
}

impl EventMailbox {
    fn new(kind: AttachmentKind, capacity: usize) -> Self {
        Self {
            kind,
            capacity: capacity.max(1),
            state: Mutex::new(MailboxState::default()),
            changed: Condvar::new(),
        }
    }

    fn push_initial_snapshot(&self, snapshot: ScreenSnapshot) {
        let mut state = self.state.lock().unwrap();
        if state.receiver_closed || state.producer_closed {
            return;
        }
        let sequence = take_sequence(&mut state);
        state.screen.push_back(QueuedEvent {
            sequence,
            event: TabEvent::Snapshot(snapshot),
        });
        self.changed.notify_one();
    }

    fn push_snapshot(&self, snapshot: ScreenSnapshot) {
        self.push_screen(TabEvent::Snapshot(snapshot.clone()), || snapshot);
    }

    fn push_diff(&self, diff: ScreenDiff, recovery: impl FnOnce() -> ScreenSnapshot) {
        self.push_screen(TabEvent::Diff(diff), recovery);
    }

    fn push_screen(&self, event: TabEvent, recovery: impl FnOnce() -> ScreenSnapshot) {
        debug_assert_eq!(self.kind, AttachmentKind::Remote);
        let mut state = self.state.lock().unwrap();
        if state.receiver_closed || state.producer_closed {
            return;
        }
        let event_sequence = take_sequence(&mut state);
        if state.screen.len() >= self.capacity {
            // Keep the earliest replaced event's position relative to control
            // events. The snapshot semantically supersedes every removed
            // screen event, while later diffs receive later sequence numbers.
            let sequence = state
                .screen
                .front()
                .map(|queued| queued.sequence)
                .unwrap_or(event_sequence);
            state.screen.clear();
            state.screen.push_back(QueuedEvent {
                sequence,
                event: TabEvent::Snapshot(recovery()),
            });
        } else {
            state.screen.push_back(QueuedEvent {
                sequence: event_sequence,
                event,
            });
        }
        self.changed.notify_one();
    }

    fn push_raw(&self, bytes: Vec<u8>) -> bool {
        debug_assert_eq!(self.kind, AttachmentKind::Desktop);
        let mut state = self.state.lock().unwrap();
        while state.raw.len() >= self.capacity
            && !state.raw_cancelled
            && !state.receiver_closed
            && !state.producer_closed
        {
            state = self.changed.wait(state).unwrap();
        }
        if state.raw_cancelled || state.receiver_closed || state.producer_closed {
            return false;
        }
        let sequence = take_sequence(&mut state);
        state.raw.push_back(QueuedEvent {
            sequence,
            event: TabEvent::Raw(bytes),
        });
        self.changed.notify_one();
        true
    }

    fn cancel_raw(&self) {
        let mut state = self.state.lock().unwrap();
        state.raw_cancelled = true;
        self.changed.notify_all();
    }

    fn push_control(&self, event: TabEvent) {
        let Some(kind) = control_kind(&event) else {
            return;
        };
        let mut state = self.state.lock().unwrap();
        if state.receiver_closed || state.producer_closed {
            return;
        }
        if kind == ControlKind::Exited && state.controls.contains_key(&kind) {
            return;
        }
        let sequence = take_sequence(&mut state);
        state.controls.insert(kind, QueuedEvent { sequence, event });
        self.changed.notify_one();
    }

    fn finish(&self, exit: TabExit) {
        let mut state = self.state.lock().unwrap();
        if state.receiver_closed || state.producer_closed {
            return;
        }
        if !state.controls.contains_key(&ControlKind::Exited) {
            let sequence = take_sequence(&mut state);
            state.controls.insert(
                ControlKind::Exited,
                QueuedEvent {
                    sequence,
                    event: TabEvent::Exited(exit),
                },
            );
        }
        state.producer_closed = true;
        self.changed.notify_all();
    }

    fn close_receiver(&self) {
        let mut state = self.state.lock().unwrap();
        state.receiver_closed = true;
        state.screen.clear();
        state.raw.clear();
        state.controls.clear();
        self.changed.notify_all();
    }

    fn recv(&self) -> Result<TabEvent, RecvError> {
        let mut state = self.state.lock().unwrap();
        loop {
            if let Some(event) = pop_next(&mut state) {
                self.changed.notify_all();
                return Ok(event);
            }
            if state.receiver_closed || state.producer_closed {
                return Err(RecvError);
            }
            state = self.changed.wait(state).unwrap();
        }
    }

    fn recv_timeout(&self, timeout: Duration) -> Result<TabEvent, RecvTimeoutError> {
        let deadline = Instant::now() + timeout;
        let mut state = self.state.lock().unwrap();
        loop {
            if let Some(event) = pop_next(&mut state) {
                self.changed.notify_all();
                return Ok(event);
            }
            if state.receiver_closed || state.producer_closed {
                return Err(RecvTimeoutError::Disconnected);
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(RecvTimeoutError::Timeout);
            }
            let (next, timeout_result) = self.changed.wait_timeout(state, deadline - now).unwrap();
            state = next;
            if timeout_result.timed_out()
                && state.screen.is_empty()
                && state.raw.is_empty()
                && state.controls.is_empty()
            {
                return Err(RecvTimeoutError::Timeout);
            }
        }
    }

    fn try_recv(&self) -> Result<TabEvent, TryRecvError> {
        let mut state = self.state.lock().unwrap();
        if let Some(event) = pop_next(&mut state) {
            self.changed.notify_all();
            return Ok(event);
        }
        if state.receiver_closed || state.producer_closed {
            Err(TryRecvError::Disconnected)
        } else {
            Err(TryRecvError::Empty)
        }
    }
}

fn take_sequence(state: &mut MailboxState) -> u64 {
    let sequence = state.next_sequence;
    state.next_sequence = state.next_sequence.saturating_add(1);
    sequence
}

fn control_kind(event: &TabEvent) -> Option<ControlKind> {
    match event {
        TabEvent::FocusChanged { .. } => Some(ControlKind::Focus),
        TabEvent::Metadata(_) => Some(ControlKind::Metadata),
        TabEvent::Title(_) => Some(ControlKind::Title),
        TabEvent::Bell => Some(ControlKind::Bell),
        TabEvent::Exited(_) => Some(ControlKind::Exited),
        TabEvent::Raw(_) | TabEvent::Snapshot(_) | TabEvent::Diff(_) => None,
    }
}

fn pop_next(state: &mut MailboxState) -> Option<TabEvent> {
    enum Lane {
        Screen,
        Raw,
        Control(ControlKind),
    }

    let mut next: Option<(u64, Lane)> = state
        .screen
        .front()
        .map(|queued| (queued.sequence, Lane::Screen));
    if let Some(raw) = state.raw.front() {
        if next
            .as_ref()
            .is_none_or(|(sequence, _)| raw.sequence < *sequence)
        {
            next = Some((raw.sequence, Lane::Raw));
        }
    }
    for (kind, queued) in &state.controls {
        if next
            .as_ref()
            .is_none_or(|(sequence, _)| queued.sequence < *sequence)
        {
            next = Some((queued.sequence, Lane::Control(*kind)));
        }
    }

    match next?.1 {
        Lane::Screen => state.screen.pop_front().map(|queued| queued.event),
        Lane::Raw => state.raw.pop_front().map(|queued| queued.event),
        Lane::Control(kind) => state.controls.remove(&kind).map(|queued| queued.event),
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
    mailbox: Arc<EventMailbox>,
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
        self.mailbox.recv()
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Result<TabEvent, RecvTimeoutError> {
        self.mailbox.recv_timeout(timeout)
    }

    pub fn try_recv(&self) -> Result<TabEvent, TryRecvError> {
        self.mailbox.try_recv()
    }
}

impl Drop for TabEventReceiver {
    fn drop(&mut self) {
        // Wake a desktop producer before trying to take the per-tab lock it may
        // hold while applying lossless backpressure.
        self.mailbox.close_receiver();
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

    pub fn open_desktop(&self, mut launch: TabLaunch) -> Result<TabId, TabError> {
        launch.desktop_pending = true;
        self.open(launch)
    }

    pub fn open(&self, launch: TabLaunch) -> Result<TabId, TabError> {
        let id = TabId::new();
        let slot_id = launch.slot_id.clone();
        let pending_desktop_raw = launch.desktop_pending.then(Vec::new);
        {
            let mut maps = self.inner.maps.lock().unwrap();
            if maps.by_slot.contains_key(&slot_id) || maps.pending_slots.contains_key(&slot_id) {
                return Err(TabError::new(
                    "tab.slot_in_use",
                    "another tab already owns this slot",
                ));
            }
            maps.pending_slots.insert(slot_id.clone(), id.clone());
        }
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
        let tab = Arc::new(TabCell {
            live: Mutex::new(LiveTab {
                descriptor,
                screen: ScreenModel::new(launch.size),
                pty: PtyBinding::Pending,
                attachments: HashMap::new(),
                pending_desktop_raw,
                pending_replies: VecDeque::new(),
                exit_notified: false,
            }),
            raw: RawDispatch::default(),
        });

        let sink = Arc::new(TabSink {
            registry: Arc::downgrade(&self.inner),
            tab: Arc::downgrade(&tab),
        });
        let pty_id = match self.inner.backend.spawn(spec, sink) {
            Ok(pty_id) => pty_id,
            Err(error) => {
                self.inner.release_pending_slot(&slot_id, &id);
                return Err(TabError::new("tab.spawn_failed", error));
            }
        };

        let exited_publication = {
            let _output_order = tab.raw.send_order.lock().unwrap();
            let mut live = tab.live.lock().unwrap();
            if live.descriptor.state == TabState::Exited {
                Some(self.inner.publish_locked(&id, &tab, &live, None))
            } else {
                live.pty = PtyBinding::Flushing(pty_id);
                None
            }
        };
        if let Some(publication) = exited_publication {
            if let Err(error) = publication {
                self.inner.release_pending_slot(&slot_id, &id);
                self.inner.backend.kill(pty_id);
                return Err(error);
            }
            return Ok(id);
        }

        loop {
            let (reply, publication) = {
                let _output_order = tab.raw.send_order.lock().unwrap();
                let mut live = tab.live.lock().unwrap();
                if live.descriptor.state == TabState::Exited {
                    (
                        None,
                        Some(self.inner.publish_locked(&id, &tab, &live, None)),
                    )
                } else {
                    match live.pending_replies.pop_front() {
                        Some(reply) => (Some(reply), None),
                        None => {
                            live.pty = PtyBinding::Ready(pty_id);
                            (
                                None,
                                Some(self.inner.publish_locked(&id, &tab, &live, Some(pty_id))),
                            )
                        }
                    }
                }
            };
            if let Some(publication) = publication {
                if let Err(error) = publication {
                    self.inner.release_pending_slot(&slot_id, &id);
                    self.inner.backend.kill(pty_id);
                    return Err(error);
                }
                return Ok(id);
            }
            let Some(reply) = reply else {
                unreachable!("binding either writes a reply or publishes the tab");
            };
            if let Err(error) = self.inner.backend.write(pty_id, &reply) {
                let exited = tab.live.lock().unwrap().descriptor.state == TabState::Exited;
                if exited {
                    continue;
                }
                self.inner.release_pending_slot(&slot_id, &id);
                self.inner.backend.kill(pty_id);
                return Err(TabError::new("tab.reply_failed", error));
            }
        }
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
            .map(|tab| tab.live.lock().unwrap().descriptor.clone())
            .collect()
    }

    pub fn get(&self, id: &TabId) -> Result<TabDescriptor, TabError> {
        let tab = self.inner.tab(id)?;
        let descriptor = tab.live.lock().unwrap().descriptor.clone();
        Ok(descriptor)
    }

    pub fn update(&self, id: &TabId, update: TabUpdate) -> Result<TabDescriptor, TabError> {
        let tab = self.inner.tab(id)?;
        let _output_order = tab.raw.send_order.lock().unwrap();
        tab.raw.require_open()?;
        let descriptor = {
            let mut live = tab.live.lock().unwrap();
            if live.descriptor.state != TabState::Running {
                return Err(TabError::new("tab.closed", "the tab has exited"));
            }
            if let Some(slot) = update.slot_id {
                let mut maps = self.inner.maps.lock().unwrap();
                if maps.by_slot.get(&slot).is_some_and(|owner| owner != id)
                    || maps
                        .pending_slots
                        .get(&slot)
                        .is_some_and(|owner| owner != id)
                {
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
            live.enqueue_control_all(TabEvent::Metadata(descriptor.clone()));
            descriptor
        };
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
        let mailbox = Arc::new(EventMailbox::new(kind, self.inner.queue_capacity));
        let _output_order = tab.raw.send_order.lock().unwrap();
        tab.raw.require_open()?;
        {
            let mut live = tab.live.lock().unwrap();
            if live.descriptor.state != TabState::Running {
                return Err(TabError::new("tab.closed", "the tab has exited"));
            }
            // The snapshot is in the mailbox before the attachment enters the
            // tab's subscriber map. Output cannot discover this attachment and
            // enqueue a diff ahead of its initial state.
            if kind == AttachmentKind::Remote {
                mailbox.push_initial_snapshot(live.screen.snapshot(id.as_str()));
            }
            if kind == AttachmentKind::Desktop && !tab.raw.register(attachment_id.clone(), &mailbox)
            {
                return Err(TabError::new("tab.closed", "the tab is closing"));
            }
            if kind == AttachmentKind::Desktop {
                if let Some(pending) = live.pending_desktop_raw.take() {
                    if !pending.is_empty() {
                        let _ = mailbox.push_raw(pending);
                    }
                }
            }
            live.attachments.insert(
                attachment_id.clone(),
                AttachmentState {
                    kind,
                    mailbox: mailbox.clone(),
                },
            );
            if kind == AttachmentKind::Desktop && live.descriptor.input_owner.is_none() {
                live.descriptor.input_owner = Some(attachment_id.clone());
                live.enqueue_control_all(TabEvent::FocusChanged {
                    owner: Some(attachment_id.clone()),
                    size: live.descriptor.size,
                });
            }
        }
        Ok(TabAttachment {
            id: attachment_id.clone(),
            events: TabEventReceiver {
                mailbox,
                registry: Arc::downgrade(&self.inner),
                tab_id: id.clone(),
                attachment_id,
            },
        })
    }

    pub fn snapshot(&self, id: &TabId) -> Result<ScreenSnapshot, TabError> {
        let tab = self.inner.tab(id)?;
        let _output_order = tab.raw.send_order.lock().unwrap();
        let snapshot = tab.live.lock().unwrap().screen.snapshot(id.as_str());
        Ok(snapshot)
    }

    pub fn scrollback(
        &self,
        id: &TabId,
        offset: usize,
        count: usize,
    ) -> Result<Vec<ScreenRow>, TabError> {
        let tab = self.inner.tab(id)?;
        let _output_order = tab.raw.send_order.lock().unwrap();
        let page = tab
            .live
            .lock()
            .unwrap()
            .screen
            .scrollback_page(offset, count);
        Ok(page)
    }

    pub fn input(
        &self,
        id: &TabId,
        attachment: &AttachmentId,
        bytes: &[u8],
    ) -> Result<(), TabError> {
        let tab = self.inner.tab(id)?;
        let _output_order = tab.raw.send_order.lock().unwrap();
        tab.raw.require_open()?;
        let live = tab.live.lock().unwrap();
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
        let _output_order = tab.raw.send_order.lock().unwrap();
        tab.raw.require_open()?;
        {
            let mut live = tab.live.lock().unwrap();
            live.authorize_owner(attachment)?;
            let pty_id = live.live_pty()?;
            self.inner
                .backend
                .resize(pty_id, size.cols(), size.rows())
                .map_err(|error| TabError::new("terminal.resize_failed", error))?;
            live.resize(id, size);
        }
        Ok(())
    }

    pub fn take_focus(
        &self,
        id: &TabId,
        attachment: &AttachmentId,
        size: TerminalSize,
    ) -> Result<(), TabError> {
        let tab = self.inner.tab(id)?;
        let _output_order = tab.raw.send_order.lock().unwrap();
        tab.raw.require_open()?;
        {
            let mut live = tab.live.lock().unwrap();
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
            live.resize(id, size);
            live.enqueue_control_all(TabEvent::FocusChanged {
                owner: Some(attachment.clone()),
                size,
            });
        }
        Ok(())
    }

    pub fn close(&self, id: &TabId) -> Result<(), TabError> {
        let tab = self.inner.tab(id)?;
        // Cancellation is independent of both the output-order gate and live
        // state. Wake a bounded raw producer first, then join its transaction
        // before publishing Exited.
        tab.raw.close();
        let (pty_id, slot_id) = {
            let _output_order = tab.raw.send_order.lock().unwrap();
            let mut live = tab.live.lock().unwrap();
            let pty_id = live.pty.id();
            live.pty = PtyBinding::Exited;
            let slot_id = live.descriptor.slot_id.clone();
            live.mark_exited(None, None, true);
            (pty_id, slot_id)
        };
        self.inner.remove_tab(id, &slot_id, pty_id);
        if let Some(pty_id) = pty_id {
            self.inner.backend.kill(pty_id);
        }
        Ok(())
    }

    pub fn detach(&self, id: &TabId, attachment: &AttachmentId) -> Result<(), TabError> {
        self.inner.tab(id)?;
        if self.inner.detach(id, attachment) {
            Ok(())
        } else {
            Err(TabError::new(
                "terminal.attachment_not_found",
                "the attachment does not belong to this tab",
            ))
        }
    }

    pub fn tab_for_descendant(&self, pid: u32) -> Option<TabId> {
        let pty_id = self.inner.backend.pty_for_descendant(pid)?;
        self.inner.maps.lock().ok()?.by_pty.get(&pty_id).cloned()
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DesktopTabExit {
    tab_id: TabId,
    code: Option<u32>,
    signal: Option<String>,
}

fn command_error(error: TabError) -> String {
    error.to_string()
}

fn emit_desktop_exit(app: &AppHandle, tab_id: &TabId, exit: &TabExit) {
    let _ = app.emit(
        "tab://exit",
        DesktopTabExit {
            tab_id: tab_id.clone(),
            code: exit.code(),
            signal: exit.signal().map(str::to_owned),
        },
    );
}

#[tauri::command]
pub async fn tab_open(
    state: State<'_, TabRegistry>,
    launch: TabLaunch,
) -> Result<TabDescriptor, String> {
    let registry = (*state).clone();
    crate::run_blocking(move || {
        let id = registry.open_desktop(launch).map_err(command_error)?;
        registry.get(&id).map_err(command_error)
    })
    .await
}

#[tauri::command]
pub fn tab_list(state: State<'_, TabRegistry>) -> Vec<TabDescriptor> {
    state.list()
}

#[tauri::command]
pub fn tab_update(
    state: State<'_, TabRegistry>,
    tab_id: TabId,
    update: TabUpdate,
) -> Result<TabDescriptor, String> {
    state.update(&tab_id, update).map_err(command_error)
}

#[tauri::command]
pub fn tab_attach_desktop(
    app: AppHandle,
    state: State<'_, TabRegistry>,
    tab_id: TabId,
    on_output: Channel<InvokeResponseBody>,
) -> Result<AttachmentId, String> {
    let registry = (*state).clone();
    let attachment = match registry.attach(&tab_id, AttachmentKind::Desktop) {
        Ok(attachment) => attachment,
        Err(error) => {
            if let Ok(descriptor) = registry.get(&tab_id) {
                if let Some(exit) = descriptor.exit() {
                    emit_desktop_exit(&app, &tab_id, exit);
                }
            }
            return Err(command_error(error));
        }
    };
    let attachment_id = attachment.id.clone();
    let events = attachment.events;
    std::thread::Builder::new()
        .name(format!("desktop-tab-{}", tab_id.as_str()))
        .spawn(move || {
            while let Ok(event) = events.recv() {
                match event {
                    TabEvent::Raw(bytes) => {
                        let _ = on_output.send(InvokeResponseBody::Raw(bytes));
                    }
                    TabEvent::Metadata(descriptor) => {
                        let _ = app.emit("tab://changed", descriptor);
                    }
                    TabEvent::Title(_) => {
                        if let Ok(descriptor) = registry.get(&tab_id) {
                            let _ = app.emit("tab://changed", descriptor);
                        }
                    }
                    TabEvent::Exited(exit) => {
                        emit_desktop_exit(&app, &tab_id, &exit);
                    }
                    TabEvent::Snapshot(_)
                    | TabEvent::Diff(_)
                    | TabEvent::FocusChanged { .. }
                    | TabEvent::Bell => {}
                }
            }
        })
        .map_err(|error| error.to_string())?;
    Ok(attachment_id)
}

#[tauri::command]
pub async fn tab_detach(
    state: State<'_, TabRegistry>,
    tab_id: TabId,
    attachment_id: AttachmentId,
) -> Result<(), String> {
    let registry = (*state).clone();
    crate::run_blocking(move || {
        registry
            .detach(&tab_id, &attachment_id)
            .map_err(command_error)
    })
    .await
}

#[tauri::command]
pub async fn tab_write(
    state: State<'_, TabRegistry>,
    tab_id: TabId,
    attachment_id: AttachmentId,
    data: String,
) -> Result<(), String> {
    let registry = (*state).clone();
    crate::run_blocking(move || {
        registry
            .input(&tab_id, &attachment_id, data.as_bytes())
            .map_err(command_error)
    })
    .await
}

fn terminal_size(cols: u16, rows: u16) -> Result<TerminalSize, String> {
    TerminalSize::try_new(cols, rows).map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn tab_resize(
    state: State<'_, TabRegistry>,
    tab_id: TabId,
    attachment_id: AttachmentId,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let size = terminal_size(cols, rows)?;
    let registry = (*state).clone();
    crate::run_blocking(move || {
        registry
            .resize(&tab_id, &attachment_id, size)
            .map_err(command_error)
    })
    .await
}

#[tauri::command]
pub async fn tab_take_focus(
    state: State<'_, TabRegistry>,
    tab_id: TabId,
    attachment_id: AttachmentId,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let size = terminal_size(cols, rows)?;
    let registry = (*state).clone();
    crate::run_blocking(move || {
        registry
            .take_focus(&tab_id, &attachment_id, size)
            .map_err(command_error)
    })
    .await
}

#[tauri::command]
pub async fn tab_close(state: State<'_, TabRegistry>, tab_id: TabId) -> Result<(), String> {
    let registry = (*state).clone();
    crate::run_blocking(move || registry.close(&tab_id).map_err(command_error)).await
}

struct RegistryInner {
    backend: Arc<dyn PtyBackend>,
    maps: Mutex<RegistryMaps>,
    queue_capacity: usize,
}

#[derive(Default)]
struct RegistryMaps {
    by_id: HashMap<TabId, Arc<TabCell>>,
    by_slot: HashMap<String, TabId>,
    by_pty: HashMap<u32, TabId>,
    pending_slots: HashMap<String, TabId>,
    order: Vec<TabId>,
}

struct TabCell {
    live: Mutex<LiveTab>,
    raw: RawDispatch,
}

#[derive(Default)]
struct RawDispatch {
    phase: AtomicU8,
    mailboxes: Mutex<HashMap<AttachmentId, Weak<EventMailbox>>>,
    send_order: Mutex<()>,
}

impl RawDispatch {
    fn register(&self, id: AttachmentId, mailbox: &Arc<EventMailbox>) -> bool {
        if self.phase.load(Ordering::Acquire) != RAW_OPEN {
            return false;
        }
        let mut mailboxes = self.mailboxes.lock().unwrap();
        if self.phase.load(Ordering::Acquire) != RAW_OPEN {
            return false;
        }
        mailboxes.insert(id, Arc::downgrade(mailbox));
        true
    }

    fn unregister(&self, id: &AttachmentId) {
        self.mailboxes.lock().unwrap().remove(id);
    }

    fn cancel_waits(&self) {
        let mailboxes = self
            .mailboxes
            .lock()
            .unwrap()
            .values()
            .filter_map(Weak::upgrade)
            .collect::<Vec<_>>();
        for mailbox in mailboxes {
            mailbox.cancel_raw();
        }
    }

    fn prepare_exit(&self) {
        let _ = self.phase.compare_exchange(
            RAW_OPEN,
            RAW_PREPARING_EXIT,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
        self.cancel_waits();
    }

    fn close(&self) {
        self.phase.store(RAW_CLOSING, Ordering::Release);
        self.cancel_waits();
    }

    fn is_closing(&self) -> bool {
        self.phase.load(Ordering::Acquire) == RAW_CLOSING
    }

    fn require_open(&self) -> Result<(), TabError> {
        if self.phase.load(Ordering::Acquire) == RAW_OPEN {
            Ok(())
        } else {
            Err(TabError::new("tab.closed", "the tab is closing"))
        }
    }
}

const RAW_OPEN: u8 = 0;
const RAW_PREPARING_EXIT: u8 = 1;
const RAW_CLOSING: u8 = 2;

impl RegistryInner {
    fn tab(&self, id: &TabId) -> Result<Arc<TabCell>, TabError> {
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

    fn release_pending_slot(&self, slot_id: &str, id: &TabId) {
        let mut maps = self.maps.lock().unwrap();
        if maps.pending_slots.get(slot_id) == Some(id) {
            maps.pending_slots.remove(slot_id);
        }
    }

    /// Publish only while the caller holds this tab's lock. This makes the
    /// Ready/Exited state and every public index appear as one transition.
    fn publish_locked(
        &self,
        id: &TabId,
        tab: &Arc<TabCell>,
        live: &LiveTab,
        pty_id: Option<u32>,
    ) -> Result<(), TabError> {
        let mut maps = self.maps.lock().unwrap();
        if maps.pending_slots.get(&live.descriptor.slot_id) != Some(id) {
            return Err(TabError::new(
                "tab.slot_reservation_lost",
                "the opening tab no longer owns its slot reservation",
            ));
        }
        if maps
            .by_slot
            .get(&live.descriptor.slot_id)
            .is_some_and(|owner| owner != id)
        {
            return Err(TabError::new(
                "tab.slot_in_use",
                "another tab already owns this slot",
            ));
        }
        maps.pending_slots.remove(&live.descriptor.slot_id);
        maps.by_slot
            .insert(live.descriptor.slot_id.clone(), id.clone());
        maps.order.push(id.clone());
        maps.by_id.insert(id.clone(), tab.clone());
        if let Some(pty_id) = pty_id {
            maps.by_pty.insert(pty_id, id.clone());
        }
        Ok(())
    }

    fn detach(&self, tab_id: &TabId, attachment_id: &AttachmentId) -> bool {
        let Ok(tab) = self.tab(tab_id) else {
            return false;
        };
        let _output_order = tab.raw.send_order.lock().unwrap();
        let removed = {
            let mut live = tab.live.lock().unwrap();
            let Some(attachment) = live.attachments.remove(attachment_id) else {
                return false;
            };
            if live.descriptor.input_owner.as_ref() == Some(attachment_id) {
                live.descriptor.input_owner = None;
                live.enqueue_control_all(TabEvent::FocusChanged {
                    owner: None,
                    size: live.descriptor.size,
                });
            }
            attachment.kind
        };
        if removed == AttachmentKind::Desktop {
            tab.raw.unregister(attachment_id);
        }
        true
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PtyBinding {
    Pending,
    Flushing(u32),
    Ready(u32),
    Exited,
}

impl PtyBinding {
    fn id(self) -> Option<u32> {
        match self {
            Self::Flushing(id) | Self::Ready(id) => Some(id),
            Self::Pending | Self::Exited => None,
        }
    }
}

struct LiveTab {
    descriptor: TabDescriptor,
    screen: ScreenModel,
    pty: PtyBinding,
    attachments: HashMap<AttachmentId, AttachmentState>,
    /// Lossless bytes emitted between `tab_open` and the first desktop attach.
    /// Once taken, later desktop attachments observe only live output.
    pending_desktop_raw: Option<Vec<u8>>,
    pending_replies: VecDeque<Vec<u8>>,
    exit_notified: bool,
}

impl LiveTab {
    fn live_pty(&self) -> Result<u32, TabError> {
        if self.descriptor.state != TabState::Running {
            return Err(TabError::new("tab.closed", "the tab has exited"));
        }
        self.pty
            .id()
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

    fn enqueue_control_all(&self, event: TabEvent) {
        for attachment in self.attachments.values() {
            attachment.mailbox.push_control(event.clone());
        }
    }

    fn desktop_mailboxes(&self) -> Vec<Arc<EventMailbox>> {
        self.attachments
            .values()
            .filter(|attachment| attachment.kind == AttachmentKind::Desktop)
            .map(|attachment| attachment.mailbox.clone())
            .collect()
    }

    fn enqueue_remote_diff(&self, id: &TabId, diff: ScreenDiff) {
        for attachment in self
            .attachments
            .values()
            .filter(|attachment| attachment.kind == AttachmentKind::Remote)
        {
            attachment
                .mailbox
                .push_diff(diff.clone(), || self.screen.snapshot(id.as_str()));
        }
    }

    fn resize(&mut self, id: &TabId, size: TerminalSize) {
        self.descriptor.size = size;
        self.screen.resize(size);
        let snapshot = self.screen.snapshot(id.as_str());
        for attachment in self
            .attachments
            .values()
            .filter(|attachment| attachment.kind == AttachmentKind::Remote)
        {
            attachment.mailbox.push_snapshot(snapshot.clone());
        }
    }

    fn mark_exited(&mut self, code: Option<u32>, signal: Option<String>, requested: bool) {
        if self.exit_notified {
            return;
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
        for attachment in self.attachments.values() {
            attachment.mailbox.finish(exit.clone());
        }
    }

    fn queue_replies(&mut self, replies: Vec<Vec<u8>>) -> Option<u32> {
        let desktop_owns = self.pending_desktop_raw.is_some()
            || self
                .descriptor
                .input_owner
                .as_ref()
                .and_then(|owner| self.attachments.get(owner))
                .is_some_and(|attachment| attachment.kind == AttachmentKind::Desktop);
        if desktop_owns {
            return None;
        }
        self.pending_replies.extend(replies);
        if let PtyBinding::Ready(pty_id) = self.pty {
            if !self.pending_replies.is_empty() {
                self.pty = PtyBinding::Flushing(pty_id);
                return Some(pty_id);
            }
        }
        None
    }
}

struct AttachmentState {
    kind: AttachmentKind,
    mailbox: Arc<EventMailbox>,
}

struct TabSink {
    registry: Weak<RegistryInner>,
    tab: Weak<TabCell>,
}

impl PtySink for TabSink {
    fn output(&self, _pty_id: u32, bytes: &[u8]) {
        let (Some(registry), Some(tab)) = (self.registry.upgrade(), self.tab.upgrade()) else {
            return;
        };
        // The PTY reader is normally the only producer, but keep raw chunks
        // ordered even for controlled/concurrent sinks. Close flips persistent
        // cancellation without this guard, then joins it after raw push wakes.
        let _send_order = tab.raw.send_order.lock().unwrap();
        if tab.raw.is_closing() {
            return;
        }
        let desktop_mailboxes = {
            let mut live = tab.live.lock().unwrap();
            if live.descriptor.state != TabState::Running {
                return;
            }
            let mailboxes = live.desktop_mailboxes();
            if mailboxes.is_empty() {
                if let Some(pending) = &mut live.pending_desktop_raw {
                    pending.extend_from_slice(bytes);
                }
            }
            mailboxes
        };
        for mailbox in desktop_mailboxes {
            let _ = mailbox.push_raw(bytes.to_vec());
            if tab.raw.is_closing() {
                return;
            }
        }
        if tab.raw.is_closing() {
            return;
        }
        let flush = {
            let mut live = tab.live.lock().unwrap();
            if live.descriptor.state != TabState::Running {
                return;
            }
            let damage = live.screen.process(bytes);
            if let Some(diff) = damage.diff {
                live.enqueue_remote_diff(&live.descriptor.id.clone(), diff);
            }
            if let Some(title) = damage.title {
                live.descriptor.title = title.clone();
                live.enqueue_control_all(TabEvent::Title(title));
            }
            if damage.bell {
                live.enqueue_control_all(TabEvent::Bell);
            }
            live.queue_replies(damage.replies)
        };
        if let Some(pty_id) = flush {
            flush_replies(&registry, &tab, pty_id);
        }
    }

    fn preparing_exit(&self, _pty_id: u32) {
        if let Some(tab) = self.tab.upgrade() {
            tab.raw.prepare_exit();
        }
    }

    fn exited(&self, pty_id: u32, code: Option<u32>, signal: Option<&str>) {
        let (Some(registry), Some(tab)) = (self.registry.upgrade(), self.tab.upgrade()) else {
            return;
        };
        tab.raw.close();
        let _output_order = tab.raw.send_order.lock().unwrap();
        {
            let mut live = tab.live.lock().unwrap();
            live.pty = PtyBinding::Exited;
            live.pending_replies.clear();
            live.mark_exited(code, signal.map(str::to_owned), false);
        }
        registry.maps.lock().unwrap().by_pty.remove(&pty_id);
    }
}

fn flush_replies(registry: &RegistryInner, tab: &Arc<TabCell>, pty_id: u32) {
    loop {
        let reply = {
            let mut live = tab.live.lock().unwrap();
            if live.descriptor.state != TabState::Running
                || live.pty != PtyBinding::Flushing(pty_id)
            {
                return;
            }
            match live.pending_replies.pop_front() {
                Some(reply) => reply,
                None => {
                    live.pty = PtyBinding::Ready(pty_id);
                    return;
                }
            }
        };
        if registry.backend.write(pty_id, &reply).is_err() {
            let mut live = tab.live.lock().unwrap();
            if live.pty == PtyBinding::Flushing(pty_id) {
                live.pty = PtyBinding::Ready(pty_id);
            }
            return;
        }
    }
}

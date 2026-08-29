//! Fan pty traffic out to remote subscribers.
//!
//! The broker sits between the pty reader threads and the gateway. It exists to
//! keep three things true at once:
//!
//! * Android never learns a pty id. Streams are named by 128 random bits, so a
//!   compromised or buggy client cannot address a terminal it was never given.
//! * A client that reconnects gets its terminal back, not a blank screen. Each
//!   stream keeps the last mebibyte of output with a sequence number per chunk,
//!   which is enough to replay a dropped connection and cheap enough to hold
//!   for every attached terminal.
//! * Two clients never interleave keystrokes. Exactly one subscriber owns input
//!   per stream; everyone else watches until they explicitly take it.
//!
//! Nothing here touches Tauri. The pty side arrives through [`PtyObserver`] and
//! leaves through [`PtyControl`], both of which are plain traits, so the whole
//! broker is testable without a window, a runtime or a spawned shell.

use super::model::TerminalSize;
use crate::pty::PtyObserver;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

/// Per-stream replay window. The spec fixes this at 1 MiB: enough to redraw a
/// full-screen TUI plus scrollback after a tunnel drops, small enough that a
/// dozen attached terminals cost less than a single tab's xterm.js buffer.
pub const REPLAY_CAPACITY: usize = 1024 * 1024;

/// Events a subscriber may fall behind by before it is dropped.
///
/// Dropping is the right answer rather than growing the queue: a client that
/// stopped reading is either gone or wedged, and the replay buffer already
/// covers the case where it comes back. An unbounded queue would let one dead
/// phone hold every byte a busy terminal ever produced.
const EVENT_QUEUE_DEPTH: usize = 512;

/// How the broker reaches a pty. Implemented over the real pty table by
/// `pty::AppPtyControl`, and by a recorder in the tests.
pub trait PtyControl: Send + Sync {
    fn write(&self, pty_id: u32, data: &[u8]) -> Result<(), String>;
    fn resize(&self, pty_id: u32, cols: u16, rows: u16) -> Result<(), String>;
}

fn opaque_id() -> String {
    let mut raw = [0u8; 16];
    OsRng.fill_bytes(&mut raw);
    URL_SAFE_NO_PAD.encode(raw)
}

/// A terminal's name on the wire. Deliberately unrelated to the pty id: the
/// mapping lives only in [`TerminalBroker`], so nothing a client says can be
/// turned into a pty id it was not handed a stream for.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StreamId(String);

impl StreamId {
    fn random() -> Self {
        Self(opaque_id())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for StreamId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// One attachment to one stream. Two tabs on the same phone are two
/// subscribers, because focus is a property of the attachment and not of the
/// device that made it.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SubscriberId(String);

impl SubscriberId {
    fn random() -> Self {
        Self(opaque_id())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SubscriberId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// What a client is handed when it attaches.
///
/// The two variants are not a detail the client can ignore. A `Delta` continues
/// what it already has on screen; a `Snapshot` means the replay window rolled
/// past the point it acknowledged, so it must reset its emulator and draw the
/// snapshot from scratch. Appending a snapshot as though it were a delta
/// duplicates a screenful of output and leaves the emulator's state wrong.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Replay {
    Snapshot { bytes: Vec<u8>, sequence: u64 },
    Delta { bytes: Vec<u8>, sequence: u64 },
}

impl Replay {
    /// The sequence of the last chunk included, which is what the client
    /// acknowledges on its next reconnect.
    pub fn sequence(&self) -> u64 {
        match self {
            Replay::Snapshot { sequence, .. } | Replay::Delta { sequence, .. } => *sequence,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Attachment {
    stream: StreamId,
    subscriber: SubscriberId,
    replay: Replay,
    has_focus: bool,
}

impl Attachment {
    pub fn stream(&self) -> &StreamId {
        &self.stream
    }

    pub fn subscriber(&self) -> &SubscriberId {
        &self.subscriber
    }

    pub fn replay(&self) -> &Replay {
        &self.replay
    }

    pub fn has_focus(&self) -> bool {
        self.has_focus
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalEvent {
    Output {
        sequence: u64,
        bytes: Vec<u8>,
    },
    Exited {
        code: Option<u32>,
        signal: Option<String>,
    },
    /// Sent to every subscriber of the stream, the new owner included: a client
    /// that took focus and a client that lost it need the same update, and
    /// telling only the losers leaves the winner guessing whether it worked.
    FocusChanged {
        owner: Option<SubscriberId>,
    },
}

/// The receiving half of one attachment. Dropping it detaches that subscriber
/// the next time the stream produces anything.
#[derive(Debug)]
pub struct TerminalEvents {
    events: mpsc::Receiver<TerminalEvent>,
}

impl TerminalEvents {
    pub async fn next(&mut self) -> Option<TerminalEvent> {
        self.events.recv().await
    }

    pub fn try_next(&mut self) -> Option<TerminalEvent> {
        self.events.try_recv().ok()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TerminalError {
    code: &'static str,
    message: String,
}

impl TerminalError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    fn unknown_stream() -> Self {
        Self::new("terminal.unknown_stream", "no such terminal stream")
    }

    fn unknown_subscriber() -> Self {
        Self::new("terminal.unknown_subscriber", "not attached to this stream")
    }

    fn input_not_owned() -> Self {
        Self::new(
            "terminal.input_not_owned",
            "another client holds input for this terminal",
        )
    }

    pub fn code(&self) -> &'static str {
        self.code
    }
}

impl fmt::Display for TerminalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for TerminalError {}

struct Chunk {
    sequence: u64,
    bytes: Vec<u8>,
    /// True when the front of this chunk was cut away to fit the window, which
    /// disqualifies it from starting a delta — see [`ReplayBuffer::replay`].
    partial: bool,
}

/// The per-stream ring: whole output chunks, newest last, trimmed from the
/// front once they exceed [`REPLAY_CAPACITY`].
#[derive(Default)]
struct ReplayBuffer {
    chunks: VecDeque<Chunk>,
    buffered: usize,
    last_sequence: u64,
}

impl ReplayBuffer {
    fn push(&mut self, bytes: &[u8]) -> u64 {
        self.last_sequence += 1;
        self.chunks.push_back(Chunk {
            sequence: self.last_sequence,
            bytes: bytes.to_vec(),
            partial: false,
        });
        self.buffered += bytes.len();
        while self.buffered > REPLAY_CAPACITY && self.chunks.len() > 1 {
            if let Some(dropped) = self.chunks.pop_front() {
                self.buffered -= dropped.bytes.len();
            }
        }
        // A single write larger than the whole window keeps only its tail. That
        // costs the chunk its delta eligibility but keeps the memory bound
        // absolute, which matters more: the bound is the only thing standing
        // between a `cat` of a large file and unbounded growth per stream.
        if self.buffered > REPLAY_CAPACITY {
            if let Some(only) = self.chunks.front_mut() {
                let excess = self.buffered - REPLAY_CAPACITY;
                only.bytes.drain(..excess);
                only.partial = true;
                self.buffered = REPLAY_CAPACITY;
            }
        }
        self.last_sequence
    }

    fn snapshot(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.buffered);
        for chunk in &self.chunks {
            bytes.extend_from_slice(&chunk.bytes);
        }
        bytes
    }

    /// The lowest acknowledged sequence a delta can still be built from: one
    /// below the oldest whole chunk, or the oldest chunk itself when its front
    /// was trimmed and a client resuming from before it would be sent output
    /// with a hole in the middle.
    fn earliest_serviceable(&self) -> u64 {
        match self.chunks.front() {
            Some(first) if first.partial => first.sequence,
            Some(first) => first.sequence - 1,
            None => self.last_sequence,
        }
    }

    fn replay(&self, since: Option<u64>) -> Replay {
        // A client resuming from a sequence we never issued is as lost as one
        // whose window rolled — it is reset rather than argued with.
        let resumable = since
            .filter(|acknowledged| *acknowledged <= self.last_sequence)
            .filter(|acknowledged| *acknowledged >= self.earliest_serviceable());
        let Some(acknowledged) = resumable else {
            return Replay::Snapshot {
                bytes: self.snapshot(),
                sequence: self.last_sequence,
            };
        };
        let mut bytes = Vec::new();
        for chunk in self.chunks.iter().filter(|c| c.sequence > acknowledged) {
            bytes.extend_from_slice(&chunk.bytes);
        }
        Replay::Delta {
            bytes,
            sequence: self.last_sequence,
        }
    }
}

struct Subscriber {
    id: SubscriberId,
    events: mpsc::Sender<TerminalEvent>,
}

struct Stream {
    pty_id: u32,
    buffer: ReplayBuffer,
    subscribers: Vec<Subscriber>,
    owner: Option<SubscriberId>,
}

impl Stream {
    /// Deliver to everyone still listening and prune everyone who is not.
    /// Returns the subscribers that were dropped.
    fn fan_out(&mut self, event: &TerminalEvent) -> Vec<SubscriberId> {
        let mut dropped = Vec::new();
        self.subscribers.retain(|subscriber| {
            if subscriber.events.try_send(event.clone()).is_ok() {
                return true;
            }
            dropped.push(subscriber.id.clone());
            false
        });
        dropped
    }

    fn broadcast(&mut self, event: TerminalEvent) {
        let dropped = self.fan_out(&event);
        if self
            .owner
            .as_ref()
            .is_some_and(|owner| dropped.contains(owner))
        {
            self.owner = None;
            let released = TerminalEvent::FocusChanged { owner: None };
            self.fan_out(&released);
        }
    }

    fn require_owner(&self, subscriber: &SubscriberId) -> Result<u32, TerminalError> {
        if !self.subscribers.iter().any(|s| &s.id == subscriber) {
            return Err(TerminalError::unknown_subscriber());
        }
        if self.owner.as_ref() != Some(subscriber) {
            return Err(TerminalError::input_not_owned());
        }
        Ok(self.pty_id)
    }
}

#[derive(Default)]
struct BrokerState {
    streams: HashMap<StreamId, Stream>,
    by_pty: HashMap<u32, StreamId>,
}

pub struct TerminalBroker {
    control: Arc<dyn PtyControl>,
    state: Mutex<BrokerState>,
}

impl TerminalBroker {
    pub fn new(control: Arc<dyn PtyControl>) -> Self {
        Self {
            control,
            state: Mutex::new(BrokerState::default()),
        }
    }

    /// Start (or find) the stream for a pty.
    ///
    /// Streams are created on demand rather than for every spawned pty: a
    /// stream costs up to a mebibyte of replay, and a desktop with twenty tabs
    /// open and no phone attached should pay nothing at all.
    pub fn open_stream(&self, pty_id: u32) -> StreamId {
        let mut state = self.lock();
        if let Some(existing) = state.by_pty.get(&pty_id) {
            return existing.clone();
        }
        let id = StreamId::random();
        state.streams.insert(
            id.clone(),
            Stream {
                pty_id,
                buffer: ReplayBuffer::default(),
                subscribers: Vec::new(),
                owner: None,
            },
        );
        state.by_pty.insert(pty_id, id.clone());
        id
    }

    /// Forget a stream and its replay buffer. Attached clients see their event
    /// channels close.
    pub fn close_stream(&self, stream: &StreamId) {
        let mut state = self.lock();
        if let Some(closed) = state.streams.remove(stream) {
            state.by_pty.remove(&closed.pty_id);
        }
    }

    /// Attach to a stream, resuming after `since` when the client has output it
    /// already drew. `None` asks for a full snapshot.
    pub fn attach(
        &self,
        stream: &StreamId,
        since: Option<u64>,
    ) -> Result<(Attachment, TerminalEvents), TerminalError> {
        let mut state = self.lock();
        let stream_entry = state
            .streams
            .get_mut(stream)
            .ok_or_else(TerminalError::unknown_stream)?;
        let replay = stream_entry.buffer.replay(since);
        let subscriber = SubscriberId::random();
        let (sender, receiver) = mpsc::channel(EVENT_QUEUE_DEPTH);
        stream_entry.subscribers.push(Subscriber {
            id: subscriber.clone(),
            events: sender,
        });
        // Unowned input goes to whoever attaches next, which is what makes the
        // first client an owner without a special case. It also means a
        // terminal whose owner left is picked up by the next arrival rather
        // than sitting untypeable until someone thinks to take focus.
        let has_focus = if stream_entry.owner.is_none() {
            stream_entry.owner = Some(subscriber.clone());
            true
        } else {
            false
        };
        Ok((
            Attachment {
                stream: stream.clone(),
                subscriber,
                replay,
                has_focus,
            },
            TerminalEvents { events: receiver },
        ))
    }

    pub fn detach(
        &self,
        stream: &StreamId,
        subscriber: &SubscriberId,
    ) -> Result<(), TerminalError> {
        let mut state = self.lock();
        let stream_entry = state
            .streams
            .get_mut(stream)
            .ok_or_else(TerminalError::unknown_stream)?;
        let before = stream_entry.subscribers.len();
        stream_entry.subscribers.retain(|s| &s.id != subscriber);
        if stream_entry.subscribers.len() == before {
            return Err(TerminalError::unknown_subscriber());
        }
        if stream_entry.owner.as_ref() == Some(subscriber) {
            stream_entry.owner = None;
            stream_entry.broadcast(TerminalEvent::FocusChanged { owner: None });
        }
        Ok(())
    }

    /// Hand input ownership to `subscriber` and tell every attached client.
    pub fn take_focus(
        &self,
        stream: &StreamId,
        subscriber: &SubscriberId,
    ) -> Result<(), TerminalError> {
        let mut state = self.lock();
        let stream_entry = state
            .streams
            .get_mut(stream)
            .ok_or_else(TerminalError::unknown_stream)?;
        if !stream_entry.subscribers.iter().any(|s| &s.id == subscriber) {
            return Err(TerminalError::unknown_subscriber());
        }
        stream_entry.owner = Some(subscriber.clone());
        stream_entry.broadcast(TerminalEvent::FocusChanged {
            owner: Some(subscriber.clone()),
        });
        Ok(())
    }

    pub fn owner(&self, stream: &StreamId) -> Option<SubscriberId> {
        self.lock().streams.get(stream)?.owner.clone()
    }

    pub fn input(
        &self,
        stream: &StreamId,
        subscriber: &SubscriberId,
        data: &[u8],
    ) -> Result<(), TerminalError> {
        let pty_id = self.owned_pty(stream, subscriber)?;
        // The write happens outside the broker lock: a pty write can block on a
        // full kernel buffer, and holding the map through that would stall
        // every other stream's output fan-out behind one stuck terminal. The
        // window this opens — focus changing between the check and the write —
        // is a keystroke wide and resolves the same way it does on a desktop
        // where two hands reach for the keyboard at once.
        self.control
            .write(pty_id, data)
            // The pty's own error text only: input bytes must never reach a
            // message that could be logged or shown.
            .map_err(|error| TerminalError::new("terminal.write_failed", error))
    }

    /// Resize is owned exactly as input is. A read-only client reshaping the
    /// terminal would reflow the owner's screen under them, and the client that
    /// is typing is the one that knows what size it is drawing at.
    pub fn resize(
        &self,
        stream: &StreamId,
        subscriber: &SubscriberId,
        size: TerminalSize,
    ) -> Result<(), TerminalError> {
        let pty_id = self.owned_pty(stream, subscriber)?;
        self.control
            .resize(pty_id, size.cols(), size.rows())
            .map_err(|error| TerminalError::new("terminal.resize_failed", error))
    }

    fn owned_pty(
        &self,
        stream: &StreamId,
        subscriber: &SubscriberId,
    ) -> Result<u32, TerminalError> {
        self.lock()
            .streams
            .get(stream)
            .ok_or_else(TerminalError::unknown_stream)?
            .require_owner(subscriber)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BrokerState> {
        // A poisoned lock here would mean a panic inside a fan-out. The state
        // it guards is a map of buffers and channels, none of which a panic can
        // leave half-written, so recovering beats taking the app down with it.
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl PtyObserver for TerminalBroker {
    fn on_output(&self, pty_id: u32, bytes: &[u8]) {
        let mut state = self.lock();
        let Some(stream) = state.by_pty.get(&pty_id).cloned() else {
            return;
        };
        let Some(stream_entry) = state.streams.get_mut(&stream) else {
            return;
        };
        let sequence = stream_entry.buffer.push(bytes);
        stream_entry.broadcast(TerminalEvent::Output {
            sequence,
            bytes: bytes.to_vec(),
        });
    }

    fn on_exit(&self, pty_id: u32, code: Option<u32>, signal: Option<&str>) {
        let mut state = self.lock();
        let Some(stream) = state.by_pty.get(&pty_id).cloned() else {
            return;
        };
        let Some(stream_entry) = state.streams.get_mut(&stream) else {
            return;
        };
        // The stream outlives the pty on purpose: a phone that was disconnected
        // when the command finished still has to be able to attach and read
        // what it printed before it died.
        stream_entry.broadcast(TerminalEvent::Exited {
            code,
            signal: signal.map(str::to_string),
        });
    }
}

/// Point the process-wide pty observer at this broker.
pub fn observe_ptys(broker: Arc<TerminalBroker>) {
    crate::pty::set_observer(broker);
}

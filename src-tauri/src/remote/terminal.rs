//! Transport-neutral remote projection of Rust-owned terminal tabs.

use super::model::{RemoteEvent, TerminalSize, PROTOCOL_VERSION};
use crate::tabs::{
    AttachmentId, AttachmentKind, TabAttachment, TabDescriptor, TabError, TabEvent,
    TabEventReceiver, TabExit, TabId, TabLaunch, TabRegistry,
};
use crate::terminal::model::{
    CursorState, Revision, RowPatch, ScreenDiff, ScreenRow, ScreenSnapshot, TerminalModes,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use uuid::Uuid;

pub const MAX_WIRE_FRAME_BYTES: usize = 1024 * 1024;
pub const DIFF_INTERVAL: Duration = Duration::from_millis(16);
const DEFAULT_TRANSFER_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_STAGED_TRANSFER_BYTES: usize = 128 * 1024 * 1024;
const MAX_STAGED_TRANSFER_ROWS: usize = 5_000;

#[derive(Clone, Debug, PartialEq, Eq)]
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

    fn semantic_row_too_large() -> Self {
        Self::new(
            "protocol.semantic_row_too_large",
            "a semantic terminal row cannot fit in one wire frame",
        )
    }

    fn invalid_transfer(message: impl Into<String>) -> Self {
        Self::new("protocol.invalid_transfer", message)
    }

    pub fn code(&self) -> &'static str {
        self.code
    }
}

impl From<TabError> for TerminalError {
    fn from(error: TabError) -> Self {
        Self::new(error.code(), error.to_string())
    }
}

impl fmt::Display for TerminalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for TerminalError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalEvent {
    Snapshot(ScreenSnapshot),
    Diff(ScreenDiff),
    FocusChanged {
        owner: Option<AttachmentId>,
        size: TerminalSize,
    },
    Title(String),
    Bell,
    Exited(TabExit),
}

fn project_event(event: TabEvent) -> Option<TerminalEvent> {
    match event {
        TabEvent::Snapshot(snapshot) => Some(TerminalEvent::Snapshot(snapshot)),
        TabEvent::Diff(diff) => Some(TerminalEvent::Diff(diff)),
        TabEvent::FocusChanged { owner, size } => Some(TerminalEvent::FocusChanged { owner, size }),
        TabEvent::Metadata(_) | TabEvent::Title(_) => None,
        TabEvent::Bell => Some(TerminalEvent::Bell),
        TabEvent::Exited(exit) => Some(TerminalEvent::Exited(exit)),
        TabEvent::Raw(_) => None,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteAttachment {
    tab_id: TabId,
    attachment_id: AttachmentId,
    snapshot: ScreenSnapshot,
    has_focus: bool,
}

impl RemoteAttachment {
    pub fn tab_id(&self) -> &TabId {
        &self.tab_id
    }

    pub fn attachment_id(&self) -> &AttachmentId {
        &self.attachment_id
    }

    pub fn snapshot(&self) -> &ScreenSnapshot {
        &self.snapshot
    }

    pub fn has_focus(&self) -> bool {
        self.has_focus
    }
}

pub struct RemoteTerminalEvents {
    receiver: Arc<Mutex<TabEventReceiver>>,
    registry: Arc<TabRegistry>,
    tab_id: TabId,
    last_title: String,
    coalescer: DiffCoalescer,
    diff_started: Option<Instant>,
}

impl fmt::Debug for RemoteTerminalEvents {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RemoteTerminalEvents")
            .finish_non_exhaustive()
    }
}

impl RemoteTerminalEvents {
    fn new(
        receiver: TabEventReceiver,
        registry: Arc<TabRegistry>,
        tab_id: TabId,
        title: String,
    ) -> Self {
        Self {
            receiver: Arc::new(Mutex::new(receiver)),
            registry,
            tab_id,
            last_title: title,
            coalescer: DiffCoalescer::new(),
            diff_started: None,
        }
    }

    /// Only diffs wait for the 16 ms coalescing deadline. Control events and
    /// recovery snapshots return as soon as the registry publishes them.
    pub async fn next(&mut self) -> Option<TerminalEvent> {
        loop {
            let timeout = self
                .diff_started
                .map(|started| DIFF_INTERVAL.saturating_sub(started.elapsed()));
            if timeout == Some(Duration::ZERO) {
                self.diff_started = None;
                if let Some(diff) = self.coalescer.flush() {
                    return Some(TerminalEvent::Diff(diff));
                }
            }
            let receiver = self.receiver.clone();
            let received = tokio::task::spawn_blocking(move || {
                let receiver = receiver.lock().unwrap();
                match timeout {
                    Some(timeout) => receiver.recv_timeout(timeout).ok(),
                    None => receiver.recv().ok(),
                }
            })
            .await
            .ok()
            .flatten();
            let Some(event) = received else {
                if self.diff_started.take().is_some() {
                    return self.coalescer.flush().map(TerminalEvent::Diff);
                }
                return None;
            };
            match event {
                TabEvent::Diff(diff) => {
                    if self.diff_started.is_none() {
                        self.diff_started = Some(Instant::now());
                    }
                    if self.coalescer.push(diff).is_err() {
                        self.diff_started = None;
                        self.coalescer.clear();
                        return self
                            .registry
                            .snapshot(&self.tab_id)
                            .ok()
                            .map(TerminalEvent::Snapshot);
                    }
                }
                TabEvent::Snapshot(snapshot) => {
                    self.diff_started = None;
                    self.coalescer.clear();
                    return Some(TerminalEvent::Snapshot(snapshot));
                }
                TabEvent::Metadata(descriptor) => {
                    if descriptor.title() != self.last_title {
                        self.last_title = descriptor.title().to_owned();
                        return Some(TerminalEvent::Title(self.last_title.clone()));
                    }
                }
                TabEvent::Title(title) => {
                    if title != self.last_title {
                        self.last_title = title;
                        return Some(TerminalEvent::Title(self.last_title.clone()));
                    }
                }
                other => {
                    if let Some(projected) = project_event(other) {
                        return Some(projected);
                    }
                }
            }
        }
    }
}

#[derive(Clone)]
pub struct RemoteTerminal {
    registry: Arc<TabRegistry>,
}

impl RemoteTerminal {
    pub fn new(registry: Arc<TabRegistry>) -> Self {
        Self { registry }
    }

    pub fn registry(&self) -> &Arc<TabRegistry> {
        &self.registry
    }

    pub fn attach(
        &self,
        tab_id: &TabId,
    ) -> Result<(RemoteAttachment, RemoteTerminalEvents), TerminalError> {
        let TabAttachment { id, events } = self.registry.attach(tab_id, AttachmentKind::Remote)?;
        let snapshot = match events.recv().map_err(|_| {
            TerminalError::new(
                "terminal.attach_failed",
                "initial snapshot was not published",
            )
        })? {
            TabEvent::Snapshot(snapshot) => snapshot,
            _ => {
                return Err(TerminalError::new(
                    "terminal.attach_failed",
                    "initial remote event was not a snapshot",
                ))
            }
        };
        let descriptor = self.registry.get(tab_id)?;
        let has_focus = descriptor.input_owner().is_some_and(|owner| owner == &id);
        Ok((
            RemoteAttachment {
                tab_id: tab_id.clone(),
                attachment_id: id,
                snapshot,
                has_focus,
            },
            RemoteTerminalEvents::new(
                events,
                self.registry.clone(),
                tab_id.clone(),
                descriptor.title().to_owned(),
            ),
        ))
    }

    /// Resume is revision-based. The registry does not retain historical
    /// bytes or diffs, so a non-current revision always receives recovery.
    pub fn resume(
        &self,
        tab_id: &TabId,
        revision: Revision,
    ) -> Result<TerminalEvent, TerminalError> {
        let _ = revision;
        Ok(TerminalEvent::Snapshot(self.registry.snapshot(tab_id)?))
    }

    pub fn list(&self) -> Vec<TabDescriptor> {
        self.registry.list()
    }

    pub fn open(&self, launch: TabLaunch) -> Result<TabId, TerminalError> {
        self.registry.open(launch).map_err(Into::into)
    }

    pub fn close(&self, tab: &TabId) -> Result<(), TerminalError> {
        self.registry.close(tab).map_err(Into::into)
    }

    pub fn input(
        &self,
        tab: &TabId,
        attachment: &AttachmentId,
        bytes: &[u8],
    ) -> Result<(), TerminalError> {
        self.registry
            .input(tab, attachment, bytes)
            .map_err(Into::into)
    }

    pub fn resize(
        &self,
        tab: &TabId,
        attachment: &AttachmentId,
        size: TerminalSize,
    ) -> Result<(), TerminalError> {
        self.registry
            .resize(tab, attachment, size)
            .map_err(Into::into)
    }

    pub fn focus(
        &self,
        tab: &TabId,
        attachment: &AttachmentId,
        size: TerminalSize,
    ) -> Result<(), TerminalError> {
        self.registry
            .take_focus(tab, attachment, size)
            .map_err(Into::into)
    }

    pub fn detach(&self, tab: &TabId, attachment: &AttachmentId) -> Result<(), TerminalError> {
        self.registry.detach(tab, attachment).map_err(Into::into)
    }

    pub fn scrollback(
        &self,
        tab: &TabId,
        offset: usize,
        count: usize,
    ) -> Result<Vec<ScreenRow>, TerminalError> {
        self.registry
            .scrollback(tab, offset, count)
            .map_err(Into::into)
    }
}

#[derive(Default)]
pub struct DiffCoalescer {
    tab_id: Option<String>,
    base_revision: Option<Revision>,
    revision: Option<Revision>,
    rows: BTreeMap<u16, ScreenRow>,
    cursor: Option<CursorState>,
    modes: Option<TerminalModes>,
}

impl DiffCoalescer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn push(&mut self, diff: ScreenDiff) -> Result<(), TerminalError> {
        if let (Some(tab), Some(revision)) = (&self.tab_id, self.revision) {
            if tab != diff.tab_id() || revision != diff.base_revision() {
                self.clear();
                return Err(TerminalError::new(
                    "terminal.revision_gap",
                    "screen damage did not continue the current revision",
                ));
            }
        }
        if self.tab_id.is_none() {
            self.tab_id = Some(diff.tab_id().to_owned());
            self.base_revision = Some(diff.base_revision());
        }
        self.revision = Some(diff.revision());
        for patch in diff.rows() {
            self.rows.insert(patch.row(), patch.content().clone());
        }
        if let Some(cursor) = diff.cursor() {
            self.cursor = Some(cursor.clone());
        }
        if let Some(modes) = diff.modes() {
            self.modes = Some(modes.clone());
        }
        Ok(())
    }

    pub fn flush(&mut self) -> Option<ScreenDiff> {
        let tab_id = self.tab_id.take()?;
        let base = self.base_revision.take()?;
        let revision = self.revision.take()?;
        let rows = std::mem::take(&mut self.rows)
            .into_iter()
            .map(|(row, content)| RowPatch::new(row, content))
            .collect();
        let mut diff = ScreenDiff::for_tab(tab_id, base, revision, rows);
        if let Some(cursor) = self.cursor.take() {
            diff = diff.with_cursor(cursor);
        }
        if let Some(modes) = self.modes.take() {
            diff = diff.with_modes(modes);
        }
        Some(diff)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferKind {
    Snapshot,
    Diff,
    Scrollback,
}

impl TransferKind {
    fn event_kind(self) -> &'static str {
        match self {
            Self::Snapshot => "terminal.snapshot",
            Self::Diff => "terminal.diff",
            Self::Scrollback => "terminal.scrollback",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferChunk {
    pub transfer_id: String,
    pub tab_id: TabId,
    pub attachment_id: Option<AttachmentId>,
    pub kind: TransferKind,
    pub base_revision: Revision,
    pub final_revision: Revision,
    pub row_start: u32,
    pub row_end: u32,
    pub index: u32,
    pub total: u32,
    pub request_id: u64,
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct SnapshotPart {
    cols: u16,
    rows: u16,
    visible: Vec<ScreenRow>,
    cursor: CursorState,
    modes: TerminalModes,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct DiffPart {
    rows: Vec<RowPatch>,
    cursor: Option<CursorState>,
    modes: Option<TerminalModes>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct ScrollbackPart {
    rows: Vec<ScreenRow>,
}

fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, TerminalError> {
    let mut bytes = Vec::new();
    ciborium::into_writer(value, &mut bytes)
        .map_err(|_| TerminalError::invalid_transfer("unable to encode transfer"))?;
    Ok(bytes)
}

fn wire_len(chunk: &TransferChunk) -> Result<usize, TerminalError> {
    let payload = encode(chunk)?;
    encode(&RemoteEvent {
        version: PROTOCOL_VERSION,
        request_id: chunk.request_id,
        kind: chunk.kind.event_kind().to_owned(),
        payload,
    })
    .map(|bytes| bytes.len())
}

fn finish_chunks(mut chunks: Vec<TransferChunk>) -> Result<Vec<TransferChunk>, TerminalError> {
    let total = u32::try_from(chunks.len())
        .map_err(|_| TerminalError::invalid_transfer("too many transfer chunks"))?;
    for chunk in &mut chunks {
        chunk.total = total;
        if wire_len(chunk)? >= MAX_WIRE_FRAME_BYTES {
            return Err(TerminalError::semantic_row_too_large());
        }
    }
    Ok(chunks)
}

fn chunk_rows<T, F>(
    request_id: u64,
    tab: &TabId,
    attachment_id: Option<&AttachmentId>,
    kind: TransferKind,
    base_revision: Revision,
    final_revision: Revision,
    row_count: usize,
    mut encode_range: F,
) -> Result<Vec<TransferChunk>, TerminalError>
where
    F: FnMut(std::ops::Range<usize>) -> Result<T, TerminalError>,
    T: Serialize,
{
    let transfer_id = Uuid::new_v4().to_string();
    let mut chunks = Vec::new();
    let mut start = 0usize;
    if row_count == 0 {
        let payload = encode(&encode_range(0..0)?)?;
        chunks.push(TransferChunk {
            transfer_id,
            tab_id: tab.clone(),
            attachment_id: attachment_id.cloned(),
            kind,
            base_revision,
            final_revision,
            row_start: 0,
            row_end: 0,
            index: 0,
            total: u32::MAX,
            request_id,
            payload,
        });
        return finish_chunks(chunks);
    }
    while start < row_count {
        let mut best: Option<TransferChunk> = None;
        let mut low = start + 1;
        let mut high = row_count;
        while low <= high {
            let end = low + (high - low) / 2;
            let payload = encode(&encode_range(start..end)?)?;
            let candidate = TransferChunk {
                transfer_id: transfer_id.clone(),
                tab_id: tab.clone(),
                attachment_id: attachment_id.cloned(),
                kind,
                base_revision,
                final_revision,
                row_start: u32::try_from(start).unwrap(),
                row_end: u32::try_from(end).unwrap(),
                index: u32::try_from(chunks.len()).unwrap(),
                total: u32::MAX,
                request_id,
                payload,
            };
            if wire_len(&candidate)? >= MAX_WIRE_FRAME_BYTES {
                high = end - 1;
            } else {
                best = Some(candidate);
                low = end + 1;
            }
        }
        let Some(chunk) = best else {
            return Err(TerminalError::semantic_row_too_large());
        };
        start = usize::try_from(chunk.row_end).unwrap();
        chunks.push(chunk);
    }
    finish_chunks(chunks)
}

pub fn chunk_snapshot(
    request_id: u64,
    tab: &TabId,
    snapshot: &ScreenSnapshot,
) -> Result<Vec<TransferChunk>, TerminalError> {
    chunk_snapshot_for_attachment(request_id, tab, None, snapshot)
}

pub fn chunk_snapshot_for_attachment(
    request_id: u64,
    tab: &TabId,
    attachment_id: Option<&AttachmentId>,
    snapshot: &ScreenSnapshot,
) -> Result<Vec<TransferChunk>, TerminalError> {
    let visible = snapshot.visible();
    chunk_rows(
        request_id,
        tab,
        attachment_id,
        TransferKind::Snapshot,
        snapshot.revision(),
        snapshot.revision(),
        visible.len(),
        |range| {
            Ok(SnapshotPart {
                cols: snapshot.cols(),
                rows: snapshot.rows(),
                visible: visible[range].to_vec(),
                cursor: snapshot.cursor().clone(),
                modes: snapshot.modes().clone(),
            })
        },
    )
}

pub fn chunk_diff(
    request_id: u64,
    tab: &TabId,
    diff: &ScreenDiff,
) -> Result<Vec<TransferChunk>, TerminalError> {
    chunk_diff_for_attachment(request_id, tab, None, diff)
}

pub fn chunk_diff_for_attachment(
    request_id: u64,
    tab: &TabId,
    attachment_id: Option<&AttachmentId>,
    diff: &ScreenDiff,
) -> Result<Vec<TransferChunk>, TerminalError> {
    let rows = diff.rows();
    chunk_rows(
        request_id,
        tab,
        attachment_id,
        TransferKind::Diff,
        diff.base_revision(),
        diff.revision(),
        rows.len(),
        |range| {
            Ok(DiffPart {
                rows: rows[range].to_vec(),
                cursor: diff.cursor().cloned(),
                modes: diff.modes().cloned(),
            })
        },
    )
}

pub fn chunk_scrollback(
    request_id: u64,
    tab: &TabId,
    revision: Revision,
    rows: Vec<ScreenRow>,
) -> Result<Vec<TransferChunk>, TerminalError> {
    chunk_scrollback_for_attachment(request_id, tab, None, revision, rows)
}

pub fn chunk_scrollback_for_attachment(
    request_id: u64,
    tab: &TabId,
    attachment_id: Option<&AttachmentId>,
    revision: Revision,
    rows: Vec<ScreenRow>,
) -> Result<Vec<TransferChunk>, TerminalError> {
    chunk_rows(
        request_id,
        tab,
        attachment_id,
        TransferKind::Scrollback,
        revision,
        revision,
        rows.len(),
        |range| {
            Ok(ScrollbackPart {
                rows: rows[range].to_vec(),
            })
        },
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransferPayload {
    Snapshot(ScreenSnapshot),
    Diff(ScreenDiff),
    Scrollback(Vec<ScreenRow>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransferStatus {
    Pending,
    Complete(TransferPayload),
    Recover,
}

struct StagedTransfer {
    transfer_id: String,
    attachment_id: Option<AttachmentId>,
    kind: TransferKind,
    base_revision: Revision,
    final_revision: Revision,
    request_id: u64,
    total: u32,
    next_index: u32,
    next_row: u32,
    bytes: usize,
    rows: usize,
    started: Instant,
    payloads: Vec<Vec<u8>>,
}

pub struct TransferAssembler {
    connection_id: String,
    tab_id: TabId,
    attachment_id: Option<AttachmentId>,
    timeout: Duration,
    revision_floor: Revision,
    staged: Option<StagedTransfer>,
}

impl TransferAssembler {
    pub fn new(connection_id: impl Into<String>, tab_id: TabId) -> Self {
        Self::with_timeout(connection_id, tab_id, DEFAULT_TRANSFER_TIMEOUT)
    }

    pub fn with_timeout(
        connection_id: impl Into<String>,
        tab_id: TabId,
        timeout: Duration,
    ) -> Self {
        Self {
            connection_id: connection_id.into(),
            tab_id,
            attachment_id: None,
            timeout,
            revision_floor: Revision(0),
            staged: None,
        }
    }

    pub fn bind_attachment(mut self, attachment_id: AttachmentId) -> Self {
        self.attachment_id = Some(attachment_id);
        self
    }

    pub fn reset_for_snapshot(&mut self, revision: Revision) {
        self.staged = None;
        self.revision_floor = revision;
    }

    pub fn expire_at(&mut self, now: Instant) -> bool {
        let expired = self
            .staged
            .as_ref()
            .is_some_and(|staged| now.saturating_duration_since(staged.started) >= self.timeout);
        if expired {
            self.staged = None;
        }
        expired
    }

    pub fn accept(&mut self, connection_id: &str, chunk: TransferChunk) -> TransferStatus {
        self.accept_at(connection_id, chunk, Instant::now())
    }

    pub fn accept_at(
        &mut self,
        connection_id: &str,
        chunk: TransferChunk,
        now: Instant,
    ) -> TransferStatus {
        if self.expire_at(now)
            || connection_id != self.connection_id
            || chunk.tab_id != self.tab_id
            || chunk.attachment_id != self.attachment_id
            || chunk.total == 0
            || usize::try_from(chunk.total).map_or(true, |total| total > MAX_STAGED_TRANSFER_ROWS)
            || usize::try_from(chunk.row_end)
                .map_or(true, |row_end| row_end > MAX_STAGED_TRANSFER_ROWS)
            || chunk.final_revision.0 < self.revision_floor.0
            || !valid_revision_metadata(&chunk)
            || wire_len(&chunk).map_or(true, |size| size >= MAX_WIRE_FRAME_BYTES)
        {
            self.staged = None;
            return TransferStatus::Recover;
        }
        if self.staged.is_none() {
            if chunk.index != 0 || chunk.row_start != 0 {
                return TransferStatus::Recover;
            }
            self.staged = Some(StagedTransfer {
                transfer_id: chunk.transfer_id.clone(),
                attachment_id: chunk.attachment_id.clone(),
                kind: chunk.kind,
                base_revision: chunk.base_revision,
                final_revision: chunk.final_revision,
                request_id: chunk.request_id,
                total: chunk.total,
                next_index: 0,
                next_row: 0,
                bytes: 0,
                rows: 0,
                started: now,
                payloads: Vec::new(),
            });
        }
        let staged = self.staged.as_mut().unwrap();
        let row_count = usize::try_from(chunk.row_end.saturating_sub(chunk.row_start)).unwrap();
        let consistent = chunk.transfer_id == staged.transfer_id
            && chunk.attachment_id == staged.attachment_id
            && chunk.kind == staged.kind
            && chunk.base_revision == staged.base_revision
            && chunk.final_revision == staged.final_revision
            && chunk.request_id == staged.request_id
            && chunk.total == staged.total
            && chunk.index == staged.next_index
            && chunk.row_start == staged.next_row
            && chunk.row_end >= chunk.row_start
            && (chunk.row_end > chunk.row_start
                || (chunk.total == 1 && chunk.index == 0 && chunk.row_start == 0))
            && chunk.index < chunk.total;
        let bounded = staged.bytes.saturating_add(chunk.payload.len()) <= MAX_STAGED_TRANSFER_BYTES
            && staged.rows.saturating_add(row_count) <= MAX_STAGED_TRANSFER_ROWS;
        if !consistent || !bounded || validate_part(&chunk, row_count).is_err() {
            self.staged = None;
            return TransferStatus::Recover;
        }
        staged.bytes += chunk.payload.len();
        staged.rows += row_count;
        staged.next_index += 1;
        staged.next_row = chunk.row_end;
        staged.payloads.push(chunk.payload);
        if staged.next_index != staged.total {
            return TransferStatus::Pending;
        }
        let staged = self.staged.take().unwrap();
        match assemble_payload(&self.tab_id, staged) {
            Ok(payload) => {
                self.revision_floor = match &payload {
                    TransferPayload::Snapshot(snapshot) => snapshot.revision(),
                    TransferPayload::Diff(diff) => diff.revision(),
                    TransferPayload::Scrollback(_) => self.revision_floor,
                };
                TransferStatus::Complete(payload)
            }
            Err(_) => TransferStatus::Recover,
        }
    }
}

fn decode<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, TerminalError> {
    ciborium::from_reader(bytes)
        .map_err(|_| TerminalError::invalid_transfer("invalid transfer payload"))
}

fn validate_part(chunk: &TransferChunk, expected_rows: usize) -> Result<(), TerminalError> {
    let rows = match chunk.kind {
        TransferKind::Snapshot => decode::<SnapshotPart>(&chunk.payload)?.visible,
        TransferKind::Diff => decode::<DiffPart>(&chunk.payload)?
            .rows
            .into_iter()
            .map(|patch| patch.content().clone())
            .collect(),
        TransferKind::Scrollback => decode::<ScrollbackPart>(&chunk.payload)?.rows,
    };
    if rows.len() != expected_rows || rows.iter().any(|row| !valid_row(row)) {
        return Err(TerminalError::invalid_transfer(
            "row range does not match payload",
        ));
    }
    Ok(())
}

fn valid_revision_metadata(chunk: &TransferChunk) -> bool {
    match chunk.kind {
        TransferKind::Snapshot | TransferKind::Scrollback => {
            chunk.base_revision == chunk.final_revision
        }
        TransferKind::Diff => chunk.base_revision.0 < chunk.final_revision.0,
    }
}

fn valid_row(row: &ScreenRow) -> bool {
    row.cells().len() <= 512
        && row
            .cells()
            .iter()
            .all(|cell| cell.is_continuation() || matches!(cell.text().chars().count(), 1..=33))
}

fn assemble_payload(
    tab_id: &TabId,
    staged: StagedTransfer,
) -> Result<TransferPayload, TerminalError> {
    match staged.kind {
        TransferKind::Snapshot => {
            let mut visible = Vec::new();
            let mut header: Option<(u16, u16, CursorState, TerminalModes)> = None;
            for payload in staged.payloads {
                let part: SnapshotPart = decode(&payload)?;
                let current = (
                    part.cols,
                    part.rows,
                    part.cursor.clone(),
                    part.modes.clone(),
                );
                if header.as_ref().is_some_and(|header| header != &current) {
                    return Err(TerminalError::invalid_transfer("mixed snapshot metadata"));
                }
                header = Some(current);
                visible.extend(part.visible);
            }
            let (cols, rows, cursor, modes) = header.ok_or_else(|| {
                TerminalError::invalid_transfer("snapshot transfer has no payload")
            })?;
            if visible.len() != usize::from(rows) {
                return Err(TerminalError::invalid_transfer(
                    "snapshot does not contain the complete viewport",
                ));
            }
            if visible
                .iter()
                .any(|row| row.cells().len() != usize::from(cols))
            {
                return Err(TerminalError::invalid_transfer(
                    "snapshot rows do not match its column count",
                ));
            }
            let size = TerminalSize::try_new(cols, rows)
                .map_err(|error| TerminalError::new(error.code(), "invalid snapshot dimensions"))?;
            Ok(TransferPayload::Snapshot(ScreenSnapshot::new(
                tab_id.as_str(),
                staged.final_revision,
                size,
                visible,
                Vec::new(),
                cursor,
                modes,
            )))
        }
        TransferKind::Diff => {
            let mut rows = Vec::new();
            let mut cursor = None;
            let mut modes = None;
            for payload in staged.payloads {
                let part: DiffPart = decode(&payload)?;
                rows.extend(part.rows);
                cursor = part.cursor;
                modes = part.modes;
            }
            let mut patched = std::collections::HashSet::new();
            if rows.iter().any(|patch| !patched.insert(patch.row())) {
                return Err(TerminalError::invalid_transfer(
                    "diff contains duplicate row patches",
                ));
            }
            let mut diff = ScreenDiff::for_tab(
                tab_id.as_str(),
                staged.base_revision,
                staged.final_revision,
                rows,
            );
            if let Some(value) = cursor {
                diff = diff.with_cursor(value);
            }
            if let Some(value) = modes {
                diff = diff.with_modes(value);
            }
            Ok(TransferPayload::Diff(diff))
        }
        TransferKind::Scrollback => {
            let mut rows = Vec::new();
            for payload in staged.payloads {
                rows.extend(decode::<ScrollbackPart>(&payload)?.rows);
            }
            Ok(TransferPayload::Scrollback(rows))
        }
    }
}

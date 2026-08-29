//! Transport-neutral remote projection of Rust-owned terminal tabs.

use super::model::{
    decode_exact, encode_terminal_frame, RemoteEvent, TerminalSize, PROTOCOL_VERSION,
};
use crate::tabs::{
    AttachmentId, AttachmentKind, RecoveryBoundary, TabAttachment, TabAttachmentCancellation,
    TabDescriptor, TabError, TabEvent, TabEventReceiver, TabExit, TabId, TabLaunch,
    TabReceiveError, TabRegistry,
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
const MAX_STAGED_TRANSFER_BYTES: usize = 64 * 1024 * 1024;

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
    title: String,
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

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn into_snapshot(self) -> ScreenSnapshot {
        self.snapshot
    }
}

pub struct RemoteTerminalEvents {
    receiver: TabEventReceiver,
    cancellation: TabAttachmentCancellation,
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
        cancellation: TabAttachmentCancellation,
        registry: Arc<TabRegistry>,
        tab_id: TabId,
        title: String,
    ) -> Self {
        Self {
            receiver,
            cancellation,
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
            let received = match timeout {
                Some(timeout) => {
                    match tokio::time::timeout(timeout, self.receiver.recv_async()).await {
                        Ok(result) => result,
                        Err(_) => continue,
                    }
                }
                None => self.receiver.recv_async().await,
            };
            let event = match received {
                Ok(event) => event,
                Err(TabReceiveError::Cancelled) => {
                    self.diff_started = None;
                    self.coalescer.clear();
                    return None;
                }
                Err(TabReceiveError::Disconnected) => {
                    if self.diff_started.take().is_some() {
                        return self.coalescer.flush().map(TerminalEvent::Diff);
                    }
                    return None;
                }
            };
            match event {
                TabEvent::Diff(diff) => {
                    if self.diff_started.is_none() {
                        self.diff_started = Some(Instant::now());
                    }
                    if self.coalescer.push(diff).is_err() {
                        self.diff_started = None;
                        self.coalescer.clear();
                        let registry = self.registry.clone();
                        let tab_id = self.tab_id.clone();
                        return tokio::task::spawn_blocking(move || registry.snapshot(&tab_id))
                            .await
                            .ok()
                            .and_then(Result::ok)
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

    pub fn cancellation(&self) -> TabAttachmentCancellation {
        self.cancellation.clone()
    }

    pub fn cancel(&self) {
        self.cancellation.cancel();
    }

    pub fn apply_recovery_boundary(&mut self, boundary: RecoveryBoundary) {
        self.diff_started = None;
        self.coalescer.clear();
        self.receiver.discard_before(boundary);
    }
}

impl Drop for RemoteTerminalEvents {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

#[derive(Clone)]
pub struct RemoteTerminal {
    registry: Arc<TabRegistry>,
}

pub struct RemoteRecovery {
    snapshot: ScreenSnapshot,
    boundary: RecoveryBoundary,
}

impl RemoteRecovery {
    pub fn snapshot(&self) -> &ScreenSnapshot {
        &self.snapshot
    }

    pub fn into_parts(self) -> (ScreenSnapshot, RecoveryBoundary) {
        (self.snapshot, self.boundary)
    }
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
        let attachment = self.registry.attach(tab_id, AttachmentKind::Remote)?;
        let descriptor = attachment.descriptor().clone();
        let TabAttachment {
            id,
            events,
            cancellation,
            ..
        } = attachment;
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
        let has_focus = descriptor.input_owner().is_some_and(|owner| owner == &id);
        Ok((
            RemoteAttachment {
                tab_id: tab_id.clone(),
                attachment_id: id,
                snapshot,
                has_focus,
                title: descriptor.title().to_owned(),
            },
            RemoteTerminalEvents::new(
                events,
                cancellation,
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
        attachment_id: &AttachmentId,
        revision: Revision,
    ) -> Result<RemoteRecovery, TerminalError> {
        let _ = revision;
        let (snapshot, boundary) = self.registry.recovery_snapshot(tab_id, attachment_id)?;
        Ok(RemoteRecovery { snapshot, boundary })
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
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
struct SnapshotPart {
    cols: u16,
    rows: u16,
    visible: Vec<ScreenRow>,
    cursor: CursorState,
    modes: TerminalModes,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DiffPart {
    rows: Vec<RowPatch>,
    cursor: Option<CursorState>,
    modes: Option<TerminalModes>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ScrollbackPart {
    rows: Vec<ScreenRow>,
}

#[derive(Serialize)]
struct SnapshotPartRef<'a> {
    cols: u16,
    rows: u16,
    visible: &'a [ScreenRow],
    cursor: &'a CursorState,
    modes: &'a TerminalModes,
}

#[derive(Serialize)]
struct DiffPartRef<'a> {
    rows: &'a [RowPatch],
    cursor: Option<&'a CursorState>,
    modes: Option<&'a TerminalModes>,
}

#[derive(Serialize)]
struct ScrollbackPartRef<'a> {
    rows: &'a [ScreenRow],
}

fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, TerminalError> {
    encode_terminal_frame(value).map_err(|error| {
        if error.code() == "protocol.frame_too_large" {
            TerminalError::semantic_row_too_large()
        } else {
            TerminalError::invalid_transfer("unable to encode transfer")
        }
    })
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

#[derive(Clone)]
enum TransferSource {
    Snapshot(ScreenSnapshot),
    Diff(ScreenDiff),
    Scrollback(Vec<ScreenRow>),
}

impl TransferSource {
    fn row_count(&self) -> usize {
        match self {
            Self::Snapshot(snapshot) => snapshot.visible().len(),
            Self::Diff(diff) => diff.rows().len(),
            Self::Scrollback(rows) => rows.len(),
        }
    }

    fn encode_range(&self, range: std::ops::Range<usize>) -> Result<Vec<u8>, TerminalError> {
        match self {
            Self::Snapshot(snapshot) => encode(&SnapshotPartRef {
                cols: snapshot.cols(),
                rows: snapshot.rows(),
                visible: &snapshot.visible()[range],
                cursor: snapshot.cursor(),
                modes: snapshot.modes(),
            }),
            Self::Diff(diff) => encode(&DiffPartRef {
                rows: &diff.rows()[range],
                cursor: diff.cursor(),
                modes: diff.modes(),
            }),
            Self::Scrollback(rows) => encode(&ScrollbackPartRef { rows: &rows[range] }),
        }
    }
}

/// An owned canonical payload plus metadata-only row ranges. No encoded chunk
/// or outbound frame is retained; callers encode and send one chunk at a time.
pub struct TransferPlan {
    transfer_id: String,
    tab_id: TabId,
    attachment_id: Option<AttachmentId>,
    kind: TransferKind,
    base_revision: Revision,
    final_revision: Revision,
    request_id: u64,
    source: TransferSource,
    ranges: Vec<std::ops::Range<usize>>,
    next: usize,
}

impl TransferPlan {
    fn new(
        request_id: u64,
        tab: &TabId,
        attachment_id: Option<&AttachmentId>,
        kind: TransferKind,
        base_revision: Revision,
        final_revision: Revision,
        source: TransferSource,
    ) -> Result<Self, TerminalError> {
        if source.row_count() > transfer_row_cap(kind) {
            return Err(TerminalError::invalid_transfer(
                "transfer exceeds the row limit for its kind",
            ));
        }
        let transfer_id = Uuid::new_v4().to_string();
        let ranges = plan_row_ranges(
            request_id,
            tab,
            attachment_id,
            kind,
            base_revision,
            final_revision,
            &transfer_id,
            &source,
        )?;
        Ok(Self {
            transfer_id,
            tab_id: tab.clone(),
            attachment_id: attachment_id.cloned(),
            kind,
            base_revision,
            final_revision,
            request_id,
            source,
            ranges,
            next: 0,
        })
    }

    pub fn next_chunk(&mut self) -> Result<Option<TransferChunk>, TerminalError> {
        let Some(range) = self.ranges.get(self.next).cloned() else {
            return Ok(None);
        };
        let payload = self.source.encode_range(range.clone())?;
        let chunk = TransferChunk {
            transfer_id: self.transfer_id.clone(),
            tab_id: self.tab_id.clone(),
            attachment_id: self.attachment_id.clone(),
            kind: self.kind,
            base_revision: self.base_revision,
            final_revision: self.final_revision,
            row_start: u32::try_from(range.start)
                .map_err(|_| TerminalError::invalid_transfer("row index exceeds protocol"))?,
            row_end: u32::try_from(range.end)
                .map_err(|_| TerminalError::invalid_transfer("row index exceeds protocol"))?,
            index: u32::try_from(self.next)
                .map_err(|_| TerminalError::invalid_transfer("too many transfer chunks"))?,
            total: u32::try_from(self.ranges.len())
                .map_err(|_| TerminalError::invalid_transfer("too many transfer chunks"))?,
            request_id: self.request_id,
            payload,
        };
        self.next += 1;
        Ok(Some(chunk))
    }

    pub fn request_id(&self) -> u64 {
        self.request_id
    }

    fn collect_chunks(mut self) -> Result<Vec<TransferChunk>, TerminalError> {
        let mut chunks = Vec::with_capacity(self.ranges.len());
        while let Some(chunk) = self.next_chunk()? {
            chunks.push(chunk);
        }
        Ok(chunks)
    }
}

fn plan_row_ranges(
    request_id: u64,
    tab: &TabId,
    attachment_id: Option<&AttachmentId>,
    kind: TransferKind,
    base_revision: Revision,
    final_revision: Revision,
    transfer_id: &str,
    source: &TransferSource,
) -> Result<Vec<std::ops::Range<usize>>, TerminalError> {
    let row_count = source.row_count();
    let mut ranges = Vec::new();
    let mut start = 0usize;
    if row_count == 0 {
        let payload = source.encode_range(0..0)?;
        let chunk = TransferChunk {
            transfer_id: transfer_id.to_owned(),
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
        };
        if wire_len(&chunk)? >= MAX_WIRE_FRAME_BYTES {
            return Err(TerminalError::semantic_row_too_large());
        }
        return Ok(vec![0..0]);
    }
    while start < row_count {
        let mut best: Option<usize> = None;
        let mut low = start + 1;
        let mut high = row_count;
        while low <= high {
            let end = low + (high - low) / 2;
            let payload = match source.encode_range(start..end) {
                Ok(payload) => payload,
                Err(error) if error.code() == "protocol.semantic_row_too_large" => {
                    high = end - 1;
                    continue;
                }
                Err(error) => return Err(error),
            };
            let candidate = TransferChunk {
                transfer_id: transfer_id.to_owned(),
                tab_id: tab.clone(),
                attachment_id: attachment_id.cloned(),
                kind,
                base_revision,
                final_revision,
                row_start: u32::try_from(start).unwrap(),
                row_end: u32::try_from(end).unwrap(),
                index: u32::try_from(ranges.len()).unwrap(),
                total: u32::MAX,
                request_id,
                payload,
            };
            match wire_len(&candidate) {
                Ok(size) if size < MAX_WIRE_FRAME_BYTES => {
                    best = Some(end);
                    low = end + 1;
                }
                Ok(_) => high = end - 1,
                Err(error) if error.code() == "protocol.semantic_row_too_large" => {
                    high = end - 1;
                }
                Err(error) => return Err(error),
            }
        }
        let Some(end) = best else {
            return Err(TerminalError::semantic_row_too_large());
        };
        ranges.push(start..end);
        start = end;
    }
    Ok(ranges)
}

pub fn plan_snapshot_for_attachment(
    request_id: u64,
    tab: &TabId,
    attachment_id: Option<&AttachmentId>,
    snapshot: ScreenSnapshot,
) -> Result<TransferPlan, TerminalError> {
    let revision = snapshot.revision();
    TransferPlan::new(
        request_id,
        tab,
        attachment_id,
        TransferKind::Snapshot,
        revision,
        revision,
        TransferSource::Snapshot(snapshot),
    )
}

pub fn plan_diff_for_attachment(
    request_id: u64,
    tab: &TabId,
    attachment_id: Option<&AttachmentId>,
    diff: ScreenDiff,
) -> Result<TransferPlan, TerminalError> {
    let base = diff.base_revision();
    let revision = diff.revision();
    TransferPlan::new(
        request_id,
        tab,
        attachment_id,
        TransferKind::Diff,
        base,
        revision,
        TransferSource::Diff(diff),
    )
}

pub fn plan_scrollback_for_attachment(
    request_id: u64,
    tab: &TabId,
    attachment_id: Option<&AttachmentId>,
    revision: Revision,
    rows: Vec<ScreenRow>,
) -> Result<TransferPlan, TerminalError> {
    TransferPlan::new(
        request_id,
        tab,
        attachment_id,
        TransferKind::Scrollback,
        revision,
        revision,
        TransferSource::Scrollback(rows),
    )
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
    plan_snapshot_for_attachment(request_id, tab, attachment_id, snapshot.clone())?.collect_chunks()
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
    plan_diff_for_attachment(request_id, tab, attachment_id, diff.clone())?.collect_chunks()
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
    plan_scrollback_for_attachment(request_id, tab, attachment_id, revision, rows)?.collect_chunks()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransferPayload {
    Snapshot(ScreenSnapshot),
    Diff(ScreenDiff),
    Scrollback(Vec<ScreenRow>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransferCommitToken(u64);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransferCompletion {
    token: TransferCommitToken,
    payload: TransferPayload,
}

impl TransferCompletion {
    pub fn token(&self) -> TransferCommitToken {
        self.token
    }

    pub fn payload(&self) -> &TransferPayload {
        &self.payload
    }

    pub fn into_payload(self) -> TransferPayload {
        self.payload
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransferStatus {
    Pending,
    Complete(TransferCompletion),
    Recover,
}

#[derive(Default)]
struct TransferBudgetState {
    next_id: u64,
    active: Option<u64>,
}

/// A connection-scoped staging budget. Only one semantic transfer may be
/// staged for a connection at a time, which gives a hard aggregate bound.
#[derive(Clone, Default)]
pub struct TransferBudget(Arc<Mutex<TransferBudgetState>>);

impl TransferBudget {
    pub fn single_active() -> Self {
        Self::default()
    }

    fn reserve(&self) -> Option<TransferReservation> {
        let mut state = self.0.lock().unwrap();
        if state.active.is_some() {
            return None;
        }
        state.next_id = state.next_id.saturating_add(1);
        let id = state.next_id;
        state.active = Some(id);
        Some(TransferReservation {
            budget: self.clone(),
            id,
        })
    }
}

struct TransferReservation {
    budget: TransferBudget,
    id: u64,
}

impl Drop for TransferReservation {
    fn drop(&mut self) {
        let mut state = self.budget.0.lock().unwrap();
        if state.active == Some(self.id) {
            state.active = None;
        }
    }
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
    parts: Vec<ValidatedPart>,
    metadata: Option<PartMetadata>,
    _reservation: TransferReservation,
}

#[derive(Clone, Copy)]
enum PendingApplication {
    Snapshot {
        revision: Revision,
        size: TerminalSize,
    },
    Diff {
        revision: Revision,
    },
    Scrollback,
}

struct PendingCommit {
    token: TransferCommitToken,
    application: PendingApplication,
}

pub struct TransferAssembler {
    connection_id: String,
    tab_id: TabId,
    attachment_id: Option<AttachmentId>,
    timeout: Duration,
    committed_revision: Revision,
    committed_size: Option<TerminalSize>,
    staged: Option<StagedTransfer>,
    pending_commit: Option<PendingCommit>,
    next_commit_token: u64,
    budget: TransferBudget,
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
        Self::with_timeout_and_budget(
            connection_id,
            tab_id,
            timeout,
            TransferBudget::single_active(),
        )
    }

    pub fn with_budget(
        connection_id: impl Into<String>,
        tab_id: TabId,
        budget: TransferBudget,
    ) -> Self {
        Self::with_timeout_and_budget(connection_id, tab_id, DEFAULT_TRANSFER_TIMEOUT, budget)
    }

    pub fn with_timeout_and_budget(
        connection_id: impl Into<String>,
        tab_id: TabId,
        timeout: Duration,
        budget: TransferBudget,
    ) -> Self {
        Self {
            connection_id: connection_id.into(),
            tab_id,
            attachment_id: None,
            timeout,
            committed_revision: Revision(0),
            committed_size: None,
            staged: None,
            pending_commit: None,
            next_commit_token: 0,
            budget,
        }
    }

    pub fn bind_attachment(mut self, attachment_id: AttachmentId) -> Self {
        self.attachment_id = Some(attachment_id);
        self
    }

    pub fn reset_for_snapshot(&mut self, revision: Revision, size: TerminalSize) {
        self.staged = None;
        self.pending_commit = None;
        self.committed_revision = revision;
        self.committed_size = Some(size);
    }

    pub fn committed_revision(&self) -> Revision {
        self.committed_revision
    }

    pub fn committed_size(&self) -> Option<TerminalSize> {
        self.committed_size
    }

    pub fn commit_applied(&mut self, token: TransferCommitToken) -> Result<(), TerminalError> {
        let pending = self.pending_commit.take().ok_or_else(|| {
            TerminalError::invalid_transfer("no completed transfer is awaiting application")
        })?;
        if pending.token != token {
            self.pending_commit = Some(pending);
            return Err(TerminalError::invalid_transfer(
                "transfer commit token does not match",
            ));
        }
        match pending.application {
            PendingApplication::Snapshot { revision, size } => {
                self.committed_revision = revision;
                self.committed_size = Some(size);
            }
            PendingApplication::Diff { revision } => {
                self.committed_revision = revision;
            }
            PendingApplication::Scrollback => {}
        }
        Ok(())
    }

    pub fn reject_applied(&mut self, token: TransferCommitToken) -> Result<(), TerminalError> {
        let pending = self.pending_commit.take().ok_or_else(|| {
            TerminalError::invalid_transfer("no completed transfer is awaiting application")
        })?;
        if pending.token != token {
            self.pending_commit = Some(pending);
            return Err(TerminalError::invalid_transfer(
                "transfer commit token does not match",
            ));
        }
        Ok(())
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

    /// Exposed so a receiver can schedule reclamation even if no later chunk
    /// arrives to drive `accept_at`.
    pub fn deadline(&self) -> Option<Instant> {
        self.staged
            .as_ref()
            .map(|staged| staged.started + self.timeout)
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
        if self.pending_commit.is_some() {
            return TransferStatus::Recover;
        }
        let row_cap = transfer_row_cap(chunk.kind);
        if self.expire_at(now)
            || connection_id != self.connection_id
            || chunk.tab_id != self.tab_id
            || chunk.attachment_id != self.attachment_id
            || chunk.total == 0
            || usize::try_from(chunk.total).map_or(true, |total| total > row_cap.max(1))
            || usize::try_from(chunk.row_end).map_or(true, |row_end| row_end > row_cap)
            || !revision_continues(self.committed_revision, &chunk)
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
            let Some(reservation) = self.budget.reserve() else {
                return TransferStatus::Recover;
            };
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
                parts: Vec::new(),
                metadata: None,
                _reservation: reservation,
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
            && staged.rows.saturating_add(row_count) <= row_cap;
        let validated = validate_part(&chunk, row_count, self.committed_size);
        let metadata_consistent = validated.as_ref().is_ok_and(|(metadata, _)| {
            staged
                .metadata
                .as_ref()
                .is_none_or(|current| current == metadata)
        });
        if !consistent || !bounded || !metadata_consistent {
            self.staged = None;
            return TransferStatus::Recover;
        }
        if staged.metadata.is_none() {
            staged.metadata = validated
                .as_ref()
                .ok()
                .map(|(metadata, _)| metadata.clone());
        }
        let (_, part) = validated.expect("validated transfer part was checked above");
        staged.bytes += chunk.payload.len();
        staged.rows += row_count;
        staged.next_index += 1;
        staged.next_row = chunk.row_end;
        staged.parts.push(part);
        if staged.next_index != staged.total {
            return TransferStatus::Pending;
        }
        let staged = self.staged.take().unwrap();
        match assemble_payload(&self.tab_id, staged) {
            Ok(payload) => {
                self.next_commit_token = self.next_commit_token.saturating_add(1);
                let token = TransferCommitToken(self.next_commit_token);
                let application = match &payload {
                    TransferPayload::Snapshot(snapshot) => PendingApplication::Snapshot {
                        revision: snapshot.revision(),
                        size: TerminalSize::try_new(snapshot.cols(), snapshot.rows())
                            .expect("assembled snapshot dimensions were validated"),
                    },
                    TransferPayload::Diff(diff) => PendingApplication::Diff {
                        revision: diff.revision(),
                    },
                    TransferPayload::Scrollback(_) => PendingApplication::Scrollback,
                };
                self.pending_commit = Some(PendingCommit { token, application });
                TransferStatus::Complete(TransferCompletion { token, payload })
            }
            Err(_) => TransferStatus::Recover,
        }
    }
}

fn transfer_row_cap(kind: TransferKind) -> usize {
    match kind {
        TransferKind::Snapshot | TransferKind::Diff => 512,
        TransferKind::Scrollback => 256,
    }
}

fn decode<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, TerminalError> {
    decode_exact(bytes).map_err(|_| TerminalError::invalid_transfer("invalid transfer payload"))
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PartMetadata {
    Snapshot(u16, u16, CursorState, TerminalModes),
    Diff(Option<CursorState>, Option<TerminalModes>),
    Scrollback,
}

enum ValidatedPart {
    Snapshot(SnapshotPart),
    Diff(DiffPart),
    Scrollback(ScrollbackPart),
}

fn validate_part(
    chunk: &TransferChunk,
    expected_rows: usize,
    committed_size: Option<TerminalSize>,
) -> Result<(PartMetadata, ValidatedPart), TerminalError> {
    let validated = match chunk.kind {
        TransferKind::Snapshot => {
            let part = decode::<SnapshotPart>(&chunk.payload)?;
            if part.visible.len() != expected_rows
                || !(1..=512).contains(&part.cols)
                || !(1..=512).contains(&part.rows)
                || part.cursor.col() >= part.cols
                || part.cursor.row() >= part.rows
                || part
                    .visible
                    .iter()
                    .any(|row| row.cells().len() > usize::from(part.cols) || !valid_row(row))
            {
                return Err(TerminalError::invalid_transfer(
                    "snapshot part exceeds canonical bounds",
                ));
            }
            let metadata = PartMetadata::Snapshot(
                part.cols,
                part.rows,
                part.cursor.clone(),
                part.modes.clone(),
            );
            (metadata, ValidatedPart::Snapshot(part))
        }
        TransferKind::Diff => {
            let part = decode::<DiffPart>(&chunk.payload)?;
            let Some(size) = committed_size else {
                return Err(TerminalError::invalid_transfer(
                    "diff arrived before an applied snapshot",
                ));
            };
            if part.rows.len() != expected_rows
                || part.rows.iter().any(|patch| {
                    patch.row() >= size.rows()
                        || patch.content().cells().len() > usize::from(size.cols())
                        || !valid_row(patch.content())
                })
                || part.cursor.as_ref().is_some_and(|cursor| {
                    cursor.col() >= size.cols() || cursor.row() >= size.rows()
                })
            {
                return Err(TerminalError::invalid_transfer(
                    "diff part exceeds canonical bounds",
                ));
            }
            let metadata = PartMetadata::Diff(part.cursor.clone(), part.modes.clone());
            (metadata, ValidatedPart::Diff(part))
        }
        TransferKind::Scrollback => {
            let part = decode::<ScrollbackPart>(&chunk.payload)?;
            if part.rows.len() != expected_rows || part.rows.iter().any(|row| !valid_row(row)) {
                return Err(TerminalError::invalid_transfer(
                    "scrollback row range does not match payload",
                ));
            }
            (PartMetadata::Scrollback, ValidatedPart::Scrollback(part))
        }
    };
    Ok(validated)
}

fn valid_revision_metadata(chunk: &TransferChunk) -> bool {
    match chunk.kind {
        TransferKind::Snapshot | TransferKind::Scrollback => {
            chunk.base_revision == chunk.final_revision
        }
        TransferKind::Diff => chunk.base_revision.0 < chunk.final_revision.0,
    }
}

fn revision_continues(current: Revision, chunk: &TransferChunk) -> bool {
    match chunk.kind {
        TransferKind::Snapshot => chunk.final_revision.0 >= current.0,
        TransferKind::Diff => chunk.base_revision == current,
        // History pages are independent of the applied live viewport. Their
        // atomic registry revision is descriptive and never advances it.
        TransferKind::Scrollback => true,
    }
}

fn valid_row(row: &ScreenRow) -> bool {
    row.cells().len() <= 512 && row.has_valid_cell_text()
}

fn assemble_payload(
    tab_id: &TabId,
    staged: StagedTransfer,
) -> Result<TransferPayload, TerminalError> {
    match staged.kind {
        TransferKind::Snapshot => {
            let mut visible = Vec::new();
            let mut header: Option<(u16, u16, CursorState, TerminalModes)> = None;
            for part in staged.parts {
                let ValidatedPart::Snapshot(part) = part else {
                    return Err(TerminalError::invalid_transfer(
                        "mixed transfer payload kinds",
                    ));
                };
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
                .any(|row| row.cells().len() > usize::from(cols))
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
            for part in staged.parts {
                let ValidatedPart::Diff(part) = part else {
                    return Err(TerminalError::invalid_transfer(
                        "mixed transfer payload kinds",
                    ));
                };
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
            for part in staged.parts {
                let ValidatedPart::Scrollback(part) = part else {
                    return Err(TerminalError::invalid_transfer(
                        "mixed transfer payload kinds",
                    ));
                };
                rows.extend(part.rows);
            }
            Ok(TransferPayload::Scrollback(rows))
        }
    }
}

use aiterm_lib::pty::{PtySink, PtySpawnSpec};
use aiterm_lib::remote::model::{RemoteEvent, TerminalSize, PROTOCOL_VERSION};
use aiterm_lib::remote::terminal::{
    chunk_scrollback, chunk_snapshot, DiffCoalescer, RemoteTerminal, TerminalEvent,
    TransferAssembler, TransferKind, TransferPayload, TransferStatus, MAX_WIRE_FRAME_BYTES,
};
use aiterm_lib::tabs::{PtyBackend, TabLaunch, TabRegistry, TabUpdate};
use aiterm_lib::terminal::model::{
    CellAttributes, CursorState, Revision, RowPatch, ScreenCell, ScreenDiff, ScreenRow,
    ScreenSnapshot, TerminalColor, TerminalModes,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Default)]
struct FakePty {
    next_id: AtomicU32,
    sinks: Mutex<HashMap<u32, Arc<dyn PtySink>>>,
    writes: Mutex<Vec<Vec<u8>>>,
    resizes: Mutex<Vec<(u16, u16)>>,
}

impl FakePty {
    fn emit(&self, id: u32, bytes: &[u8]) {
        self.sinks.lock().unwrap()[&id].output(id, bytes);
    }

    fn last_id(&self) -> u32 {
        self.next_id.load(Ordering::SeqCst)
    }

    fn writes(&self) -> Vec<Vec<u8>> {
        self.writes.lock().unwrap().clone()
    }

    fn resizes(&self) -> Vec<(u16, u16)> {
        self.resizes.lock().unwrap().clone()
    }
}

impl PtyBackend for FakePty {
    fn spawn(&self, _spec: PtySpawnSpec, sink: Arc<dyn PtySink>) -> Result<u32, String> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        self.sinks.lock().unwrap().insert(id, sink);
        Ok(id)
    }

    fn write(&self, _id: u32, bytes: &[u8]) -> Result<(), String> {
        self.writes.lock().unwrap().push(bytes.to_vec());
        Ok(())
    }

    fn resize(&self, _id: u32, cols: u16, rows: u16) -> Result<(), String> {
        self.resizes.lock().unwrap().push((cols, rows));
        Ok(())
    }

    fn kill(&self, id: u32) {
        self.sinks.lock().unwrap().remove(&id);
    }

    fn pty_for_descendant(&self, _pid: u32) -> Option<u32> {
        None
    }
}

fn size(cols: u16, rows: u16) -> TerminalSize {
    TerminalSize::try_new(cols, rows).unwrap()
}

fn setup(cols: u16, rows: u16) -> (Arc<TabRegistry>, Arc<FakePty>, aiterm_lib::tabs::TabId) {
    let pty = Arc::new(FakePty::default());
    let registry = Arc::new(TabRegistry::with_backend(pty.clone()));
    let tab = registry
        .open(TabLaunch::new("Shell", "remote-test", size(cols, rows)))
        .unwrap();
    (registry, pty, tab)
}

fn screen_row(text: &str) -> ScreenRow {
    let cells = text
        .chars()
        .map(|ch| {
            ScreenCell::try_new(
                ch.to_string(),
                1,
                TerminalColor::Default,
                TerminalColor::Default,
                CellAttributes::default(),
            )
            .unwrap()
        })
        .collect();
    ScreenRow::try_new(cells, false).unwrap()
}

fn row_text(row: &ScreenRow) -> String {
    row.cells()
        .iter()
        .filter(|cell| !cell.is_continuation())
        .map(ScreenCell::text)
        .collect::<String>()
        .trim_end()
        .to_owned()
}

fn large_snapshot(tab: &aiterm_lib::tabs::TabId) -> ScreenSnapshot {
    let dense_text = format!("x{}", "\u{301}".repeat(32));
    let dense_row = ScreenRow::try_new(
        (0..512)
            .map(|_| {
                ScreenCell::try_new(
                    dense_text.clone(),
                    1,
                    TerminalColor::Default,
                    TerminalColor::Default,
                    CellAttributes::default(),
                )
                .unwrap()
            })
            .collect(),
        false,
    )
    .unwrap();
    ScreenSnapshot::new(
        tab.as_str(),
        Revision(7),
        size(512, 48),
        vec![dense_row; 48],
        Vec::new(),
        CursorState::new(0, 0, true),
        TerminalModes::new(false, false, true),
    )
}

#[test]
fn attaching_after_early_escape_sequences_gets_the_current_screen() {
    let (registry, pty, tab) = setup(40, 4);
    pty.emit(pty.last_id(), b"\x1b[?1049hphone view");
    let remote = RemoteTerminal::new(registry);

    let (attached, _events) = remote.attach(&tab).unwrap();

    assert!(attached
        .snapshot()
        .visible()
        .iter()
        .map(row_text)
        .any(|row| row.contains("phone view")));
    assert!(attached.snapshot().modes().alternate_screen());
}

#[test]
fn a_revision_mismatch_is_recovered_by_snapshot_not_byte_replay() {
    let (registry, pty, tab) = setup(20, 2);
    pty.emit(pty.last_id(), b"ready");
    let remote = RemoteTerminal::new(registry);
    let (attached, _events) = remote.attach(&tab).unwrap();
    let wrong = Revision(attached.snapshot().revision().0.saturating_sub(1));

    let recovered = remote.resume(&tab, wrong).unwrap();

    assert!(matches!(recovered, TerminalEvent::Snapshot(_)));
}

#[test]
fn focus_input_and_resize_use_the_registry_attachment_authorization_path() {
    let (registry, pty, tab) = setup(20, 2);
    let remote = RemoteTerminal::new(registry);
    let (attached, _events) = remote.attach(&tab).unwrap();
    assert!(!attached.has_focus());

    remote
        .focus(&tab, attached.attachment_id(), size(30, 4))
        .unwrap();
    remote
        .input(&tab, attached.attachment_id(), b"secret input")
        .unwrap();
    remote
        .resize(&tab, attached.attachment_id(), size(32, 5))
        .unwrap();

    assert_eq!(pty.writes(), vec![b"secret input".to_vec()]);
    assert_eq!(pty.resizes(), vec![(30, 4), (32, 5)]);
}

#[tokio::test]
async fn title_events_bypass_pending_diff_coalescing_without_duplication() {
    let (registry, pty, tab) = setup(40, 4);
    let remote = RemoteTerminal::new(registry);
    let (_attached, mut events) = remote.attach(&tab).unwrap();
    pty.emit(pty.last_id(), b"damage");
    pty.emit(pty.last_id(), b"\x1b]0;phone-title\x07");

    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), events.next())
            .await
            .unwrap(),
        Some(TerminalEvent::Title("phone-title".to_string()))
    );
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(1), events.next())
            .await
            .unwrap(),
        Some(TerminalEvent::Diff(_))
    ));
}

#[tokio::test]
async fn metadata_title_changes_are_projected_once_as_terminal_title_events() {
    let (registry, _pty, tab) = setup(40, 4);
    let remote = RemoteTerminal::new(registry.clone());
    let (_attached, mut events) = remote.attach(&tab).unwrap();

    registry
        .update(&tab, TabUpdate::new().title("renamed remotely"))
        .unwrap();

    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), events.next())
            .await
            .unwrap(),
        Some(TerminalEvent::Title("renamed remotely".to_string()))
    );
}

#[test]
fn coalescing_keeps_newest_row_and_revision_metadata() {
    let mut coalescer = DiffCoalescer::new();
    coalescer
        .push(ScreenDiff::for_tab(
            "tab-a",
            Revision(3),
            Revision(4),
            vec![RowPatch::new(0, screen_row("old"))],
        ))
        .unwrap();
    coalescer
        .push(
            ScreenDiff::for_tab(
                "tab-a",
                Revision(4),
                Revision(5),
                vec![
                    RowPatch::new(0, screen_row("new")),
                    RowPatch::new(1, screen_row("second")),
                ],
            )
            .with_cursor(CursorState::new(1, 2, false))
            .with_modes(TerminalModes::new(true, false, true)),
        )
        .unwrap();

    let merged = coalescer.flush().unwrap();

    assert_eq!(merged.base_revision(), Revision(3));
    assert_eq!(merged.revision(), Revision(5));
    assert_eq!(row_text(merged.rows()[0].content()), "new");
    assert_eq!(row_text(merged.rows()[1].content()), "second");
    assert_eq!(merged.cursor(), Some(&CursorState::new(1, 2, false)));
    assert_eq!(merged.modes(), Some(&TerminalModes::new(true, false, true)));
}

#[test]
fn a_gap_discards_pending_damage_and_requests_snapshot_recovery() {
    let mut coalescer = DiffCoalescer::new();
    coalescer
        .push(ScreenDiff::for_tab(
            "tab-a",
            Revision(1),
            Revision(2),
            vec![],
        ))
        .unwrap();

    assert!(coalescer
        .push(ScreenDiff::for_tab(
            "tab-a",
            Revision(9),
            Revision(10),
            vec![],
        ))
        .is_err());
    assert!(coalescer.flush().is_none());
}

#[test]
fn semantic_snapshot_chunks_size_the_complete_outbound_frame_below_one_mebibyte() {
    let tab = aiterm_lib::tabs::TabId::new();
    let snapshot = large_snapshot(&tab);

    let chunks = chunk_snapshot(41, &tab, &snapshot).unwrap();

    assert!(chunks.len() > 1);
    for chunk in chunks {
        let payload = ciborium_bytes(&chunk);
        let wire = RemoteEvent {
            version: PROTOCOL_VERSION,
            request_id: 41,
            kind: "terminal.snapshot".to_string(),
            payload,
        };
        assert!(ciborium_bytes(&wire).len() < MAX_WIRE_FRAME_BYTES);
    }
}

#[test]
fn an_individually_oversized_semantic_row_is_rejected_without_truncation() {
    let (registry, _pty, tab) = setup(1, 1);
    let snapshot = registry.snapshot(&tab).unwrap();
    let huge = ScreenRow::try_new(
        vec![ScreenCell::try_new(
            "x".repeat(MAX_WIRE_FRAME_BYTES),
            1,
            TerminalColor::Default,
            TerminalColor::Default,
            CellAttributes::default(),
        )
        .unwrap()],
        false,
    )
    .unwrap();

    let error = chunk_scrollback(9, &tab, snapshot.revision(), vec![huge]).unwrap_err();

    assert_eq!(error.code(), "protocol.semantic_row_too_large");
}

#[test]
fn transfer_assembly_rejects_duplicate_out_of_order_mixed_and_stale_chunks() {
    let tab = aiterm_lib::tabs::TabId::new();
    let snapshot = large_snapshot(&tab);
    let chunks = chunk_snapshot(7, &tab, &snapshot).unwrap();
    assert!(chunks.len() > 1);

    let mut duplicate = TransferAssembler::new("connection-a", tab.clone());
    assert_eq!(
        duplicate.accept("connection-a", chunks[0].clone()),
        TransferStatus::Pending
    );
    assert_eq!(
        duplicate.accept("connection-a", chunks[0].clone()),
        TransferStatus::Recover
    );

    let mut out_of_order = TransferAssembler::new("connection-a", tab.clone());
    assert_eq!(
        out_of_order.accept("connection-a", chunks[1].clone()),
        TransferStatus::Recover
    );

    let mut mixed = TransferAssembler::new("connection-a", tab.clone());
    assert_eq!(
        mixed.accept("connection-a", chunks[0].clone()),
        TransferStatus::Pending
    );
    let mut wrong = chunks[1].clone();
    wrong.kind = TransferKind::Scrollback;
    assert_eq!(mixed.accept("connection-a", wrong), TransferStatus::Recover);

    let mut stale = TransferAssembler::new("connection-a", tab);
    assert_eq!(
        stale.accept("connection-a", chunks[0].clone()),
        TransferStatus::Pending
    );
    stale.reset_for_snapshot(Revision(snapshot.revision().0 + 1));
    assert_eq!(
        stale.accept("connection-a", chunks[1].clone()),
        TransferStatus::Recover
    );
}

#[test]
fn a_complete_transfer_is_applied_only_after_every_chunk_validates() {
    let tab = aiterm_lib::tabs::TabId::new();
    let snapshot = large_snapshot(&tab);
    let chunks = chunk_snapshot(22, &tab, &snapshot).unwrap();
    let mut assembler = TransferAssembler::new("connection-a", tab);
    let mut status = TransferStatus::Pending;

    for chunk in chunks {
        status = assembler.accept("connection-a", chunk);
    }

    let TransferStatus::Complete(TransferPayload::Snapshot(received)) = status else {
        panic!("complete ordered snapshot should apply atomically");
    };
    assert_eq!(received.revision(), snapshot.revision());
    assert_eq!(received.visible().len(), snapshot.visible().len());
}

#[test]
fn abandoned_transfer_expires_deterministically() {
    let tab = aiterm_lib::tabs::TabId::new();
    let snapshot = large_snapshot(&tab);
    let chunk = chunk_snapshot(1, &tab, &snapshot).unwrap().remove(0);
    let start = Instant::now();
    let mut assembler =
        TransferAssembler::with_timeout("connection-a", tab, Duration::from_millis(5));
    assert_eq!(
        assembler.accept_at("connection-a", chunk, start),
        TransferStatus::Pending
    );
    assert!(assembler.expire_at(start + Duration::from_millis(6)));
}

fn ciborium_bytes<T: serde::Serialize>(value: &T) -> Vec<u8> {
    let mut bytes = Vec::new();
    ciborium::into_writer(value, &mut bytes).unwrap();
    bytes
}

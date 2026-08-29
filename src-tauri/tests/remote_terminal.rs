use aiterm_lib::pty::{PtySink, PtySpawnSpec};
use aiterm_lib::remote::model::{RemoteEvent, TerminalSize, PROTOCOL_VERSION};
use aiterm_lib::remote::terminal::{
    chunk_diff, chunk_scrollback, chunk_snapshot, plan_snapshot_for_attachment, DiffCoalescer,
    RemoteTerminal, TerminalEvent, TransferAssembler, TransferBudget, TransferKind,
    TransferPayload, TransferStatus, MAX_WIRE_FRAME_BYTES,
};
use aiterm_lib::tabs::{PtyBackend, TabLaunch, TabRegistry, TabUpdate};
use aiterm_lib::terminal::model::{
    CellAttributes, CursorState, Revision, RowPatch, ScreenCell, ScreenDiff, ScreenRow,
    ScreenSnapshot, TerminalColor, TerminalModes,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

#[derive(Default)]
struct FakePty {
    next_id: AtomicU32,
    sinks: Mutex<HashMap<u32, Arc<dyn PtySink>>>,
    writes: Mutex<Vec<Vec<u8>>>,
    resizes: Mutex<Vec<(u16, u16)>>,
    block_write: AtomicBool,
    write_entered: AtomicBool,
    write_released: Mutex<bool>,
    write_changed: Condvar,
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

    fn exit(&self, id: u32, code: Option<u32>, signal: Option<&str>) {
        self.sinks.lock().unwrap()[&id].exited(id, code, signal);
    }

    fn block_writes(&self) {
        self.block_write.store(true, Ordering::SeqCst);
    }

    fn release_write(&self) {
        *self.write_released.lock().unwrap() = true;
        self.write_changed.notify_all();
    }
}

impl PtyBackend for FakePty {
    fn spawn(&self, _spec: PtySpawnSpec, sink: Arc<dyn PtySink>) -> Result<u32, String> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        self.sinks.lock().unwrap().insert(id, sink);
        Ok(id)
    }

    fn write(&self, _id: u32, bytes: &[u8]) -> Result<(), String> {
        if self.block_write.load(Ordering::SeqCst) {
            self.write_entered.store(true, Ordering::SeqCst);
            let mut released = self.write_released.lock().unwrap();
            while !*released {
                released = self.write_changed.wait(released).unwrap();
            }
        }
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
fn attach_captures_the_initial_title_at_the_snapshot_boundary() {
    let (registry, _pty, tab) = setup(40, 4);
    registry
        .update(&tab, TabUpdate::new().title("current title"))
        .unwrap();
    let remote = RemoteTerminal::new(registry);

    let (attached, _events) = remote.attach(&tab).unwrap();

    assert_eq!(attached.title(), "current title");
}

#[tokio::test]
async fn cancelling_an_idle_remote_event_stream_wakes_it_and_detaches_promptly() {
    let (registry, _pty, tab) = setup(40, 4);
    let remote = RemoteTerminal::new(registry.clone());
    let (_attached, mut events) = remote.attach(&tab).unwrap();
    assert_eq!(registry.attachment_count(&tab).unwrap(), 1);

    events.cancel();

    assert_eq!(
        tokio::time::timeout(Duration::from_millis(100), events.next())
            .await
            .unwrap(),
        None
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    while registry.attachment_count(&tab).unwrap() != 0 {
        assert!(tokio::time::Instant::now() < deadline);
        tokio::task::yield_now().await;
    }
}

#[tokio::test]
async fn explicit_cancellation_discards_pending_coalesced_damage() {
    let (registry, pty, tab) = setup(8, 2);
    let remote = RemoteTerminal::new(registry);
    let (_attached, mut events) = remote.attach(&tab).unwrap();
    let cancellation = events.cancellation();

    pty.emit(pty.last_id(), b"x");
    let pending = tokio::spawn(async move { events.next().await });
    tokio::time::sleep(Duration::from_millis(2)).await;
    cancellation.cancel();

    assert_eq!(pending.await.unwrap(), None);
}

#[tokio::test]
async fn natural_exit_publishes_final_snapshot_before_one_exit_and_nothing_after() {
    let (registry, pty, tab) = setup(8, 2);
    let remote = RemoteTerminal::new(registry);
    let (_attached, mut events) = remote.attach(&tab).unwrap();
    pty.emit(pty.last_id(), b"final");
    let pty_id = pty.last_id();
    let exiting = pty.clone();
    let collected = tokio::spawn(async move {
        let mut collected = Vec::new();
        while let Some(event) = events.next().await {
            collected.push(event);
        }
        collected
    });
    tokio::time::sleep(Duration::from_millis(2)).await;
    exiting.exit(pty_id, Some(0), None);

    let events = collected.await.unwrap();
    let exit_index = events
        .iter()
        .position(|event| matches!(event, TerminalEvent::Exited(_)))
        .expect("natural exit must be published");
    assert!(matches!(
        events.get(exit_index.wrapping_sub(1)),
        Some(TerminalEvent::Snapshot(_) | TerminalEvent::SharedSnapshot(_))
    ));
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, TerminalEvent::Exited(_)))
            .count(),
        1
    );
    assert!(!events[exit_index + 1..].iter().any(|event| matches!(
        event,
        TerminalEvent::Snapshot(_) | TerminalEvent::SharedSnapshot(_) | TerminalEvent::Diff(_)
    )));
}

#[test]
fn idle_remote_receive_does_not_depend_on_blocking_pool_capacity() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let occupied = tokio::task::spawn_blocking(move || {
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();

        let (registry, pty, tab) = setup(8, 2);
        let remote = RemoteTerminal::new(registry);
        let (_attached, mut events) = remote.attach(&tab).unwrap();
        pty.emit(pty.last_id(), b"x");
        let event = tokio::time::timeout(Duration::from_secs(1), events.next())
            .await
            .expect("async mailbox must not wait for a blocking worker");
        assert!(matches!(event, Some(TerminalEvent::Diff(_))));

        release_tx.send(()).unwrap();
        occupied.await.unwrap();
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn mailbox_cancellation_wakes_while_ordered_registry_detach_is_blocked() {
    let (registry, pty, tab) = setup(20, 2);
    let remote = RemoteTerminal::new(registry.clone());
    let (attached, mut events) = remote.attach(&tab).unwrap();
    remote
        .focus(&tab, attached.attachment_id(), size(20, 2))
        .unwrap();
    let cancellation = events.cancellation();
    pty.block_writes();
    let writing_remote = remote.clone();
    let writing_tab = tab.clone();
    let writing_attachment = attached.attachment_id().clone();
    let writer = std::thread::spawn(move || {
        writing_remote.input(&writing_tab, &writing_attachment, b"blocked")
    });
    let entered_deadline = Instant::now() + Duration::from_secs(1);
    while !pty.write_entered.load(Ordering::SeqCst) {
        assert!(Instant::now() < entered_deadline);
        tokio::task::yield_now().await;
    }

    cancellation.close_mailbox();
    assert_eq!(
        tokio::time::timeout(Duration::from_millis(100), events.next())
            .await
            .expect("mailbox wake must not wait for send_order"),
        None
    );
    let detached = cancellation.clone();
    let mut detaching = tokio::task::spawn_blocking(move || detached.detach_registry());
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut detaching)
            .await
            .is_err()
    );
    pty.release_write();
    writer.join().unwrap().unwrap();
    detaching.await.unwrap();
    assert_eq!(registry.attachment_count(&tab).unwrap(), 0);
    assert!(registry.get(&tab).unwrap().input_owner().is_none());
}

#[test]
fn a_revision_mismatch_is_recovered_by_snapshot_not_byte_replay() {
    let (registry, pty, tab) = setup(20, 2);
    pty.emit(pty.last_id(), b"ready");
    let remote = RemoteTerminal::new(registry);
    let (attached, _events) = remote.attach(&tab).unwrap();
    let wrong = Revision(attached.snapshot().revision().0.saturating_sub(1));

    let recovered = remote
        .resume(&tab, attached.attachment_id(), wrong)
        .unwrap();

    assert_eq!(
        recovered.snapshot().revision(),
        attached.snapshot().revision()
    );
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
    stale.reset_for_snapshot(Revision(snapshot.revision().0 + 1), size(512, 512));
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

    let TransferStatus::Complete(completion) = status else {
        panic!("complete ordered snapshot should apply atomically");
    };
    let token = completion.token();
    let TransferPayload::Snapshot(received) = completion.into_payload() else {
        panic!("snapshot transfer returned the wrong payload kind");
    };
    assert_eq!(assembler.committed_revision(), Revision(0));
    assert_eq!(received.revision(), snapshot.revision());
    assert_eq!(received.visible().len(), snapshot.visible().len());
    assembler.commit_applied(token).unwrap();
    assert_eq!(assembler.committed_revision(), snapshot.revision());
    assert_eq!(
        assembler.committed_size(),
        Some(size(snapshot.cols(), snapshot.rows()))
    );
}

#[test]
fn a_real_sparse_registry_snapshot_round_trips_without_wire_padding() {
    let (registry, pty, tab) = setup(8, 3);
    pty.emit(pty.last_id(), b"x");
    let remote = RemoteTerminal::new(registry);
    let (attached, _events) = remote.attach(&tab).unwrap();
    assert!(attached
        .snapshot()
        .visible()
        .iter()
        .any(|row| row.cells().len() < usize::from(attached.snapshot().cols())));

    let chunks = chunk_snapshot(77, &tab, attached.snapshot()).unwrap();
    let mut assembler = TransferAssembler::new("connection-a", tab);
    let mut status = TransferStatus::Pending;
    for chunk in chunks {
        status = assembler.accept("connection-a", chunk);
    }

    let TransferStatus::Complete(completion) = status else {
        panic!("canonical sparse snapshot should assemble");
    };
    let token = completion.token();
    let TransferPayload::Snapshot(received) = completion.into_payload() else {
        panic!("snapshot transfer returned the wrong payload kind");
    };
    assert_eq!(received, *attached.snapshot());
    assembler.commit_applied(token).unwrap();
}

#[test]
fn diff_transfer_requires_the_current_applied_revision_without_advancing_on_a_gap() {
    let tab = aiterm_lib::tabs::TabId::new();
    let mut assembler = TransferAssembler::new("connection-a", tab.clone());
    assembler.reset_for_snapshot(Revision(5), size(20, 2));

    let gap = ScreenDiff::for_tab(
        tab.as_str(),
        Revision(7),
        Revision(8),
        vec![RowPatch::new(0, screen_row("gap"))],
    );
    let gap = chunk_diff(1, &tab, &gap).unwrap().remove(0);
    assert_eq!(
        assembler.accept("connection-a", gap),
        TransferStatus::Recover
    );

    let current = ScreenDiff::for_tab(
        tab.as_str(),
        Revision(5),
        Revision(6),
        vec![RowPatch::new(0, screen_row("current"))],
    );
    let current = chunk_diff(2, &tab, &current).unwrap().remove(0);
    let TransferStatus::Complete(completion) = assembler.accept("connection-a", current) else {
        panic!("current diff should complete");
    };
    assert!(
        matches!(completion.payload(), TransferPayload::Diff(diff) if diff.revision() == Revision(6))
    );
    assert_eq!(assembler.committed_revision(), Revision(5));
    assembler.commit_applied(completion.token()).unwrap();
    assert_eq!(assembler.committed_revision(), Revision(6));
}

#[derive(serde::Serialize, serde::Deserialize)]
struct DiffPartWire {
    rows: Vec<RowPatch>,
    cursor: Option<CursorState>,
    modes: Option<TerminalModes>,
}

#[test]
fn multi_chunk_diff_rejects_mismatched_cursor_or_mode_metadata() {
    let tab = aiterm_lib::tabs::TabId::new();
    let snapshot = large_snapshot(&tab);
    let patches = snapshot
        .visible()
        .iter()
        .enumerate()
        .map(|(row, content)| RowPatch::new(u16::try_from(row).unwrap(), content.clone()))
        .collect();
    let diff = ScreenDiff::for_tab(tab.as_str(), Revision(7), Revision(8), patches)
        .with_cursor(CursorState::new(1, 1, true))
        .with_modes(TerminalModes::new(false, true, true));
    let mut chunks = chunk_diff(8, &tab, &diff).unwrap();
    assert!(chunks.len() > 1);
    let mut second: DiffPartWire = ciborium::from_reader(chunks[1].payload.as_slice()).unwrap();
    second.cursor = Some(CursorState::new(2, 1, true));
    chunks[1].payload = ciborium_bytes(&second);
    let mut assembler = TransferAssembler::new("connection-a", tab);
    assembler.reset_for_snapshot(Revision(7), size(512, 512));

    assert_eq!(
        assembler.accept("connection-a", chunks.remove(0)),
        TransferStatus::Pending
    );
    assert_eq!(
        assembler.accept("connection-a", chunks.remove(0)),
        TransferStatus::Recover
    );
}

#[test]
fn a_cell_requires_one_base_scalar_followed_only_by_combining_scalars() {
    let tab = aiterm_lib::tabs::TabId::new();
    let invalid = ScreenRow::try_new(
        vec![ScreenCell::try_new(
            "ab",
            1,
            TerminalColor::Default,
            TerminalColor::Default,
            CellAttributes::default(),
        )
        .unwrap()],
        false,
    )
    .unwrap();
    let chunk = chunk_scrollback(9, &tab, Revision(1), vec![invalid])
        .unwrap()
        .remove(0);
    let mut assembler = TransferAssembler::new("connection-a", tab);

    assert_eq!(
        assembler.accept("connection-a", chunk),
        TransferStatus::Recover
    );
}

#[test]
fn diff_rows_and_cursor_are_bounded_to_the_canonical_viewport_limits() {
    let tab = aiterm_lib::tabs::TabId::new();
    let diff = ScreenDiff::for_tab(
        tab.as_str(),
        Revision(1),
        Revision(2),
        vec![RowPatch::new(999, screen_row("outside"))],
    )
    .with_cursor(CursorState::new(999, 999, true));
    let chunk = chunk_diff(3, &tab, &diff).unwrap().remove(0);
    let mut assembler = TransferAssembler::new("connection-a", tab);
    assembler.reset_for_snapshot(Revision(1), size(20, 2));

    assert_eq!(
        assembler.accept("connection-a", chunk),
        TransferStatus::Recover
    );
}

#[test]
fn assembler_validates_diffs_against_committed_dimensions_and_never_advances_on_reject() {
    let tab = aiterm_lib::tabs::TabId::new();
    let mut assembler = TransferAssembler::new("connection-a", tab.clone());
    assembler.reset_for_snapshot(Revision(9), size(20, 2));

    let invalid = [
        ScreenDiff::for_tab(
            tab.as_str(),
            Revision(9),
            Revision(10),
            vec![RowPatch::new(10, screen_row("x"))],
        ),
        ScreenDiff::for_tab(
            tab.as_str(),
            Revision(9),
            Revision(10),
            vec![RowPatch::new(0, screen_row(&"x".repeat(21)))],
        ),
        ScreenDiff::for_tab(tab.as_str(), Revision(9), Revision(10), vec![])
            .with_cursor(CursorState::new(20, 0, true)),
    ];
    for diff in invalid {
        let chunk = chunk_diff(1, &tab, &diff).unwrap().remove(0);
        assert_eq!(
            assembler.accept("connection-a", chunk),
            TransferStatus::Recover
        );
        assert_eq!(assembler.committed_revision(), Revision(9));
        assert_eq!(assembler.committed_size(), Some(size(20, 2)));
    }
}

#[test]
fn pending_application_blocks_later_diffs_until_commit_or_reject() {
    let tab = aiterm_lib::tabs::TabId::new();
    let mut assembler = TransferAssembler::new("connection-a", tab.clone());
    assembler.reset_for_snapshot(Revision(1), size(20, 2));
    let diff = ScreenDiff::for_tab(
        tab.as_str(),
        Revision(1),
        Revision(2),
        vec![RowPatch::new(0, screen_row("ok"))],
    );
    let chunk = chunk_diff(1, &tab, &diff).unwrap().remove(0);
    let TransferStatus::Complete(completion) = assembler.accept("connection-a", chunk.clone())
    else {
        panic!("valid diff should complete");
    };
    assert_eq!(
        assembler.accept("connection-a", chunk),
        TransferStatus::Recover
    );
    assert_eq!(assembler.committed_revision(), Revision(1));
    assembler.reject_applied(completion.token()).unwrap();
    assert_eq!(assembler.committed_revision(), Revision(1));
}

#[test]
fn transfer_parts_reject_trailing_cbor_and_kind_specific_row_overflow() {
    let tab = aiterm_lib::tabs::TabId::new();
    let snapshot = large_snapshot(&tab);
    let mut trailing = chunk_snapshot(1, &tab, &snapshot).unwrap().remove(0);
    trailing.payload.push(0xf6);
    let mut assembler = TransferAssembler::new("connection-a", tab.clone());
    assert_eq!(
        assembler.accept("connection-a", trailing),
        TransferStatus::Recover
    );

    let mut overflow = chunk_snapshot(2, &tab, &snapshot).unwrap().remove(0);
    overflow.row_end = 513;
    assert_eq!(
        assembler.accept("connection-a", overflow),
        TransferStatus::Recover
    );

    let rows = (0..256).map(|_| screen_row("x")).collect();
    let mut scrollback = chunk_scrollback(3, &tab, Revision(1), rows)
        .unwrap()
        .remove(0);
    scrollback.row_end = 257;
    assert_eq!(
        assembler.accept("connection-a", scrollback),
        TransferStatus::Recover
    );
}

#[test]
fn transfer_id_must_be_a_bounded_canonical_lowercase_hyphenated_uuid() {
    let tab = aiterm_lib::tabs::TabId::new();
    let snapshot = large_snapshot(&tab);
    for invalid in [
        "550E8400-E29B-41D4-A716-446655440000".to_owned(),
        "550e8400e29b41d4a716446655440000".to_owned(),
        "x".repeat(4096),
    ] {
        let mut chunk = chunk_snapshot(1, &tab, &snapshot).unwrap().remove(0);
        chunk.transfer_id = invalid;
        let mut assembler = TransferAssembler::new("connection-a", tab.clone());
        assert_eq!(
            assembler.accept("connection-a", chunk),
            TransferStatus::Recover
        );
    }
}

#[test]
fn legal_maximum_viewport_round_trips_through_sender_plan_and_receiver_budget() {
    let tab = aiterm_lib::tabs::TabId::new();
    let text = format!("\u{1f600}{}", "\u{1d167}".repeat(32));
    let cell = ScreenCell::try_new(
        text,
        1,
        TerminalColor::Rgb {
            r: 255,
            g: 254,
            b: 253,
        },
        TerminalColor::Rgb { r: 1, g: 2, b: 3 },
        CellAttributes::new(true, true, true, true, true, true, true),
    )
    .unwrap();
    let row = ScreenRow::try_new(vec![cell; 512], false).unwrap();
    let snapshot = ScreenSnapshot::new(
        tab.as_str(),
        Revision(9),
        size(512, 512),
        vec![row; 512],
        Vec::new(),
        CursorState::new(511, 511, true),
        TerminalModes::new(true, true, true),
    );
    let mut plan = plan_snapshot_for_attachment(81, &tab, None, snapshot).unwrap();
    let mut assembler = TransferAssembler::new("connection-max", tab);
    let mut status = TransferStatus::Pending;
    let mut chunks = 0;
    while let Some(chunk) = plan.next_chunk().unwrap() {
        let chunk_debug = (chunk.index, chunk.total, chunk.row_start, chunk.row_end);
        let wire = RemoteEvent {
            version: PROTOCOL_VERSION,
            request_id: chunk.request_id,
            kind: "terminal.snapshot".to_owned(),
            payload: ciborium_bytes(&chunk),
        };
        assert!(ciborium_bytes(&wire).len() < MAX_WIRE_FRAME_BYTES);
        status = assembler.accept("connection-max", chunk);
        assert_ne!(
            status,
            TransferStatus::Recover,
            "recovered at chunk {chunks}: {chunk_debug:?}"
        );
        chunks += 1;
    }
    let TransferStatus::Complete(completion) = status else {
        panic!("maximum legal viewport must complete");
    };
    let token = completion.token();
    let TransferPayload::Snapshot(received) = completion.into_payload() else {
        panic!("maximum viewport completed with wrong transfer kind");
    };
    assert_eq!(received.visible().len(), 512);
    assert!(chunks > 1);
    assembler.commit_applied(token).unwrap();
    assert_eq!(assembler.committed_size(), Some(size(512, 512)));
}

#[test]
fn shared_connection_budget_allows_only_one_staged_transfer_and_releases_on_expiry() {
    let tab_a = aiterm_lib::tabs::TabId::new();
    let tab_b = aiterm_lib::tabs::TabId::new();
    let snapshot_a = large_snapshot(&tab_a);
    let snapshot_b = large_snapshot(&tab_b);
    let chunk_a = chunk_snapshot(1, &tab_a, &snapshot_a).unwrap().remove(0);
    let chunk_b = chunk_snapshot(2, &tab_b, &snapshot_b).unwrap().remove(0);
    let budget = TransferBudget::single_active();
    let start = Instant::now();
    let mut first = TransferAssembler::with_timeout_and_budget(
        "connection-a",
        tab_a,
        Duration::from_millis(5),
        budget.clone(),
    );
    let mut second = TransferAssembler::with_budget("connection-a", tab_b, budget);

    assert_eq!(
        first.accept_at("connection-a", chunk_a, start),
        TransferStatus::Pending
    );
    assert_eq!(
        second.accept("connection-a", chunk_b.clone()),
        TransferStatus::Recover
    );
    assert!(first.expire_at(start + Duration::from_millis(6)));
    assert_eq!(
        second.accept("connection-a", chunk_b),
        TransferStatus::Pending
    );
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

#[test]
fn staged_transfer_exposes_an_active_expiration_deadline() {
    let tab = aiterm_lib::tabs::TabId::new();
    let snapshot = large_snapshot(&tab);
    let chunk = chunk_snapshot(1, &tab, &snapshot).unwrap().remove(0);
    let start = Instant::now();
    let mut assembler =
        TransferAssembler::with_timeout("connection-a", tab, Duration::from_millis(5));
    assembler.accept_at("connection-a", chunk, start);

    assert_eq!(assembler.deadline(), Some(start + Duration::from_millis(5)));
}

#[test]
fn valid_chunk_progress_extends_inactivity_deadline_but_duplicate_does_not() {
    let tab = aiterm_lib::tabs::TabId::new();
    let chunks = chunk_snapshot(73, &tab, &large_snapshot(&tab)).unwrap();
    assert!(chunks.len() > 2);
    let start = Instant::now();
    let idle = Duration::from_millis(20);
    let mut assembler = TransferAssembler::with_timeout("connection-a", tab, idle);
    assert_eq!(
        assembler.accept_at("connection-a", chunks[0].clone(), start),
        TransferStatus::Pending
    );
    assert_eq!(assembler.deadline(), Some(start + idle));
    let progress = start + Duration::from_millis(15);
    assert_eq!(
        assembler.accept_at("connection-a", chunks[1].clone(), progress),
        TransferStatus::Pending
    );
    assert_eq!(assembler.deadline(), Some(progress + idle));
    assert_eq!(
        assembler.accept_at(
            "connection-a",
            chunks[1].clone(),
            progress + Duration::from_millis(1)
        ),
        TransferStatus::Recover
    );
    assert_eq!(assembler.deadline(), None);
}

fn ciborium_bytes<T: serde::Serialize>(value: &T) -> Vec<u8> {
    let mut bytes = Vec::new();
    ciborium::into_writer(value, &mut bytes).unwrap();
    bytes
}

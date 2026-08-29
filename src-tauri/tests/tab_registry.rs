//! PTY output-boundary checks. Tab ownership tests join this file in Task 4.

use aiterm_lib::pty::{
    clear_observer, set_observer, PtyManager, PtyObserver, PtySink, PtySpawnSpec,
};
use aiterm_lib::remote::model::TerminalSize;
use aiterm_lib::tabs::{
    AttachmentId, AttachmentKind, PtyBackend, TabEvent, TabId, TabLaunch, TabRegistry, TabState,
    TabUpdate,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

// The compatibility observer is process-global until Task 6. Serialize PTY
// tests so one test cannot intentionally observe another test's output.
static PTY_TEST_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Debug, PartialEq, Eq)]
struct Exit {
    pty_id: u32,
    code: Option<u32>,
    signal: Option<String>,
}

#[derive(Default)]
struct RecordingSink {
    output: Mutex<Vec<u8>>,
    exits: Mutex<Vec<Exit>>,
    exited: Condvar,
}

impl RecordingSink {
    fn output(&self) -> Vec<u8> {
        self.output.lock().unwrap().clone()
    }

    fn exits(&self) -> Vec<Exit> {
        self.exits.lock().unwrap().clone()
    }

    fn wait_for_exit(&self) {
        let exits = self.exits.lock().unwrap();
        let (exits, timeout) = self
            .exited
            .wait_timeout_while(exits, Duration::from_secs(10), |exits| exits.is_empty())
            .unwrap();
        assert!(
            !timeout.timed_out(),
            "the PTY never delivered its exit event: {exits:?}"
        );
    }
}

impl PtySink for RecordingSink {
    fn output(&self, _pty_id: u32, bytes: &[u8]) {
        self.output.lock().unwrap().extend_from_slice(bytes);
    }

    fn exited(&self, pty_id: u32, code: Option<u32>, signal: Option<&str>) {
        self.exits.lock().unwrap().push(Exit {
            pty_id,
            code,
            signal: signal.map(str::to_owned),
        });
        self.exited.notify_all();
    }
}

impl PtyObserver for RecordingSink {
    fn on_output(&self, pty_id: u32, bytes: &[u8]) {
        PtySink::output(self, pty_id, bytes);
    }

    fn on_exit(&self, pty_id: u32, code: Option<u32>, signal: Option<&str>) {
        PtySink::exited(self, pty_id, code, signal);
    }
}

struct ObserverReset;

impl Drop for ObserverReset {
    fn drop(&mut self) {
        clear_observer();
    }
}

#[test]
fn spawn_delivers_output_and_exactly_one_exit_to_its_sink() {
    let _pty_test_lock = PTY_TEST_LOCK.lock().unwrap();
    let manager = PtyManager::default();
    let sink = Arc::new(RecordingSink::default());

    let id = manager
        .spawn(PtySpawnSpec::command("printf first"), sink.clone())
        .expect("spawn PTY");
    sink.wait_for_exit();

    assert_eq!(sink.output(), b"first");
    assert_eq!(
        sink.exits(),
        vec![Exit {
            pty_id: id,
            code: Some(0),
            signal: None,
        }],
        "a PTY must report exactly one terminal exit to its own sink"
    );
}

#[test]
fn each_spawn_routes_bytes_only_to_its_own_sink() {
    let _pty_test_lock = PTY_TEST_LOCK.lock().unwrap();
    let manager = PtyManager::default();
    let first = Arc::new(RecordingSink::default());
    let second = Arc::new(RecordingSink::default());

    let first_id = manager
        .spawn(PtySpawnSpec::command("printf alpha"), first.clone())
        .expect("spawn first PTY");
    let second_id = manager
        .spawn(PtySpawnSpec::command("printf beta"), second.clone())
        .expect("spawn second PTY");
    first.wait_for_exit();
    second.wait_for_exit();

    assert_eq!(first.output(), b"alpha");
    assert_eq!(second.output(), b"beta");
    assert_eq!(first.exits()[0].pty_id, first_id);
    assert_eq!(second.exits()[0].pty_id, second_id);
}

#[test]
fn spawned_pty_also_reaches_the_temporary_legacy_observer() {
    let _pty_test_lock = PTY_TEST_LOCK.lock().unwrap();
    let _observer_reset = ObserverReset;
    let manager = PtyManager::default();
    let sink = Arc::new(RecordingSink::default());
    let observer = Arc::new(RecordingSink::default());
    set_observer(observer.clone());

    let id = manager
        .spawn(PtySpawnSpec::command("printf bridge"), sink.clone())
        .expect("spawn PTY");
    sink.wait_for_exit();
    observer.wait_for_exit();

    assert_eq!(sink.output(), b"bridge");
    assert_eq!(observer.output(), b"bridge");
    assert_eq!(observer.exits()[0].pty_id, id);
}

#[test]
fn a_naturally_exited_pty_is_removed_before_its_sink_is_notified() {
    let _pty_test_lock = PTY_TEST_LOCK.lock().unwrap();
    let manager = PtyManager::default();
    let sink = Arc::new(RecordingSink::default());

    let id = manager
        .spawn(PtySpawnSpec::command("printf finished"), sink.clone())
        .expect("spawn PTY");
    sink.wait_for_exit();

    assert_eq!(
        manager.write(id, b"late input").unwrap_err(),
        "no such pty",
        "a reaped PTY must no longer retain a writer"
    );
    assert_eq!(
        manager.resize(id, 100, 30).unwrap_err(),
        "no such pty",
        "a reaped PTY must no longer retain its master"
    );
}

#[derive(Default)]
struct FakePty {
    next_id: AtomicU32,
    sinks: Mutex<HashMap<u32, Arc<dyn PtySink>>>,
    writes: Mutex<Vec<(u32, Vec<u8>)>>,
    resizes: Mutex<Vec<(u32, u16, u16)>>,
    kills: Mutex<Vec<u32>>,
    descendants: Mutex<HashMap<u32, u32>>,
}

impl FakePty {
    fn last_id(&self) -> u32 {
        self.next_id.load(Ordering::SeqCst)
    }

    fn emit_output(&self, id: u32, bytes: &[u8]) {
        self.sinks
            .lock()
            .unwrap()
            .get(&id)
            .expect("fake PTY has a registered sink")
            .output(id, bytes);
    }

    fn exit(&self, id: u32, code: Option<u32>, signal: Option<&str>) {
        let sink = self
            .sinks
            .lock()
            .unwrap()
            .remove(&id)
            .expect("fake PTY has a registered sink");
        sink.exited(id, code, signal);
    }

    fn writes(&self) -> Vec<Vec<u8>> {
        self.writes
            .lock()
            .unwrap()
            .iter()
            .map(|(_, bytes)| bytes.clone())
            .collect()
    }

    fn resizes(&self) -> Vec<(u16, u16)> {
        self.resizes
            .lock()
            .unwrap()
            .iter()
            .map(|(_, cols, rows)| (*cols, *rows))
            .collect()
    }

    fn map_descendant(&self, pid: u32, pty_id: u32) {
        self.descendants.lock().unwrap().insert(pid, pty_id);
    }
}

impl PtyBackend for FakePty {
    fn spawn(&self, spec: PtySpawnSpec, sink: Arc<dyn PtySink>) -> Result<u32, String> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        self.sinks.lock().unwrap().insert(id, sink.clone());
        if spec.command.as_deref() == Some("emit-first") {
            sink.output(id, b"ready");
        } else if spec.command.as_deref() == Some("query-first") {
            sink.output(id, b"\x1b[5n");
        }
        Ok(id)
    }

    fn write(&self, id: u32, bytes: &[u8]) -> Result<(), String> {
        self.writes.lock().unwrap().push((id, bytes.to_vec()));
        Ok(())
    }

    fn resize(&self, id: u32, cols: u16, rows: u16) -> Result<(), String> {
        self.resizes.lock().unwrap().push((id, cols, rows));
        Ok(())
    }

    fn kill(&self, id: u32) {
        self.kills.lock().unwrap().push(id);
        if let Some(sink) = self.sinks.lock().unwrap().remove(&id) {
            sink.exited(id, None, None);
        }
    }

    fn pty_for_descendant(&self, pid: u32) -> Option<u32> {
        self.descendants.lock().unwrap().get(&pid).copied()
    }
}

fn size(cols: u16, rows: u16) -> TerminalSize {
    TerminalSize::try_new(cols, rows).expect("test terminal dimensions are valid")
}

fn shell_launch(slot: &str) -> TabLaunch {
    TabLaunch::new("Shell", slot, size(80, 24))
}

fn registry() -> (TabRegistry, Arc<FakePty>) {
    let pty = Arc::new(FakePty::default());
    (TabRegistry::with_backend(pty.clone()), pty)
}

fn text(snapshot: &aiterm_lib::terminal::model::ScreenSnapshot) -> Vec<String> {
    snapshot
        .visible()
        .iter()
        .map(|row| row.cells().iter().map(|cell| cell.text()).collect())
        .collect()
}

fn recv_matching(
    receiver: &aiterm_lib::tabs::TabEventReceiver,
    predicate: impl Fn(&TabEvent) -> bool,
) -> TabEvent {
    for _ in 0..10 {
        let event = receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("expected a tab event");
        if predicate(&event) {
            return event;
        }
    }
    panic!("matching tab event was not delivered");
}

#[test]
fn registry_lists_a_tab_and_owns_its_first_output_before_any_client_attaches() {
    let (registry, _) = registry();
    let id = registry
        .open(shell_launch("shell:ready").with_command("emit-first"))
        .unwrap();

    assert_eq!(registry.list()[0].id(), &id);
    assert!(text(&registry.snapshot(&id).unwrap())[0].contains("ready"));
}

#[test]
fn registry_late_binds_the_pty_before_flushing_a_first_chunk_terminal_reply() {
    let (registry, pty) = registry();

    registry
        .open(shell_launch("shell:query").with_command("query-first"))
        .unwrap();

    assert_eq!(pty.writes(), vec![b"\x1b[0n".to_vec()]);
}

#[test]
fn registry_mints_random_non_numeric_tab_and_attachment_ids() {
    let (registry, _) = registry();
    let first = registry.open(shell_launch("slot:first")).unwrap();
    let second = registry.open(shell_launch("slot:second")).unwrap();
    let attachment = registry.attach(&first, AttachmentKind::Remote).unwrap();

    assert_ne!(first, second);
    assert!(uuid::Uuid::parse_str(first.as_str()).is_ok());
    assert!(uuid::Uuid::parse_str(second.as_str()).is_ok());
    assert!(uuid::Uuid::parse_str(attachment.id.as_str()).is_ok());
    assert!(first.as_str().parse::<u32>().is_err());
    assert!(attachment.id.as_str().parse::<u32>().is_err());

    let encoded = serde_json::to_value(registry.list()).unwrap();
    assert_eq!(encoded[0]["id"], first.as_str());
    assert!(encoded[0].get("ptyId").is_none());
}

#[test]
fn registry_rejects_duplicate_slots_without_spawning_a_second_pty() {
    let (registry, pty) = registry();
    registry.open(shell_launch("claude:repo")).unwrap();

    let error = registry.open(shell_launch("claude:repo")).unwrap_err();

    assert_eq!(error.code(), "tab.slot_in_use");
    assert_eq!(pty.last_id(), 1);
}

#[test]
fn registry_updates_authoritative_metadata_without_changing_tab_identity() {
    let (registry, _) = registry();
    let id = registry.open(shell_launch("slot:old")).unwrap();

    let updated = registry
        .update(
            &id,
            TabUpdate::new()
                .title("Renamed")
                .session_id("session-1")
                .agent_id("claude"),
        )
        .unwrap();

    assert_eq!(updated.id(), &id);
    assert_eq!(updated.title(), "Renamed");
    assert_eq!(updated.session_id(), Some("session-1"));
    assert_eq!(updated.agent_id(), Some("claude"));
    assert_eq!(registry.list(), vec![updated]);
}

#[test]
fn session_hook_rekeys_the_slot_atomically_and_releases_the_old_slot() {
    let (registry, _) = registry();
    let id = registry.open(shell_launch("claude:repo")).unwrap();

    let descriptor = registry.rekey_session(&id, "session-new").unwrap();

    assert_eq!(descriptor.id(), &id);
    assert_eq!(descriptor.slot_id(), "session-new");
    assert_eq!(descriptor.session_id(), Some("session-new"));
    registry.open(shell_launch("claude:repo")).unwrap();
    assert_eq!(
        registry
            .open(shell_launch("session-new"))
            .unwrap_err()
            .code(),
        "tab.slot_in_use"
    );
}

#[test]
fn descendant_lookup_returns_an_opaque_tab_id_not_the_numeric_pty_id() {
    let (registry, pty) = registry();
    let id = registry.open(shell_launch("slot:descendant")).unwrap();
    pty.map_descendant(44_001, pty.last_id());

    assert_eq!(registry.tab_for_descendant(44_001), Some(id));
    assert_eq!(registry.tab_for_descendant(44_002), None);
}

#[test]
fn phone_cannot_input_or_resize_until_it_takes_focus() {
    let (registry, pty) = registry();
    let tab = registry.open(shell_launch("slot:focus")).unwrap();
    let desktop = registry.attach(&tab, AttachmentKind::Desktop).unwrap();
    let phone = registry.attach(&tab, AttachmentKind::Remote).unwrap();

    assert_eq!(
        registry.input(&tab, &phone.id, b"x").unwrap_err().code(),
        "terminal.input_not_owned"
    );
    registry.take_focus(&tab, &phone.id, size(42, 18)).unwrap();
    assert_eq!(registry.input(&tab, &phone.id, b"x"), Ok(()));
    assert_eq!(
        registry
            .resize(&tab, &desktop.id, size(80, 24))
            .unwrap_err()
            .code(),
        "terminal.input_not_owned"
    );
    assert_eq!(pty.writes(), vec![b"x".to_vec()]);
    assert_eq!(pty.resizes(), vec![(42, 18)]);

    let focus = recv_matching(
        &desktop.events,
        |event| matches!(event, TabEvent::FocusChanged { owner: Some(owner), .. } if owner == &phone.id),
    );
    assert!(matches!(
        focus,
        TabEvent::FocusChanged { size: current, .. } if current == size(42, 18)
    ));
}

#[test]
fn desktop_receives_raw_bytes_and_never_typed_screen_frames() {
    let (registry, pty) = registry();
    let tab = registry.open(shell_launch("slot:desktop-raw")).unwrap();
    let desktop = registry.attach(&tab, AttachmentKind::Desktop).unwrap();
    let remote = registry.attach(&tab, AttachmentKind::Remote).unwrap();
    let _ = remote
        .events
        .recv_timeout(Duration::from_secs(1))
        .expect("remote initial snapshot");

    pty.emit_output(pty.last_id(), b"raw\0bytes");

    assert_eq!(
        recv_matching(&desktop.events, |event| matches!(event, TabEvent::Raw(_))),
        TabEvent::Raw(b"raw\0bytes".to_vec())
    );
    while let Ok(event) = desktop.events.try_recv() {
        assert!(!matches!(event, TabEvent::Snapshot(_) | TabEvent::Diff(_)));
    }
}

#[test]
fn remote_attach_gets_a_viewport_snapshot_then_typed_diffs_and_no_raw_bytes() {
    let (registry, pty) = registry();
    let tab = registry.open(shell_launch("slot:remote-diff")).unwrap();
    pty.emit_output(pty.last_id(), b"before\r\nline\r\nold");
    let remote = registry.attach(&tab, AttachmentKind::Remote).unwrap();

    let snapshot = recv_matching(&remote.events, |event| {
        matches!(event, TabEvent::Snapshot(_))
    });
    let TabEvent::Snapshot(snapshot) = snapshot else {
        unreachable!()
    };
    assert!(snapshot.scrollback().is_empty());
    assert!(text(&snapshot).iter().any(|line| line.contains("before")));

    pty.emit_output(pty.last_id(), b"\rnew");
    let event = remote
        .events
        .recv_timeout(Duration::from_secs(1))
        .expect("remote typed damage");
    assert!(matches!(event, TabEvent::Diff(diff) if diff.tab_id() == tab.as_str()));
    while let Ok(event) = remote.events.try_recv() {
        assert!(!matches!(event, TabEvent::Raw(_)));
    }
}

#[test]
fn scrollback_is_paged_separately_from_remote_viewport_snapshots() {
    let (registry, pty) = registry();
    let tab = registry
        .open(TabLaunch::new("Small", "slot:scrollback", size(8, 2)))
        .unwrap();
    pty.emit_output(pty.last_id(), b"one\r\ntwo\r\nthree");
    let remote = registry.attach(&tab, AttachmentKind::Remote).unwrap();
    let TabEvent::Snapshot(snapshot) = recv_matching(&remote.events, |event| {
        matches!(event, TabEvent::Snapshot(_))
    }) else {
        unreachable!()
    };

    assert!(snapshot.scrollback().is_empty());
    assert_eq!(registry.scrollback(&tab, 0, 1).unwrap().len(), 1);
}

#[test]
fn desktop_owned_queries_suppress_rust_replies_because_xterm_answers_them() {
    let (registry, pty) = registry();
    let tab = registry.open(shell_launch("slot:xterm-reply")).unwrap();
    let _desktop = registry.attach(&tab, AttachmentKind::Desktop).unwrap();

    pty.emit_output(pty.last_id(), b"\x1b[5n");

    assert!(pty.writes().is_empty());
}

#[test]
fn remote_focus_suppresses_xterm_input_and_delivers_the_rust_reply() {
    let (registry, pty) = registry();
    let tab = registry.open(shell_launch("slot:rust-reply")).unwrap();
    let desktop = registry.attach(&tab, AttachmentKind::Desktop).unwrap();
    let remote = registry.attach(&tab, AttachmentKind::Remote).unwrap();
    registry.take_focus(&tab, &remote.id, size(80, 24)).unwrap();

    assert_eq!(
        registry
            .input(&tab, &desktop.id, b"\x1b[0n")
            .unwrap_err()
            .code(),
        "terminal.input_not_owned"
    );
    pty.emit_output(pty.last_id(), b"\x1b[5n");

    assert_eq!(pty.writes(), vec![b"\x1b[0n".to_vec()]);
}

#[test]
fn natural_exit_and_explicit_close_each_notify_once_and_cleanup_pty_mapping() {
    let (registry, pty) = registry();
    let natural = registry.open(shell_launch("slot:natural")).unwrap();
    let attachment = registry.attach(&natural, AttachmentKind::Remote).unwrap();
    let natural_pty = pty.last_id();
    pty.map_descendant(91_001, natural_pty);

    pty.exit(natural_pty, Some(7), None);

    assert!(matches!(
        recv_matching(&attachment.events, |event| matches!(event, TabEvent::Exited(_))),
        TabEvent::Exited(exit) if exit.code() == Some(7) && !exit.requested()
    ));
    assert_eq!(registry.get(&natural).unwrap().state(), &TabState::Exited);
    assert_eq!(registry.tab_for_descendant(91_001), None);
    registry.close(&natural).unwrap();
    assert!(attachment.events.try_recv().is_err());

    let explicit = registry.open(shell_launch("slot:explicit")).unwrap();
    let attachment = registry.attach(&explicit, AttachmentKind::Remote).unwrap();
    registry.close(&explicit).unwrap();
    assert!(matches!(
        recv_matching(&attachment.events, |event| matches!(event, TabEvent::Exited(_))),
        TabEvent::Exited(exit) if exit.requested()
    ));
    assert_eq!(registry.get(&explicit).unwrap_err().code(), "tab.not_found");
    assert!(attachment.events.try_recv().is_err());
}

#[test]
fn dropping_an_attachment_cleans_it_up_and_releases_focus_ownership() {
    let (registry, pty) = registry();
    let tab = registry.open(shell_launch("slot:drop")).unwrap();
    let desktop = registry.attach(&tab, AttachmentKind::Desktop).unwrap();
    let old_id = desktop.id.clone();
    drop(desktop);
    let replacement = registry.attach(&tab, AttachmentKind::Desktop).unwrap();

    assert_eq!(
        registry.input(&tab, &old_id, b"stale").unwrap_err().code(),
        "terminal.attachment_not_found"
    );
    registry.input(&tab, &replacement.id, b"live").unwrap();
    assert_eq!(pty.writes(), vec![b"live".to_vec()]);
}

#[test]
fn remote_queue_loss_discards_stale_diffs_and_recovers_with_a_snapshot() {
    let pty = Arc::new(FakePty::default());
    let registry = TabRegistry::with_backend_and_queue_capacity(pty.clone(), 1);
    let tab = registry.open(shell_launch("slot:overflow")).unwrap();
    let remote = registry.attach(&tab, AttachmentKind::Remote).unwrap();
    let _initial = remote.events.recv_timeout(Duration::from_secs(1)).unwrap();

    pty.emit_output(pty.last_id(), b"one");
    pty.emit_output(pty.last_id(), b"\rtwo");

    let recovered = remote.events.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(matches!(recovered, TabEvent::Snapshot(_)));
    assert!(text(match &recovered {
        TabEvent::Snapshot(snapshot) => snapshot,
        _ => unreachable!(),
    })[0]
        .contains("two"));
    assert!(remote.events.try_recv().is_err());
}

#[test]
fn registry_rejects_unknown_tab_and_attachment_ids_consistently() {
    let (registry, _) = registry();
    let tab = registry.open(shell_launch("slot:unknown")).unwrap();
    let unknown_tab = TabId::new();
    let unknown_attachment = AttachmentId::new();

    assert_eq!(
        registry.snapshot(&unknown_tab).unwrap_err().code(),
        "tab.not_found"
    );
    assert_eq!(
        registry
            .attach(&unknown_tab, AttachmentKind::Remote)
            .unwrap_err()
            .code(),
        "tab.not_found"
    );
    assert_eq!(
        registry
            .input(&tab, &unknown_attachment, b"x")
            .unwrap_err()
            .code(),
        "terminal.attachment_not_found"
    );
    assert_eq!(
        registry
            .resize(&tab, &unknown_attachment, size(80, 24))
            .unwrap_err()
            .code(),
        "terminal.attachment_not_found"
    );
    assert_eq!(
        registry.close(&unknown_tab).unwrap_err().code(),
        "tab.not_found"
    );
}

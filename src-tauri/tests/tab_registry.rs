//! PTY output-boundary checks. Tab ownership tests join this file in Task 4.

use aiterm_lib::pty::{
    clear_observer, set_observer, PtyManager, PtyObserver, PtySink, PtySpawnSpec,
};
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

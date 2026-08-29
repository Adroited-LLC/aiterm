//! Terminal broker: opaque stream ids, bounded replay and exclusive input.
//!
//! Every test drives output through the [`PtyObserver`] hook rather than a real
//! pty. That is the same entry point `pty_spawn`'s reader thread uses, so these
//! exercise the production path without a Tauri runtime or a spawned shell.

use aiterm_lib::pty::PtyObserver;
use aiterm_lib::remote::model::TerminalSize;
use aiterm_lib::remote::terminal::{
    PtyControl, Replay, TerminalBroker, TerminalEvent, TerminalEvents, REPLAY_CAPACITY,
};
use std::sync::{Arc, Mutex};

/// Chosen so the numeric id cannot appear by accident in a serialized form:
/// 0xDEADBEEF is ten decimal digits, and CBOR/JSON never split it across a
/// byte array the way a small id could be reconstructed from one.
const TEST_PTY: u32 = 0xDEAD_BEEF;

#[derive(Default)]
struct RecordingPty {
    writes: Mutex<Vec<(u32, Vec<u8>)>>,
    resizes: Mutex<Vec<(u32, u16, u16)>>,
}

impl RecordingPty {
    fn writes(&self) -> Vec<(u32, Vec<u8>)> {
        self.writes.lock().unwrap().clone()
    }

    fn resizes(&self) -> Vec<(u32, u16, u16)> {
        self.resizes.lock().unwrap().clone()
    }
}

impl PtyControl for RecordingPty {
    fn write(&self, pty_id: u32, data: &[u8]) -> Result<(), String> {
        self.writes.lock().unwrap().push((pty_id, data.to_vec()));
        Ok(())
    }

    fn resize(&self, pty_id: u32, cols: u16, rows: u16) -> Result<(), String> {
        self.resizes.lock().unwrap().push((pty_id, cols, rows));
        Ok(())
    }
}

fn broker() -> (Arc<TerminalBroker>, Arc<RecordingPty>) {
    let control = Arc::new(RecordingPty::default());
    (Arc::new(TerminalBroker::new(control.clone())), control)
}

fn drain(events: &mut TerminalEvents) -> Vec<TerminalEvent> {
    let mut out = Vec::new();
    while let Some(event) = events.try_next() {
        out.push(event);
    }
    out
}

fn replay_bytes(replay: &Replay) -> &[u8] {
    match replay {
        Replay::Snapshot { bytes, .. } | Replay::Delta { bytes, .. } => bytes,
    }
}

#[test]
fn second_attached_client_cannot_write_until_it_takes_focus() {
    let (broker, control) = broker();
    let stream = broker.open_stream(TEST_PTY);

    let (first, _first_events) = broker.attach(&stream, None).expect("first attach");
    let (second, _second_events) = broker.attach(&stream, None).expect("second attach");
    assert!(first.has_focus(), "the first client to attach owns input");
    assert!(!second.has_focus(), "a second client attaches read-only");

    broker
        .input(&stream, first.subscriber(), b"ls\n")
        .expect("the owner may type");
    let denied = broker
        .input(&stream, second.subscriber(), b"rm -rf /\n")
        .expect_err("a non-owner must be refused");
    assert_eq!(denied.code(), "terminal.input_not_owned");
    assert_eq!(
        control.writes(),
        vec![(TEST_PTY, b"ls\n".to_vec())],
        "refused input must never reach the pty",
    );

    broker
        .take_focus(&stream, second.subscriber())
        .expect("focus is transferable");
    broker
        .input(&stream, second.subscriber(), b"echo hi\n")
        .expect("the new owner may type");
    assert_eq!(
        broker
            .input(&stream, first.subscriber(), b"nope\n")
            .expect_err("the old owner lost input")
            .code(),
        "terminal.input_not_owned",
    );
    assert_eq!(
        control.writes(),
        vec![
            (TEST_PTY, b"ls\n".to_vec()),
            (TEST_PTY, b"echo hi\n".to_vec()),
        ],
    );
}

#[test]
fn reconnect_after_replay_window_gets_snapshot_not_partial_output() {
    let (broker, _control) = broker();
    let stream = broker.open_stream(TEST_PTY);

    broker.on_output(TEST_PTY, b"OLDEST-MARKER\n");
    let (early, _early_events) = broker.attach(&stream, None).expect("attach");
    let acknowledged = match early.replay() {
        Replay::Snapshot { sequence, .. } => *sequence,
        Replay::Delta { .. } => panic!("a first attach must be given a snapshot"),
    };
    broker.detach(&stream, early.subscriber()).expect("detach");

    // A reconnect inside the window is a delta the client can append.
    broker.on_output(TEST_PTY, b"still-buffered\n");
    let (inside, _inside_events) = broker
        .attach(&stream, Some(acknowledged))
        .expect("attach inside the window");
    match inside.replay() {
        Replay::Delta { bytes, sequence } => {
            assert_eq!(bytes.as_slice(), b"still-buffered\n");
            assert_eq!(*sequence, acknowledged + 1);
        }
        Replay::Snapshot { .. } => panic!("output still in the ring must be served as a delta"),
    }
    broker.detach(&stream, inside.subscriber()).expect("detach");

    // Roll the ring well past the acknowledged sequence.
    let filler = vec![b'#'; 8192];
    for _ in 0..(2 * REPLAY_CAPACITY / filler.len()) {
        broker.on_output(TEST_PTY, &filler);
    }

    let (late, _late_events) = broker
        .attach(&stream, Some(acknowledged))
        .expect("attach after the window rolled");
    let Replay::Snapshot { bytes, sequence } = late.replay() else {
        panic!("a client whose sequence rolled out must be reset, not fed a partial delta");
    };
    assert!(
        *sequence > acknowledged,
        "the snapshot must carry the current sequence",
    );
    assert!(
        !bytes.windows(13).any(|w| w == b"OLDEST-MARKER"),
        "the snapshot must not resurrect output that left the ring",
    );
    assert!(
        bytes.len() <= REPLAY_CAPACITY,
        "a snapshot is bounded by the replay capacity",
    );
}

#[test]
fn stream_and_attachment_never_carry_the_numeric_pty_id() {
    let (broker, _control) = broker();
    let stream = broker.open_stream(TEST_PTY);
    broker.on_output(TEST_PTY, b"hello\n");
    let (attachment, mut events) = broker.attach(&stream, None).expect("attach");
    broker.on_output(TEST_PTY, b"more\n");
    broker.on_exit(TEST_PTY, Some(0), None);

    let decoded = base64_url_decode(stream.as_str());
    assert_eq!(decoded.len(), 16, "a stream id is 128 opaque random bits");
    assert_ne!(
        broker.open_stream(0xDEAD_BEEE).as_str(),
        stream.as_str(),
        "two ptys must not share a stream id",
    );

    let mut encoded = serde_json::to_string(&attachment).expect("attachment serializes");
    for event in drain(&mut events) {
        encoded.push_str(&serde_json::to_string(&event).expect("event serializes"));
    }
    assert!(
        !encoded.contains(&TEST_PTY.to_string()),
        "the internal pty id must never cross the protocol boundary",
    );

    let mut cbor = Vec::new();
    ciborium::into_writer(&attachment, &mut cbor).expect("attachment encodes as CBOR");
    for bytes in [TEST_PTY.to_be_bytes(), TEST_PTY.to_le_bytes()] {
        assert!(
            !cbor.windows(4).any(|window| window == bytes),
            "the pty id leaked into the CBOR encoding",
        );
    }
}

#[test]
fn output_sequences_are_continuous_across_attachments() {
    let (broker, _control) = broker();
    let stream = broker.open_stream(TEST_PTY);

    for chunk in [b"one\n".as_slice(), b"two\n", b"three\n"] {
        broker.on_output(TEST_PTY, chunk);
    }
    let (attachment, mut events) = broker.attach(&stream, None).expect("attach");
    let Replay::Snapshot { bytes, sequence } = attachment.replay() else {
        panic!("expected a snapshot");
    };
    assert_eq!(bytes.as_slice(), b"one\ntwo\nthree\n");
    assert_eq!(*sequence, 3, "one sequence number per output chunk");

    broker.on_output(TEST_PTY, b"four\n");
    broker.on_output(TEST_PTY, b"five\n");
    let live: Vec<TerminalEvent> = drain(&mut events);
    let sequences: Vec<u64> = live
        .iter()
        .map(|event| match event {
            TerminalEvent::Output { sequence, .. } => *sequence,
            other => panic!("unexpected event {other:?}"),
        })
        .collect();
    assert_eq!(sequences, vec![4, 5], "live events continue the snapshot");

    // A client that acknowledged the snapshot gets exactly what it missed.
    let (resumed, _resumed_events) = broker.attach(&stream, Some(3)).expect("resume");
    assert_eq!(replay_bytes(resumed.replay()), b"four\nfive\n");
    // And one that acknowledged nothing is handed everything, still in order.
    let (fresh, _fresh_events) = broker.attach(&stream, None).expect("fresh");
    assert_eq!(
        replay_bytes(fresh.replay()),
        b"one\ntwo\nthree\nfour\nfive\n",
    );
}

#[test]
fn taking_focus_notifies_every_attached_client() {
    let (broker, _control) = broker();
    let stream = broker.open_stream(TEST_PTY);
    let (first, mut first_events) = broker.attach(&stream, None).expect("attach");
    let (second, mut second_events) = broker.attach(&stream, None).expect("attach");

    // A read-only attach changes nothing, so nobody is told anything yet.
    assert!(
        drain(&mut first_events).is_empty(),
        "a second attach must not be announced as a focus change",
    );
    assert!(drain(&mut second_events).is_empty());

    broker
        .take_focus(&stream, second.subscriber())
        .expect("take focus");
    let expected = TerminalEvent::FocusChanged {
        owner: Some(second.subscriber().clone()),
    };
    assert_eq!(drain(&mut first_events), vec![expected.clone()]);
    assert_eq!(
        drain(&mut second_events),
        vec![expected],
        "the client taking focus is told as well, not only the others",
    );
    // `Attachment::has_focus` is what was true at attach time; the broker is
    // the live answer, and it must now name the second client.
    assert!(first.has_focus());
    assert_eq!(
        broker.owner(&stream),
        Some(second.subscriber().clone()),
        "the broker must record the takeover, not just announce it",
    );
}

#[test]
fn detaching_the_owner_releases_focus() {
    let (broker, control) = broker();
    let stream = broker.open_stream(TEST_PTY);
    let (first, _first_events) = broker.attach(&stream, None).expect("attach");
    let (second, mut second_events) = broker.attach(&stream, None).expect("attach");

    broker.detach(&stream, first.subscriber()).expect("detach");
    assert_eq!(
        drain(&mut second_events)
            .into_iter()
            .filter(|event| matches!(event, TerminalEvent::FocusChanged { owner: None }))
            .count(),
        1,
        "the remaining client must be told the terminal is unowned",
    );
    assert_eq!(
        broker
            .input(&stream, second.subscriber(), b"still not mine\n")
            .expect_err("a reader does not inherit input")
            .code(),
        "terminal.input_not_owned",
    );
    assert_eq!(
        broker
            .input(&stream, first.subscriber(), b"gone\n")
            .expect_err("a detached client is no longer a subscriber")
            .code(),
        "terminal.unknown_subscriber",
    );

    // Unowned input is claimed by the next attach, exactly as the first one was.
    let (third, _third_events) = broker.attach(&stream, None).expect("attach");
    assert!(third.has_focus());
    broker
        .input(&stream, third.subscriber(), b"mine\n")
        .expect("the new owner may type");
    assert_eq!(control.writes(), vec![(TEST_PTY, b"mine\n".to_vec())]);
}

#[test]
fn resize_is_owned_the_same_way_input_is() {
    let (broker, control) = broker();
    let stream = broker.open_stream(TEST_PTY);
    let (first, _first_events) = broker.attach(&stream, None).expect("attach");
    let (second, _second_events) = broker.attach(&stream, None).expect("attach");
    let size = TerminalSize::try_new(100, 30).expect("valid size");

    assert_eq!(
        broker
            .resize(&stream, second.subscriber(), size)
            .expect_err("a reader must not reshape the owner's terminal")
            .code(),
        "terminal.input_not_owned",
    );
    broker
        .resize(&stream, first.subscriber(), size)
        .expect("the owner may resize");
    assert_eq!(control.resizes(), vec![(TEST_PTY, 100, 30)]);
}

#[test]
fn the_replay_buffer_stays_within_one_mebibyte() {
    let (broker, _control) = broker();
    let stream = broker.open_stream(TEST_PTY);
    let chunk = vec![b'x'; 8192];
    for _ in 0..(4 * REPLAY_CAPACITY / chunk.len()) {
        broker.on_output(TEST_PTY, &chunk);
    }

    let (attachment, _events) = broker.attach(&stream, None).expect("attach");
    let buffered = replay_bytes(attachment.replay()).len();
    assert!(
        buffered <= REPLAY_CAPACITY,
        "4 MiB of output left {buffered} bytes buffered",
    );
    assert!(
        buffered > REPLAY_CAPACITY / 2,
        "the ring dropped far more than it had to ({buffered} bytes kept)",
    );
}

#[test]
fn a_client_that_stops_reading_is_dropped_rather_than_queued_without_bound() {
    let (broker, _control) = broker();
    let stream = broker.open_stream(TEST_PTY);
    let (attachment, events) = broker.attach(&stream, None).expect("attach");
    drop(events); // a phone that went away without closing the socket

    for _ in 0..4096 {
        broker.on_output(TEST_PTY, b"unread\n");
    }
    assert_eq!(
        broker
            .input(&stream, attachment.subscriber(), b"hello\n")
            .expect_err("an unreachable subscriber must be dropped")
            .code(),
        "terminal.unknown_subscriber",
    );
}

#[test]
fn an_exit_reaches_every_attached_client() {
    let (broker, _control) = broker();
    let stream = broker.open_stream(TEST_PTY);
    let (_first, mut first_events) = broker.attach(&stream, None).expect("attach");
    let (_second, mut second_events) = broker.attach(&stream, None).expect("attach");

    broker.on_exit(TEST_PTY, None, Some("Killed"));
    let exited = TerminalEvent::Exited {
        code: None,
        signal: Some("Killed".to_string()),
    };
    assert!(drain(&mut first_events).contains(&exited));
    assert!(drain(&mut second_events).contains(&exited));
    assert_eq!(
        broker.open_stream(TEST_PTY).as_str(),
        stream.as_str(),
        "the same pty keeps its stream id while the stream is still known",
    );
}

#[test]
fn requests_against_an_unknown_stream_are_refused() {
    let (broker, _control) = broker();
    let stream = broker.open_stream(TEST_PTY);
    let (attachment, _events) = broker.attach(&stream, None).expect("attach");
    broker.close_stream(&stream);

    assert_eq!(
        broker.attach(&stream, None).expect_err("gone").code(),
        "terminal.unknown_stream",
    );
    assert_eq!(
        broker
            .input(&stream, attachment.subscriber(), b"x")
            .expect_err("gone")
            .code(),
        "terminal.unknown_stream",
    );
}

fn base64_url_decode(value: &str) -> Vec<u8> {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    URL_SAFE_NO_PAD
        .decode(value)
        .expect("a stream id is base64url without padding")
}

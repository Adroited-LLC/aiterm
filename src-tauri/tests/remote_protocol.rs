use aiterm_lib::remote::model::{ProtocolError, RemoteRequest, TerminalSize};
use serde::Serialize;

#[derive(Serialize)]
struct TestEnvelope<'a> {
    version: u16,
    request_id: u64,
    kind: &'a str,
    payload: &'a [u8],
}

fn cbor_request(version: u16, kind: &str, payload: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    ciborium::into_writer(
        &TestEnvelope {
            version,
            request_id: 42,
            kind,
            payload,
        },
        &mut bytes,
    )
    .expect("test envelope should encode");
    bytes
}

#[test]
fn rejects_unsupported_protocol_version() {
    let err = RemoteRequest::decode(&cbor_request(99, "session.list", b"{}"))
        .expect_err("an incompatible peer must not be accepted");

    assert_eq!(err.code(), "protocol.unsupported_version");
}

#[test]
fn rejects_an_operation_the_protocol_does_not_define() {
    let err = RemoteRequest::decode(&cbor_request(1, "filesystem.write_anywhere", b"{}"))
        .expect_err("unknown operations must fail closed");

    assert_eq!(err.code(), "protocol.unknown_request");
}

#[test]
fn accepts_a_known_request_without_rewriting_its_identity_or_payload() {
    let request = RemoteRequest::decode(&cbor_request(1, "session.list", b"phone"))
        .expect("a version-one list request should decode");

    assert_eq!(request.request_id(), 42);
    assert_eq!(request.kind(), "session.list");
    assert_eq!(request.payload(), b"phone");
}

#[test]
fn rejects_terminal_size_outside_bounds() {
    assert_eq!(
        TerminalSize::try_new(0, 24).unwrap_err(),
        ProtocolError::invalid_terminal_size()
    );
    assert_eq!(
        TerminalSize::try_new(513, 24).unwrap_err(),
        ProtocolError::invalid_terminal_size()
    );
    assert_eq!(
        TerminalSize::try_new(80, 0).unwrap_err(),
        ProtocolError::invalid_terminal_size()
    );
    assert_eq!(
        TerminalSize::try_new(80, 513).unwrap_err(),
        ProtocolError::invalid_terminal_size()
    );
}

#[test]
fn terminal_size_keeps_valid_dimensions() {
    let size = TerminalSize::try_new(80, 24).expect("ordinary terminal dimensions are valid");

    assert_eq!(size.cols(), 80);
    assert_eq!(size.rows(), 24);
}

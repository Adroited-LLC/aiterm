use serde::{Deserialize, Serialize};
use std::fmt;

use crate::terminal::MAX_SCREEN_FRAME_BYTES;

pub const PROTOCOL_VERSION: u16 = 1;
const MAX_TERMINAL_DIMENSION: u16 = 512;

const KNOWN_REQUESTS: &[&str] = &[
    "session.list",
    "session.preview",
    "session.open",
    "session.close",
    "session.delete",
    "session.fork",
    "session.stop",
    "agent.list",
    "agent.action",
    "tab.list",
    "tab.open",
    "tab.close",
    "terminal.attach",
    "terminal.input",
    "terminal.resize",
    "terminal.detach",
    "terminal.scrollback",
    // Taking input ownership is its own request because it is a deliberate act.
    // Attaching gives a second client a read-only view; only this says "I am
    // typing now", and the broker announces it to everyone else on the stream.
    "terminal.focus",
];

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestEnvelope {
    version: u16,
    request_id: u64,
    kind: String,
    payload: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteRequest {
    request_id: u64,
    kind: String,
    payload: Vec<u8>,
}

impl RemoteRequest {
    pub fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        let envelope: RequestEnvelope = ciborium::from_reader(bytes)
            .map_err(|_| ProtocolError::new("protocol.invalid_cbor", "invalid request envelope"))?;
        if envelope.version != PROTOCOL_VERSION {
            return Err(ProtocolError::new(
                "protocol.unsupported_version",
                "unsupported protocol version",
            ));
        }
        if !KNOWN_REQUESTS.contains(&envelope.kind.as_str()) {
            return Err(ProtocolError::new(
                "protocol.unknown_request",
                "unknown request kind",
            ));
        }
        Ok(Self {
            request_id: envelope.request_id,
            kind: envelope.kind,
            payload: envelope.payload,
        })
    }

    pub fn request_id(&self) -> u64 {
        self.request_id
    }

    pub fn kind(&self) -> &str {
        &self.kind
    }

    pub fn payload(&self) -> &[u8] {
        &self.payload
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RemoteEvent {
    pub version: u16,
    pub request_id: u64,
    pub kind: String,
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ProtocolError {
    code: &'static str,
    message: &'static str,
}

impl ProtocolError {
    fn new(code: &'static str, message: &'static str) -> Self {
        Self { code, message }
    }

    pub fn invalid_terminal_size() -> Self {
        Self::new(
            "terminal.invalid_size",
            "terminal dimensions must be between 1 and 512",
        )
    }

    pub fn frame_too_large() -> Self {
        Self::new(
            "protocol.frame_too_large",
            "terminal frame exceeds the one mebibyte limit",
        )
    }

    pub fn code(&self) -> &'static str {
        self.code
    }
}

/// Reject a serialized terminal frame before it reaches a remote transport.
pub fn validate_terminal_frame(frame: &[u8]) -> Result<(), ProtocolError> {
    if frame.len() > MAX_SCREEN_FRAME_BYTES {
        return Err(ProtocolError::frame_too_large());
    }
    Ok(())
}

/// Serialize a typed terminal frame and enforce its wire-size limit before it
/// can be handed to a remote sender.
pub fn encode_terminal_frame<T: Serialize>(frame: &T) -> Result<Vec<u8>, ProtocolError> {
    let mut bytes = Vec::new();
    ciborium::into_writer(frame, &mut bytes).map_err(|_| {
        ProtocolError::new("protocol.invalid_frame", "unable to encode terminal frame")
    })?;
    validate_terminal_frame(&bytes)?;
    Ok(bytes)
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ProtocolError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalSize {
    cols: u16,
    rows: u16,
}

impl TerminalSize {
    pub fn try_new(cols: u16, rows: u16) -> Result<Self, ProtocolError> {
        if !(1..=MAX_TERMINAL_DIMENSION).contains(&cols)
            || !(1..=MAX_TERMINAL_DIMENSION).contains(&rows)
        {
            return Err(ProtocolError::invalid_terminal_size());
        }
        Ok(Self { cols, rows })
    }

    pub fn cols(&self) -> u16 {
        self.cols
    }

    pub fn rows(&self) -> u16 {
        self.rows
    }
}

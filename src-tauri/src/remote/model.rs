use serde::{Deserialize, Serialize};
use std::fmt;

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
    "terminal.attach",
    "terminal.input",
    "terminal.resize",
    "terminal.detach",
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

    pub fn code(&self) -> &'static str {
        self.code
    }
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

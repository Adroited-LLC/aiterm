//! The small, transport-only protocol between an AITerm desktop connector
//! and an AITerm relay.
//!
//! Application TLS is already established by the phone with the desktop.
//! These frames carry those opaque bytes; they never contain decoded remote
//! requests, device credentials, terminal state, or session data.

use std::fmt;
use sha2::{Digest, Sha256};

const MAGIC: &[u8; 4] = b"ATRP";
const VERSION: u8 = 1;
const HEADER_BYTES: usize = 14;
pub const MAX_DATA_BYTES: usize = 64 * 1024;
pub const MAX_CLOSE_REASON_BYTES: usize = 1024;
pub const MAX_FRAME_BYTES: usize = HEADER_BYTES + MAX_DATA_BYTES;

/// The exact digest a phone authorizes when it grants a desktop a relay
/// route. Length-prefixing every variable field makes the statement
/// unambiguous across Rust and Android implementations.
pub fn enrollment_digest(
    control_origin: &str,
    route_id: &str,
    token_sha256: &[u8; 32],
    desktop_spki_sha256: &[u8; 32],
) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"aiterm-relay-enrollment-v1\0");
    update_field(&mut digest, control_origin.as_bytes());
    update_field(&mut digest, route_id.as_bytes());
    update_field(&mut digest, token_sha256);
    update_field(&mut digest, desktop_spki_sha256);
    digest.finalize().into()
}

fn update_field(digest: &mut Sha256, value: &[u8]) {
    digest.update((value.len() as u32).to_be_bytes());
    digest.update(value);
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Frame {
    Open { stream_id: u64 },
    Data { stream_id: u64, bytes: Vec<u8> },
    Close { stream_id: u64, reason: Vec<u8> },
    Ping,
    Pong,
}

impl Frame {
    pub fn encode(&self) -> Result<Vec<u8>, ProtocolError> {
        let (kind, stream_id, payload): (u8, u64, &[u8]) = match self {
            Self::Open { stream_id } => (1, *stream_id, &[]),
            Self::Data { stream_id, bytes } => {
                if bytes.is_empty() || bytes.len() > MAX_DATA_BYTES {
                    return Err(ProtocolError::new("relay data frame has an invalid size"));
                }
                (2, *stream_id, bytes)
            }
            Self::Close { stream_id, reason } => {
                if reason.len() > MAX_CLOSE_REASON_BYTES {
                    return Err(ProtocolError::new("relay close reason is too large"));
                }
                (3, *stream_id, reason)
            }
            Self::Ping => (4, 0, &[]),
            Self::Pong => (5, 0, &[]),
        };
        if kind <= 3 && stream_id == 0 {
            return Err(ProtocolError::new("relay stream id must be nonzero"));
        }
        let mut out = Vec::with_capacity(HEADER_BYTES + payload.len());
        out.extend_from_slice(MAGIC);
        out.push(VERSION);
        out.push(kind);
        out.extend_from_slice(&stream_id.to_be_bytes());
        out.extend_from_slice(payload);
        Ok(out)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        if bytes.len() < HEADER_BYTES || bytes.len() > MAX_FRAME_BYTES {
            return Err(ProtocolError::new("relay frame has an invalid size"));
        }
        if &bytes[..4] != MAGIC || bytes[4] != VERSION {
            return Err(ProtocolError::new("relay frame has an invalid header"));
        }
        let stream_id = u64::from_be_bytes(
            bytes[6..14]
                .try_into()
                .expect("the bounded relay header always has eight id bytes"),
        );
        let payload = &bytes[HEADER_BYTES..];
        match bytes[5] {
            1 if stream_id != 0 && payload.is_empty() => Ok(Self::Open { stream_id }),
            2 if stream_id != 0 && !payload.is_empty() && payload.len() <= MAX_DATA_BYTES => {
                Ok(Self::Data {
                    stream_id,
                    bytes: payload.to_vec(),
                })
            }
            3 if stream_id != 0 && payload.len() <= MAX_CLOSE_REASON_BYTES => Ok(Self::Close {
                stream_id,
                reason: payload.to_vec(),
            }),
            4 if stream_id == 0 && payload.is_empty() => Ok(Self::Ping),
            5 if stream_id == 0 && payload.is_empty() => Ok(Self::Pong),
            _ => Err(ProtocolError::new("relay frame kind or payload is invalid")),
        }
    }

    pub fn stream_id(&self) -> Option<u64> {
        match self {
            Self::Open { stream_id }
            | Self::Data { stream_id, .. }
            | Self::Close { stream_id, .. } => Some(*stream_id),
            Self::Ping | Self::Pong => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProtocolError(&'static str);

impl ProtocolError {
    fn new(message: &'static str) -> Self {
        Self(message)
    }
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for ProtocolError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_frame_round_trips() {
        let frames = [
            Frame::Open { stream_id: 7 },
            Frame::Data {
                stream_id: 7,
                bytes: vec![0, 1, 2, 255],
            },
            Frame::Close {
                stream_id: 7,
                reason: b"done".to_vec(),
            },
            Frame::Ping,
            Frame::Pong,
        ];
        for frame in frames {
            assert_eq!(Frame::decode(&frame.encode().unwrap()).unwrap(), frame);
        }
    }

    #[test]
    fn invalid_and_unbounded_frames_are_rejected() {
        assert!(Frame::decode(b"ATRP").is_err());
        assert!(Frame::Data {
            stream_id: 1,
            bytes: vec![]
        }
        .encode()
        .is_err());
        assert!(Frame::Data {
            stream_id: 1,
            bytes: vec![0; MAX_DATA_BYTES + 1]
        }
        .encode()
        .is_err());
        assert!(Frame::Open { stream_id: 0 }.encode().is_err());

        let mut unknown = Frame::Ping.encode().unwrap();
        unknown[5] = 99;
        assert!(Frame::decode(&unknown).is_err());
    }

    #[test]
    fn enrollment_digest_is_domain_and_route_bound() {
        let token = [7; 32];
        let desktop = [9; 32];
        let expected = enrollment_digest(
            "https://control.relay.example.com:8443",
            "desktop-1234",
            &token,
            &desktop,
        );
        assert_ne!(
            expected,
            enrollment_digest(
                "https://other.example.com:8443",
                "desktop-1234",
                &token,
                &desktop,
            )
        );
        assert_ne!(
            expected,
            enrollment_digest(
                "https://control.relay.example.com:8443",
                "desktop-5678",
                &token,
                &desktop,
            )
        );
    }
}

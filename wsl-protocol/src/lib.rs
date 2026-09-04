//! Private, versioned desktop/WSL byte stream. stdout contains protocol only.
use serde::{Deserialize, Serialize};
use std::io::{self, BufRead, Read, Write};

pub const VERSION: u32 = 1;
pub const MAX_FRAME: usize = 1024 * 1024;
pub const OUTPUT_WINDOW: usize = 256 * 1024;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    Start {
        version: u32,
        cols: u16,
        rows: u16,
        #[serde(default)]
        cwd: Option<String>,
        #[serde(default)]
        command: Option<String>,
    },
    Input {
        data: String,
    },
    Resize {
        cols: u16,
        rows: u16,
    },
    Ack {
        sequence: u64,
    },
    Close,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    Ready {
        version: u32,
        home: String,
        shell: String,
        pid: u32,
    },
    Output {
        sequence: u64,
        data: String,
    },
    Exit {
        code: Option<u32>,
        signal: Option<String>,
    },
    Error {
        message: String,
    },
}

pub fn read_frame<T: serde::de::DeserializeOwned>(
    reader: &mut impl BufRead,
) -> io::Result<Option<T>> {
    // Limit before allocation, including frames without a terminating newline.
    let mut bytes = Vec::new();
    let n = reader
        .take((MAX_FRAME + 1) as u64)
        .read_until(b'\n', &mut bytes)?;
    if n == 0 {
        return Ok(None);
    }
    if n > MAX_FRAME || bytes.last() != Some(&b'\n') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Invalid or oversized protocol frame",
        ));
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

pub fn write_frame(writer: &mut impl Write, value: &impl Serialize) -> io::Result<()> {
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(b'\n');
    if bytes.len() > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Protocol frame too large",
        ));
    }
    writer.write_all(&bytes)?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn refuses_unterminated_or_oversized_input() {
        assert!(read_frame::<Request>(&mut &b"{\"type\":\"close\"}"[..]).is_err());
        assert!(read_frame::<Request>(&mut vec![b'x'; MAX_FRAME + 1].as_slice()).is_err());
    }
    #[test]
    fn preserves_multiple_frames() {
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &Request::Close).unwrap();
        write_frame(&mut bytes, &Request::Ack { sequence: 7 }).unwrap();
        let mut reader = bytes.as_slice();
        assert!(matches!(
            read_frame(&mut reader).unwrap(),
            Some(Request::Close)
        ));
        assert!(matches!(
            read_frame(&mut reader).unwrap(),
            Some(Request::Ack { sequence: 7 })
        ));
        assert!(read_frame::<Request>(&mut reader).unwrap().is_none());
    }
}

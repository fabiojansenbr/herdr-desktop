//! Read-only mirror of the Herdr **endpoint generation 1** wire contract.
//!
//! Source of truth: `../herdr` at commit `03749ae3970a74e077fc16b9327bddfc957771c9`
//! (`src/protocol/wire.rs`, `src/protocol/endpoint.rs`, `src/input/model.rs`,
//! `src/api/schema/common.rs`, `src/config/model.rs`). Digests, license and the
//! extraction rules live in `PROVENANCE.md`.
//!
//! Rules: enum variant order and field order are frozen; new behaviour goes through
//! `EndpointControl` named messages, never through new variants. `tests/frozen_v1.rs`
//! compares this mirror against the engine's published bincode digests.

pub mod endpoint;
pub mod wire;

use std::io::{self, Read, Write};

use serde::{de::DeserializeOwned, Serialize};

/// Maximum allowed frame payload size (2 MiB), identical to the engine.
pub const MAX_FRAME_SIZE: usize = 2 * 1024 * 1024;

/// Length of the little-endian u32 length prefix.
const LENGTH_PREFIX_BYTES: usize = 4;

/// Framing errors, mirroring `protocol::FramingError`.
#[derive(Debug)]
pub enum FramingError {
    Oversized { claimed: usize, max: usize },
    Io(io::Error),
    Bincode(String),
    UnexpectedEof,
}

impl std::fmt::Display for FramingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FramingError::Oversized { claimed, max } => {
                write!(f, "frame size {claimed} exceeds maximum {max}")
            }
            FramingError::Io(e) => write!(f, "I/O error: {e}"),
            FramingError::Bincode(e) => write!(f, "bincode error: {e}"),
            FramingError::UnexpectedEof => write!(f, "unexpected end of stream"),
        }
    }
}

impl std::error::Error for FramingError {}

impl From<io::Error> for FramingError {
    fn from(e: io::Error) -> Self {
        FramingError::Io(e)
    }
}

/// Encodes a message with the engine's bincode configuration (`standard()`).
pub fn encode_message<M: Serialize>(msg: &M) -> Result<Vec<u8>, FramingError> {
    bincode::serde::encode_to_vec(msg, bincode::config::standard())
        .map_err(|e| FramingError::Bincode(e.to_string()))
}

/// Decodes a complete payload, enforcing that every byte is consumed.
pub fn decode_message<M: DeserializeOwned>(payload: &[u8]) -> Result<M, FramingError> {
    let (msg, consumed) = bincode::serde::decode_from_slice(payload, bincode::config::standard())
        .map_err(|e| FramingError::Bincode(e.to_string()))?;
    if consumed != payload.len() {
        return Err(FramingError::Bincode(format!(
            "decoded {consumed} bytes but payload length was {}; trailing bytes are not allowed",
            payload.len()
        )));
    }
    Ok(msg)
}

/// Writes `[u32 LE length][bincode payload]` and flushes.
pub fn write_message<W: Write, M: Serialize>(writer: &mut W, msg: &M) -> Result<(), FramingError> {
    let payload = encode_message(msg)?;
    let len = payload.len();
    if len > u32::MAX as usize {
        return Err(FramingError::Bincode(format!(
            "payload length {len} exceeds u32::MAX, would be truncated by length prefix"
        )));
    }
    writer.write_all(&(len as u32).to_le_bytes())?;
    writer.write_all(&payload)?;
    writer.flush()?;
    Ok(())
}

/// Reads one raw length-prefixed frame without decoding it.
pub fn read_frame<R: Read>(reader: &mut R, max_frame_size: usize) -> Result<Vec<u8>, FramingError> {
    let mut len_buf = [0u8; LENGTH_PREFIX_BYTES];
    read_exact_or_eof(reader, &mut len_buf)?;
    let claimed_len = u32::from_le_bytes(len_buf) as usize;
    if claimed_len > max_frame_size {
        return Err(FramingError::Oversized {
            claimed: claimed_len,
            max: max_frame_size,
        });
    }
    let mut payload = vec![0u8; claimed_len];
    read_exact_or_eof(reader, &mut payload)?;
    Ok(payload)
}

/// Reads and decodes one length-prefixed frame.
pub fn read_message<R: Read, M: DeserializeOwned>(
    reader: &mut R,
    max_frame_size: usize,
) -> Result<M, FramingError> {
    let payload = read_frame(reader, max_frame_size)?;
    decode_message(&payload)
}

/// Returns the leading enum tag of an encoded message (bincode varint, single byte below 251).
pub fn peek_tag(payload: &[u8]) -> Option<u32> {
    match payload.first() {
        Some(&b) if b < 251 => Some(u32::from(b)),
        Some(251) => payload
            .get(1..3)
            .map(|b| u32::from(u16::from_le_bytes([b[0], b[1]]))),
        Some(252) => payload
            .get(1..5)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])),
        _ => None,
    }
}

fn read_exact_or_eof<R: Read>(reader: &mut R, buf: &mut [u8]) -> Result<(), FramingError> {
    reader.read_exact(buf).map_err(|e| {
        if e.kind() == io::ErrorKind::UnexpectedEof {
            FramingError::UnexpectedEof
        } else {
            FramingError::Io(e)
        }
    })
}

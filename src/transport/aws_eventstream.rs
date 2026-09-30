//! Incremental parser for the AWS binary `EventStream` framing.
//!
//! Bedrock's converse-stream replies in frames, not SSE text:
//!
//! ```text
//! [total_len u32 BE][headers_len u32 BE][prelude_crc u32 BE][headers ...][payload ...][message_crc u32 BE]
//! ```
//!
//! Frames are fed raw body bytes as they arrive; partial frames are retained until more bytes
//! show up, and a CRC mismatch is an error rather than something to skip past, because a
//! corrupted stream is untrustworthy from that byte on. Only the header shapes this layer needs
//! are decoded; the `:event-type` / `:exception-type` names and the JSON payload belong to the
//! protocol above.
//!
//! [`decode_record`] turns a frame into the [`Record`] a stream hands over, `(event name, data)`:
//! exception frames arrive as `__exception:<type>`, since they carry no `:event-type` of their own,
//! and `ping` frames carry no output at all.

use super::record::Record;

/// A typed header value of an event-stream frame. Only the shapes the wire defines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum EventStreamValue {
  Bool(bool),
  Byte(u8),
  Short(i16),
  Int(i32),
  Long(i64),
  ByteArray(Vec<u8>),
  String(String),
  Timestamp(i64),
  Uuid([u8; 16]),
}

impl EventStreamValue {
  /// The string view of a header value, if it is a string.
  pub(super) fn as_str(&self) -> Option<&str> {
    match self {
      EventStreamValue::String(value) => Some(value),
      _ => None,
    }
  }
}

/// One decoded frame: headers plus the raw payload bytes.
#[derive(Debug, Clone)]
pub(super) struct EventStreamFrame {
  headers: Vec<(String, EventStreamValue)>,
  payload: Vec<u8>,
}

impl EventStreamFrame {
  /// Looks a string header up by name.
  fn get_string_header(&self, name: &str) -> Option<&str> {
    self.headers.iter().find(|(key, _)| key == name).and_then(|(_, value)| value.as_str())
  }
}

/// A framing-side limit was exceeded or a frame is malformed; the stream is no longer trustworthy.
#[derive(Debug, thiserror::Error)]
pub(super) enum EventStreamError {
  #[error("event-stream frame CRC mismatch")]
  CrcMismatch,
  #[error("event-stream frame exceeds {limit} bytes")]
  FrameTooLarge { limit: usize },
  #[error("malformed event-stream header value type {value_type}")]
  BadValueType { value_type: u8 },
  #[error("truncated event-stream header section")]
  TruncatedHeader,
  #[error("event-stream frame is too small for a prelude")]
  TruncatedPrelude,
}

/// One parser instance belongs to exactly one response body.
#[derive(Debug)]
pub(super) struct EventStreamParser {
  buffer: Vec<u8>,
  max_frame: usize,
}

const PRELUDE_LEN: usize = 4 + 4 + 4;
const MIN_FRAME: usize = PRELUDE_LEN + 4;

impl EventStreamParser {
  /// New parser with an explicit per-frame byte cap.
  pub(super) fn with_max_frame(max_frame: usize) -> Self {
    Self { buffer: Vec::new(), max_frame }
  }

  /// Feeds raw bytes; returns every complete frame, in arrival order. The parser must be
  /// abandoned after an error.
  pub(super) fn feed(&mut self, bytes: &[u8]) -> Result<Vec<EventStreamFrame>, EventStreamError> {
    self.buffer.extend_from_slice(bytes);
    let mut out = Vec::new();
    loop {
      if self.buffer.len() < MIN_FRAME {
        return Ok(out);
      }
      let total =
        u32::from_be_bytes([self.buffer[0], self.buffer[1], self.buffer[2], self.buffer[3]])
          as usize;
      if total < MIN_FRAME {
        return Err(EventStreamError::TruncatedPrelude);
      }
      if total > self.max_frame {
        return Err(EventStreamError::FrameTooLarge { limit: self.max_frame });
      }
      if self.buffer.len() < total {
        return Ok(out);
      }
      let frame = parse_frame(&self.buffer[..total])?;
      self.buffer.drain(..total);
      out.push(frame);
    }
  }

  /// Flushes end of stream: every byte must have belonged to a whole frame.
  pub(super) fn finish(&mut self) -> Result<(), EventStreamError> {
    if self.buffer.is_empty() { Ok(()) } else { Err(EventStreamError::TruncatedPrelude) }
  }
}

fn parse_frame(bytes: &[u8]) -> Result<EventStreamFrame, EventStreamError> {
  let total = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
  let headers_len = u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
  let prelude_crc = u32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
  let message_crc =
    u32::from_be_bytes([bytes[total - 4], bytes[total - 3], bytes[total - 2], bytes[total - 1]]);
  if crc32(&bytes[..8]) != prelude_crc {
    return Err(EventStreamError::CrcMismatch);
  }
  if crc32(&bytes[..total - 4]) != message_crc {
    return Err(EventStreamError::CrcMismatch);
  }
  let headers_end = PRELUDE_LEN + headers_len;
  if headers_end + 4 > total {
    return Err(EventStreamError::TruncatedHeader);
  }
  let headers = parse_headers(&bytes[PRELUDE_LEN..headers_end])?;
  let payload = bytes[headers_end..total - 4].to_vec();
  Ok(EventStreamFrame { headers, payload })
}

fn parse_headers(mut slice: &[u8]) -> Result<Vec<(String, EventStreamValue)>, EventStreamError> {
  let mut out = Vec::new();
  while !slice.is_empty() {
    let name_len = slice[0] as usize;
    if slice.len() < 1 + name_len + 1 {
      return Err(EventStreamError::TruncatedHeader);
    }
    let name = String::from_utf8_lossy(&slice[1..=name_len]).into_owned();
    let value_type = slice[1 + name_len];
    slice = &slice[1 + name_len + 1..];
    let value = match value_type {
      0 => EventStreamValue::Bool(true),
      1 => EventStreamValue::Bool(false),
      2 => EventStreamValue::Byte(take_array::<1>(&mut slice)?[0]),
      3 => EventStreamValue::Short(i16::from_be_bytes(take_array(&mut slice)?)),
      4 => EventStreamValue::Int(i32::from_be_bytes(take_array(&mut slice)?)),
      5 => EventStreamValue::Long(i64::from_be_bytes(take_array(&mut slice)?)),
      6 => EventStreamValue::ByteArray(take_sized(&mut slice)?.to_vec()),
      7 => EventStreamValue::String(String::from_utf8_lossy(take_sized(&mut slice)?).into_owned()),
      8 => EventStreamValue::Timestamp(i64::from_be_bytes(take_array(&mut slice)?)),
      9 => EventStreamValue::Uuid(take_array(&mut slice)?),
      other => return Err(EventStreamError::BadValueType { value_type: other }),
    };
    out.push((name, value));
  }
  Ok(out)
}

/// The next `len` bytes of a header section, or `None` when fewer are left.
fn take<'a>(slice: &mut &'a [u8], len: usize) -> Option<&'a [u8]> {
  if slice.len() < len {
    return None;
  }
  let (head, tail) = slice.split_at(len);
  *slice = tail;
  Some(head)
}

/// The next `N` bytes of a header section, as the fixed-width value they encode.
fn take_array<const N: usize>(slice: &mut &[u8]) -> Result<[u8; N], EventStreamError> {
  let bytes = take(slice, N).ok_or(EventStreamError::TruncatedHeader)?;
  Ok(bytes.try_into().expect("take returns exactly N bytes"))
}

/// A variable-width value of a header section: a big-endian `u16` length, then that many bytes.
fn take_sized<'a>(slice: &mut &'a [u8]) -> Result<&'a [u8], EventStreamError> {
  let len = u16::from_be_bytes(take_array(slice)?) as usize;
  take(slice, len).ok_or(EventStreamError::TruncatedHeader)
}

/// CRC-32 (IEEE 802.3, the zlib polynomial), as the event-stream spec defines it.
fn crc32(bytes: &[u8]) -> u32 {
  let mut crc = 0xFFFF_FFFF_u32;
  for &byte in bytes {
    crc ^= u32::from(byte);
    for _ in 0..8 {
      if crc & 1 != 0 {
        crc = (crc >> 1) ^ 0xEDB8_8320;
      } else {
        crc >>= 1;
      }
    }
  }
  !crc
}

/// The record a frame becomes; `ping` frames carry no output and are dropped, and so is a frame
/// that names no event at all.
pub(super) fn decode_record(frame: EventStreamFrame) -> Option<Record> {
  if frame.get_string_header(":message-type") == Some("exception") {
    let exception = frame.get_string_header(":exception-type").unwrap_or("UnknownException");
    return Some(Record {
      event: Some(format!("__exception:{exception}")),
      data: String::from_utf8_lossy(&frame.payload).into_owned(),
    });
  }
  let event = frame.get_string_header(":event-type")?.to_owned();
  if event == "ping" {
    return None;
  }
  Some(Record { event: Some(event), data: String::from_utf8_lossy(&frame.payload).into_owned() })
}

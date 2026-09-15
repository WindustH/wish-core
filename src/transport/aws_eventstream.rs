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
//! [`BedrockStream`] is the driver around the parser: it reads a [`ReplyStream`], bounds events
//! like the SSE driver does, and hands each payload over as `(event name, data)` - exception
//! frames arrive as `__exception:<type>`, since they carry no `:event-type` of their own.

use std::collections::VecDeque;

use super::payload_error;
use crate::protocol::error::Error;
use crate::protocol::wire::ReplyStream;

/// A typed header value of an event-stream frame. Only the shapes the wire defines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventStreamValue {
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
  pub fn as_str(&self) -> Option<&str> {
    match self {
      EventStreamValue::String(value) => Some(value),
      _ => None,
    }
  }
}

/// One decoded frame: headers plus the raw payload bytes.
#[derive(Debug, Clone)]
pub struct EventStreamFrame {
  pub headers: Vec<(String, EventStreamValue)>,
  pub payload: Vec<u8>,
}

impl EventStreamFrame {
  /// Looks a header up by name.
  pub fn header(&self, name: &str) -> Option<&EventStreamValue> {
    self.headers.iter().find(|(key, _)| key == name).map(|(_, value)| value)
  }

  /// The `:message-type` header, if present.
  pub fn message_type(&self) -> Option<&str> {
    self.header(":message-type").and_then(EventStreamValue::as_str)
  }

  /// The `:event-type` header, if present.
  pub fn event_type(&self) -> Option<&str> {
    self.header(":event-type").and_then(EventStreamValue::as_str)
  }

  /// The `:exception-type` header, if present.
  pub fn exception_type(&self) -> Option<&str> {
    self.header(":exception-type").and_then(EventStreamValue::as_str)
  }
}

/// A framing-side limit was exceeded or a frame is malformed; the stream is no longer trustworthy.
#[derive(Debug, thiserror::Error)]
pub enum EventStreamError {
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
pub struct EventStreamParser {
  buffer: Vec<u8>,
  max_frame: usize,
}

const PRELUDE_LEN: usize = 4 + 4 + 4;
const MIN_FRAME: usize = PRELUDE_LEN + 4;

impl EventStreamParser {
  /// New parser with an explicit per-frame byte cap.
  pub fn with_max_frame(max_frame: usize) -> Self {
    Self { buffer: Vec::new(), max_frame }
  }

  /// Feeds raw bytes; returns every complete frame, in arrival order. The parser must be
  /// abandoned after an error.
  pub fn feed(&mut self, bytes: &[u8]) -> Result<Vec<EventStreamFrame>, EventStreamError> {
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
  pub fn finish(&mut self) -> Result<(), EventStreamError> {
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
      2 => {
        if slice.is_empty() {
          return Err(EventStreamError::TruncatedHeader);
        }
        let value = slice[0];
        slice = &slice[1..];
        EventStreamValue::Byte(value)
      }
      3 => {
        if slice.len() < 2 {
          return Err(EventStreamError::TruncatedHeader);
        }
        let value = i16::from_be_bytes([slice[0], slice[1]]);
        slice = &slice[2..];
        EventStreamValue::Short(value)
      }
      4 => {
        if slice.len() < 4 {
          return Err(EventStreamError::TruncatedHeader);
        }
        let value = i32::from_be_bytes([slice[0], slice[1], slice[2], slice[3]]);
        slice = &slice[4..];
        EventStreamValue::Int(value)
      }
      5 => {
        if slice.len() < 8 {
          return Err(EventStreamError::TruncatedHeader);
        }
        let value = i64::from_be_bytes([
          slice[0], slice[1], slice[2], slice[3], slice[4], slice[5], slice[6], slice[7],
        ]);
        slice = &slice[8..];
        EventStreamValue::Long(value)
      }
      6 => {
        let Some(raw) = take(&mut slice, 2) else {
          return Err(EventStreamError::TruncatedHeader);
        };
        let len = u16::from_be_bytes([raw[0], raw[1]]) as usize;
        let Some(raw) = take(&mut slice, len) else {
          return Err(EventStreamError::TruncatedHeader);
        };
        EventStreamValue::ByteArray(raw.to_vec())
      }
      7 => {
        let Some(raw) = take(&mut slice, 2) else {
          return Err(EventStreamError::TruncatedHeader);
        };
        let len = u16::from_be_bytes([raw[0], raw[1]]) as usize;
        let Some(raw) = take(&mut slice, len) else {
          return Err(EventStreamError::TruncatedHeader);
        };
        EventStreamValue::String(String::from_utf8_lossy(raw).into_owned())
      }
      8 => {
        if slice.len() < 8 {
          return Err(EventStreamError::TruncatedHeader);
        }
        let value = i64::from_be_bytes([
          slice[0], slice[1], slice[2], slice[3], slice[4], slice[5], slice[6], slice[7],
        ]);
        slice = &slice[8..];
        EventStreamValue::Timestamp(value)
      }
      9 => {
        if slice.len() < 16 {
          return Err(EventStreamError::TruncatedHeader);
        }
        let mut value = [0u8; 16];
        value.copy_from_slice(&slice[..16]);
        slice = &slice[16..];
        EventStreamValue::Uuid(value)
      }
      other => return Err(EventStreamError::BadValueType { value_type: other }),
    };
    out.push((name, value));
  }
  Ok(out)
}

fn take<'a>(slice: &mut &'a [u8], len: usize) -> Option<&'a [u8]> {
  if slice.len() < len {
    return None;
  }
  let (head, tail) = slice.split_at(len);
  *slice = tail;
  Some(head)
}

/// CRC-32 (IEEE 802.3, the zlib polynomial), as the event-stream spec defines it.
pub fn crc32(bytes: &[u8]) -> u32 {
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

/// One payload record of a bedrock stream: the frame's event name and its payload as text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BedrockRecord {
  /// `:event-type`, or `__exception:<type>` for exception frames.
  pub event: String,
  /// The payload, decoded lossily: exception payloads are not guaranteed to be JSON.
  pub data: String,
}

/// One open response body, read as event-stream frames.
///
/// Bounded like the SSE driver: the event ceiling is checked here because only framing knows
/// where an event ends.
pub struct BedrockStream<S: ReplyStream> {
  reply: S,
  parser: EventStreamParser,
  pending: VecDeque<BedrockRecord>,
  max_events: u64,
  events: u64,
  exhausted: bool,
}

impl<S: ReplyStream> BedrockStream<S> {
  /// Reads `reply` as event-stream frames, bounded by the limits the reply was opened with.
  pub fn new(reply: S) -> Self {
    let limits = reply.limits();
    Self {
      reply,
      parser: EventStreamParser::with_max_frame(limits.max_event_bytes),
      pending: VecDeque::new(),
      max_events: limits.max_stream_events,
      events: 0,
      exhausted: false,
    }
  }

  /// Status of the reply being streamed.
  pub fn status(&self) -> u16 {
    self.reply.status()
  }

  /// `retry-after` of the reply head, in milliseconds.
  pub fn retry_after_ms(&self) -> Option<u64> {
    self.reply.retry_after_ms()
  }

  /// The headers of the reply head, as received.
  pub fn headers(&self) -> &[(String, String)] {
    self.reply.headers()
  }

  /// Whether that status is in the `2xx` range.
  pub fn is_success(&self) -> bool {
    self.reply.is_success()
  }

  /// Best-effort body of a non-`2xx` reply, for the layer that classifies it.
  pub async fn error_body(&mut self) -> Vec<u8> {
    self.reply.error_body().await
  }

  /// The next payload record, or `None` at the end of the body.
  pub async fn next(&mut self) -> Result<Option<BedrockRecord>, Error> {
    loop {
      if let Some(record) = self.pending.pop_front() {
        self.events += 1;
        if self.events > self.max_events {
          return Err(payload_error(&format!("stream exceeds {} events", self.max_events)));
        }
        return Ok(Some(record));
      }
      if self.exhausted {
        return Ok(None);
      }
      // Framing failed: the bytes are no longer trustworthy, so the stream is over either way.
      match self.reply.next().await? {
        Some(chunk) => {
          let frames =
            self.parser.feed(&chunk).map_err(|error| payload_error(&error.to_string()))?;
          self.pending.extend(frames.into_iter().filter_map(record_of));
        }
        None => {
          self.exhausted = true;
          self.parser.finish().map_err(|error| payload_error(&error.to_string()))?;
        }
      }
    }
  }
}

/// The record a frame becomes; `ping` frames carry no output and are dropped.
fn record_of(frame: EventStreamFrame) -> Option<BedrockRecord> {
  if frame.message_type() == Some("exception") {
    let exception = frame.exception_type().unwrap_or("UnknownException");
    return Some(BedrockRecord {
      event: format!("__exception:{exception}"),
      data: String::from_utf8_lossy(&frame.payload).into_owned(),
    });
  }
  let event = frame.event_type()?.to_owned();
  if event == "ping" {
    return None;
  }
  Some(BedrockRecord { event, data: String::from_utf8_lossy(&frame.payload).into_owned() })
}

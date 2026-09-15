//! Incremental, byte-level SSE parser.
//!
//! Feeds raw bytes as they arrive from a response body and emits records for complete frames.
//! Partial lines are retained until more bytes show up, so nothing ever blocks on a buffer
//! boundary. Text is decoded lossily: a corrupted byte must not abort an otherwise valid stream.
//!
//! [`SseStream`] is the driver around the parser: it reads a [`ReplyStream`], drops comments, caps
//! how many events one attempt may produce, and flushes a trailing event when a body ends without
//! the blank line that would normally close it.

use std::collections::VecDeque;

use super::payload_error;
use crate::protocol::error::Error;
use crate::protocol::wire::ReplyStream;

/// A parsed SSE record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SseEvent {
  /// A dispatch record: `data:` payload (multi-line `data:` joined with `\n`), optional `event:`.
  Dispatch { event: Option<String>, data: String, id: Option<String> },
  /// A comment line (`:`-prefixed). Heartbeats land here.
  Comment(String),
}

/// A parser-side limit was exceeded; the stream is no longer trustworthy.
#[derive(Debug, thiserror::Error)]
pub enum SseError {
  #[error("sse event exceeds {limit} bytes")]
  EventTooLarge { limit: usize },
}

/// One parser instance belongs to exactly one response body.
#[derive(Debug)]
pub struct SseParser {
  buffer: Vec<u8>,
  pending_event: Option<String>,
  pending_id: Option<String>,
  pending_data: String,
  has_pending_data: bool,
  /// Raw bytes charged to the record currently being accumulated. Ignored fields and comments
  /// count too: the cap bounds one boundary-less span rather than trying to predict allocator
  /// overhead from retained values alone.
  pending_bytes: usize,
  /// Byte cap for a single accumulated record.
  max_event_bytes: usize,
}

impl SseParser {
  /// New parser with an explicit per-record byte cap.
  pub fn with_max_event_bytes(max_event_bytes: usize) -> Self {
    Self {
      buffer: Vec::new(),
      pending_event: None,
      pending_id: None,
      pending_data: String::new(),
      has_pending_data: false,
      pending_bytes: 0,
      max_event_bytes,
    }
  }

  /// Feeds raw bytes; returns the records they completed, in arrival order.
  ///
  /// An over-cap record returns [`SseError::EventTooLarge`], after which the parser must be
  /// abandoned.
  pub fn feed(&mut self, chunk: &[u8]) -> Result<Vec<SseEvent>, SseError> {
    self.buffer.extend_from_slice(chunk);
    let mut out = Vec::new();
    let mut consumed = 0;
    while let Some(relative_nl) = self.buffer[consumed..].iter().position(|&b| b == b'\n') {
      let nl = consumed + relative_nl;
      let line_end = if nl > consumed && self.buffer[nl - 1] == b'\r' { nl - 1 } else { nl };
      // Charge the line (including any CR) plus its LF terminator BEFORE allocating the string,
      // so an over-cap line is never materialized.
      self.charge_line(nl - consumed)?;
      let line = String::from_utf8_lossy(&self.buffer[consumed..line_end]).into_owned();
      consumed = nl + 1;
      self.handle_line(&line, &mut out)?;
    }
    if consumed > 0 {
      self.buffer.drain(..consumed);
    }
    // Whatever is left is an unterminated partial line and belongs to the record being
    // accumulated, so the cap has to cover it: without this check a newline-less upstream grows
    // the buffer without bound.
    self.enforce_cap()?;
    Ok(out)
  }

  /// Flushes end of stream: a trailing record without its blank line is still dispatched (lenient
  /// tail handling for truncated streams). An over-cap tail is an explicit error, never a silent
  /// drop.
  pub fn finish(&mut self) -> Result<Vec<SseEvent>, SseError> {
    let mut out = Vec::new();
    self.enforce_cap()?;
    if !self.buffer.is_empty() {
      let tail_len = self.buffer.len();
      self.charge_line(tail_len.saturating_sub(1))?;
      let line = String::from_utf8_lossy(&self.buffer).into_owned();
      self.buffer.clear();
      self.handle_line(&line, &mut out)?;
    }
    self.flush_pending(&mut out);
    Ok(out)
  }

  /// Charged bytes plus the unterminated bytes still in the buffer, which belong to the same
  /// accumulating record and have not been charged yet. Nothing can be double-charged: a line is
  /// charged exactly once, as it leaves the buffer.
  fn enforce_cap(&self) -> Result<(), SseError> {
    let charged = self.pending_bytes.saturating_add(self.buffer.len());
    if charged > self.max_event_bytes {
      return Err(SseError::EventTooLarge { limit: self.max_event_bytes });
    }
    Ok(())
  }

  /// Charges one received line: its raw byte length plus one terminator.
  fn charge_line(&mut self, line_len: usize) -> Result<(), SseError> {
    let next = self.pending_bytes.saturating_add(line_len).saturating_add(1);
    if next > self.max_event_bytes {
      return Err(SseError::EventTooLarge { limit: self.max_event_bytes });
    }
    self.pending_bytes = next;
    Ok(())
  }

  fn handle_line(&mut self, line: &str, out: &mut Vec<SseEvent>) -> Result<(), SseError> {
    if line.is_empty() {
      // A blank line is the record boundary.
      self.flush_pending(out);
      return Ok(());
    }
    let (field, value) = match line.split_once(':') {
      Some((field, rest)) => {
        let value = rest.strip_prefix(' ').unwrap_or(rest);
        (field, value)
      }
      None => (line, ""),
    };
    match field {
      "data" => {
        if self.has_pending_data {
          self.pending_data.push('\n');
        }
        self.pending_data.push_str(value);
        self.has_pending_data = true;
      }
      "event" => self.pending_event = Some(value.to_owned()),
      "id" => self.pending_id = Some(value.to_owned()),
      "" => out.push(SseEvent::Comment(value.to_owned())),
      _ => {
        // Unknown field names (including `retry`) are ignored.
      }
    }
    Ok(())
  }

  fn flush_pending(&mut self, out: &mut Vec<SseEvent>) {
    if !self.has_pending_data && self.pending_event.is_none() && self.pending_id.is_none() {
      // Even a boundary with nothing to dispatch closes the charged record.
      self.pending_bytes = 0;
      return;
    }
    let event = SseEvent::Dispatch {
      event: self.pending_event.take(),
      data: std::mem::take(&mut self.pending_data),
      id: self.pending_id.take(),
    };
    out.push(event);
    self.has_pending_data = false;
    self.pending_bytes = 0;
  }
}

/// One open response body, read as SSE.
///
/// The event ceiling is checked here rather than in the byte stream: only framing knows where an
/// event ends. Comments (keep-alives) are dropped: no protocol reads them, and they must not count
/// against the ceiling either.
pub struct SseStream<S: ReplyStream> {
  reply: S,
  parser: SseParser,
  pending: VecDeque<SseEvent>,
  max_events: u64,
  events: u64,
  exhausted: bool,
}

impl<S: ReplyStream> SseStream<S> {
  /// Reads `reply` as SSE, bounded by the limits the reply was opened with.
  pub fn new(reply: S) -> Self {
    let limits = reply.limits();
    Self {
      reply,
      parser: SseParser::with_max_event_bytes(limits.max_event_bytes),
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

  /// The next dispatch record, or `None` at the end of the body.
  pub async fn next(&mut self) -> Result<Option<SseEvent>, Error> {
    loop {
      if let Some(event) = self.pending.pop_front() {
        if matches!(event, SseEvent::Comment(_)) {
          continue;
        }
        self.events += 1;
        if self.events > self.max_events {
          return Err(payload_error(&format!("stream exceeds {} events", self.max_events)));
        }
        return Ok(Some(event));
      }
      if self.exhausted {
        return Ok(None);
      }
      // Framing failed: the bytes are no longer trustworthy, so the stream is over either way.
      match self.reply.next().await? {
        Some(chunk) => {
          let events =
            self.parser.feed(&chunk).map_err(|error| payload_error(&error.to_string()))?;
          self.pending.extend(events);
        }
        None => {
          self.exhausted = true;
          let events = self.parser.finish().map_err(|error| payload_error(&error.to_string()))?;
          self.pending.extend(events);
        }
      }
    }
  }
}

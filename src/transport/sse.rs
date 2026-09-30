//! Incremental, byte-level SSE parser.
//!
//! Feeds raw bytes as they arrive from a response body and emits a [`Record`] for each complete
//! dispatch: its `data:` payload (multi-line `data:` joined with `\n`) and its optional `event:`.
//! Partial lines are retained until more bytes show up, so nothing ever blocks on a buffer
//! boundary. Text is decoded lossily: a corrupted byte must not abort an otherwise valid stream.
//! Comments (`:`-prefixed lines, the heartbeats) are dropped here: no protocol reads them. An `id:`
//! is not kept either - no protocol resumes a stream - but a record that carries only an id is
//! still a record, as the framing says.

use super::record::Record;

/// A parser-side limit was exceeded; the stream is no longer trustworthy.
#[derive(Debug, thiserror::Error)]
pub(super) enum SseError {
  #[error("sse event exceeds {limit} bytes")]
  EventTooLarge { limit: usize },
}

/// One parser instance belongs to exactly one response body.
#[derive(Debug)]
pub(super) struct SseParser {
  buffer: Vec<u8>,
  pending_event: Option<String>,
  /// Whether the record being accumulated named an `id:`, which makes it a record to dispatch
  /// even without data.
  has_pending_id: bool,
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
  pub(super) fn with_max_event_bytes(max_event_bytes: usize) -> Self {
    Self {
      buffer: Vec::new(),
      pending_event: None,
      has_pending_id: false,
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
  pub(super) fn feed(&mut self, chunk: &[u8]) -> Result<Vec<Record>, SseError> {
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
  pub(super) fn finish(&mut self) -> Result<Vec<Record>, SseError> {
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

  fn handle_line(&mut self, line: &str, out: &mut Vec<Record>) -> Result<(), SseError> {
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
      "id" => self.has_pending_id = true,
      _ => {
        // Comments, and unknown field names (including `retry`), are ignored.
      }
    }
    Ok(())
  }

  fn flush_pending(&mut self, out: &mut Vec<Record>) {
    if !self.has_pending_data && self.pending_event.is_none() && !self.has_pending_id {
      // Even a boundary with nothing to dispatch closes the charged record.
      self.pending_bytes = 0;
      return;
    }
    out.push(Record {
      event: self.pending_event.take(),
      data: std::mem::take(&mut self.pending_data),
    });
    self.has_pending_id = false;
    self.has_pending_data = false;
    self.pending_bytes = 0;
  }
}

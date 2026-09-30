//! A streamed body read record by record, whichever framing cuts it.
//!
//! Two framings reach this crate: SSE text, which most wires stream in, and the binary AWS
//! event-stream frames Bedrock streams in. Both cut a body into the same thing - an event name,
//! when the framing carries one, and a payload - so both are read through one [`RecordStream`]:
//! it feeds the body's chunks to the framing's parser, caps how many records one attempt may
//! produce, and flushes what the parser still holds when the body ends. What a record means is the
//! protocol decoder's business above this layer.

use std::collections::VecDeque;

use super::aws_eventstream::{EventStreamParser, decode_record};
use super::build_payload_error;
use super::sse::SseParser;
use crate::protocol::attempt::ReplyStream;
use crate::protocol::error::Error;

/// One record of a streamed body: the event name its framing gave it, if any, and its payload as
/// text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
  /// SSE's `event:` field, or an event-stream frame's `:event-type` (`__exception:<type>` for an
  /// exception frame, which carries none of its own).
  pub event: Option<String>,
  /// The payload: SSE's `data:` lines joined with `\n`, or a frame's payload decoded lossily.
  pub data: String,
}

/// How a streamed body is cut into records.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Framing {
  /// Server-sent events, the text framing.
  Sse,
  /// AWS event-stream frames, the binary framing Bedrock streams in.
  AwsEventStream,
}

/// The parser of one framing, for one body.
enum Parser {
  Sse(SseParser),
  AwsEventStream(EventStreamParser),
}

impl Parser {
  /// The records a chunk completed, in arrival order.
  fn feed(&mut self, chunk: &[u8]) -> Result<Vec<Record>, Error> {
    match self {
      Parser::Sse(parser) => {
        parser.feed(chunk).map_err(|error| build_payload_error(&error.to_string()))
      }
      Parser::AwsEventStream(parser) => parser
        .feed(chunk)
        .map(|frames| frames.into_iter().filter_map(decode_record).collect())
        .map_err(|error| build_payload_error(&error.to_string())),
    }
  }

  /// What the parser still holds at the end of the body: a trailing SSE record without its blank
  /// line, and nothing from an event stream, where every byte must have belonged to a whole frame.
  fn finish(&mut self) -> Result<Vec<Record>, Error> {
    match self {
      Parser::Sse(parser) => {
        parser.finish().map_err(|error| build_payload_error(&error.to_string()))
      }
      Parser::AwsEventStream(parser) => parser
        .finish()
        .map(|()| Vec::new())
        .map_err(|error| build_payload_error(&error.to_string())),
    }
  }
}

/// One open response body, read as records.
///
/// The record ceiling is checked here rather than in the byte stream: only framing knows where a
/// record ends. What the framing drops - SSE comments, the keep-alives, and event-stream `ping`s -
/// never becomes a record, so it never counts against the ceiling either.
pub struct RecordStream<S: ReplyStream> {
  reply: S,
  parser: Parser,
  pending: VecDeque<Record>,
  max_records: u64,
  records: u64,
  exhausted: bool,
}

impl<S: ReplyStream> RecordStream<S> {
  /// Reads `reply` in `framing`, bounded by the limits the reply was opened with.
  pub fn new(reply: S, framing: Framing) -> Self {
    let limits = reply.get_limits();
    let parser = match framing {
      Framing::Sse => Parser::Sse(SseParser::with_max_event_bytes(limits.max_event_bytes)),
      Framing::AwsEventStream => {
        Parser::AwsEventStream(EventStreamParser::with_max_frame(limits.max_event_bytes))
      }
    };
    Self {
      reply,
      parser,
      pending: VecDeque::new(),
      max_records: limits.max_stream_events,
      records: 0,
      exhausted: false,
    }
  }

  /// The next record, or `None` at the end of the body.
  pub async fn next(&mut self) -> Result<Option<Record>, Error> {
    loop {
      if let Some(record) = self.pending.pop_front() {
        self.records += 1;
        if self.records > self.max_records {
          return Err(build_payload_error(&format!("stream exceeds {} events", self.max_records)));
        }
        return Ok(Some(record));
      }
      if self.exhausted {
        return Ok(None);
      }
      // Framing failed: the bytes are no longer trustworthy, so the stream is over either way.
      match self.reply.next().await? {
        Some(chunk) => {
          let records = self.parser.feed(&chunk)?;
          self.pending.extend(records);
        }
        None => {
          self.exhausted = true;
          let records = self.parser.finish()?;
          self.pending.extend(records);
        }
      }
    }
  }
}

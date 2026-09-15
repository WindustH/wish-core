//! The attempt contract: what one call is, what one reply is, and what performs them.
//!
//! A protocol renders a [`Call`] - method, absolute URL, merged headers, serialized body - and hands
//! it to a [`Transport`], which performs exactly one attempt and hands back a [`Reply`]: status,
//! headers and untouched body bytes. Nothing is interpreted at this boundary; an error status is a
//! reply like any other, and the layer that knows the protocol classifies it.
//!
//! The streamed half is the same contract unbuffered: [`Transport::execute_stream`] opens one
//! attempt and its body arrives as a [`ReplyStream`], chunk by chunk. Where an event ends belongs to the framing above this layer, which only bounds time and
//! bytes.

use std::future::Future;
use std::time::Duration;

use crate::protocol::error::Error;

/// HTTP method. Providers only need these two.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
  Get,
  Post,
}

impl Method {
  /// Uppercase wire spelling.
  pub fn as_str(self) -> &'static str {
    match self {
      Method::Get => "GET",
      Method::Post => "POST",
    }
  }
}

/// One HTTP call, already rendered and merged by the layer above.
///
/// Method, absolute URL, merged headers (endpoint plus auth) and the serialized body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Call {
  pub method: Method,
  pub url: String,
  pub headers: Vec<(String, String)>,
  pub body: Vec<u8>,
}

/// One raw attempt reply: status, headers and the untouched body bytes.
///
/// Headers are kept because policy above needs them (`retry-after` and friends) without the
/// transport having to interpret anything.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reply {
  pub status: u16,
  pub headers: Vec<(String, String)>,
  pub body: Vec<u8>,
}

impl Reply {
  /// First value of a header, matched case-insensitively.
  pub fn header(&self, name: &str) -> Option<&str> {
    self
      .headers
      .iter()
      .find(|(key, _)| key.eq_ignore_ascii_case(name))
      .map(|(_, value)| value.as_str())
  }

  /// Whether the status is in the `2xx` range.
  pub fn is_success(&self) -> bool {
    (200..300).contains(&self.status)
  }

  /// `retry-after` as milliseconds, when the reply carries a usable value.
  pub fn retry_after_ms(&self) -> Option<u64> {
    parse_retry_after(self.header("retry-after")?)
  }
}

/// `retry-after` in milliseconds: both forms of RFC 9110 (delta-seconds and an HTTP-date) are
/// understood, and the delay is capped at one hour - nothing above that is worth waiting for, and
/// an upstream must not be able to park a caller for a day. A value in neither form is `None`.
pub(crate) fn parse_retry_after(raw: &str) -> Option<u64> {
  let raw = raw.trim();
  if let Ok(seconds) = raw.parse::<f64>()
    && seconds.is_finite()
    && seconds >= 0.0
  {
    return Some((seconds.min(3600.0) * 1000.0) as u64);
  }
  let retry_at = httpdate::parse_http_date(raw).ok()?;
  let delay = retry_at.duration_since(std::time::SystemTime::now()).unwrap_or_default();
  Some(delay.min(Duration::from_secs(3600)).as_millis() as u64)
}

/// Time and byte ceilings for one attempt.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
  /// TCP/TLS connect timeout.
  pub connect: Duration,
  /// Time until the response head arrives.
  pub first_byte: Duration,
  /// Largest gap tolerated between body chunks.
  pub idle: Duration,
  /// Wall-clock ceiling for the whole attempt.
  pub total: Duration,
  /// Cap on a successful response body.
  pub max_response_bytes: usize,
  /// Cap on an error body, which is only kept so the layer above can classify it.
  pub max_error_body_bytes: usize,
  /// Cap on one accumulated SSE event.
  pub max_event_bytes: usize,
  /// Cap on SSE events decoded from one attempt.
  pub max_stream_events: u64,
}

impl Default for Limits {
  /// One sane set of ceilings, so a caller with no opinion does not have to spell out eight numbers.
  ///
  /// These are this crate's own numbers, not borrowed defaults: ten seconds to connect, sixty for
  /// the response head, sixty of silence tolerated inside the body, five minutes for the attempt as
  /// a whole; 32 MiB of response body, 64 KiB of error body, 8 MiB per SSE event and 100k events
  /// per stream.
  fn default() -> Self {
    Self {
      connect: Duration::from_secs(10),
      first_byte: Duration::from_secs(60),
      idle: Duration::from_secs(60),
      total: Duration::from_secs(300),
      max_response_bytes: 32 * 1024 * 1024,
      max_error_body_bytes: 64 * 1024,
      max_event_bytes: 8 * 1024 * 1024,
      max_stream_events: 100_000,
    }
  }
}

/// One upstream body as it arrives, chunk by chunk.
///
/// The streamed half of the attempt contract: a transport opens a call and hands the body over as
/// the upstream produced it, with the ceilings the attempt was opened under. Nothing is buffered and
/// nothing is framed here; where an event ends belongs to the framing layer, which reads the same
/// ceilings off [`ReplyStream::limits`].
pub trait ReplyStream: Send {
  /// Status line as received, before any body byte.
  fn status(&self) -> u16;

  /// `retry-after` of the reply head, in milliseconds, when it carried a usable value.
  fn retry_after_ms(&self) -> Option<u64>;

  /// The headers of the reply head, as received.
  ///
  /// Kept because a service reports what a call left in its account only on a call it really
  /// served, and for several of them the headers are the only place it says so.
  fn headers(&self) -> &[(String, String)];

  /// Whether the status is in the `2xx` range.
  fn is_success(&self) -> bool {
    (200..300).contains(&self.status())
  }

  /// The ceilings this stream was opened with.
  fn limits(&self) -> Limits;

  /// The next chunk of the body, or `None` at its end.
  fn next(&mut self) -> impl Future<Output = Result<Option<Vec<u8>>, Error>> + Send;

  /// Best-effort body of a non-`2xx` stream, capped by `max_error_body_bytes`.
  ///
  /// Mirrors the buffered path: an error body exists to be classified, so failing to read it must
  /// not hide the status code, and a body that is not whole is dropped rather than half-kept.
  fn error_body(&mut self) -> impl Future<Output = Vec<u8>> + Send {
    async move {
      let cap = self.limits().max_error_body_bytes;
      let mut body = Vec::new();
      loop {
        match self.next().await {
          Ok(Some(chunk)) => {
            if body.len() + chunk.len() > cap {
              return Vec::new();
            }
            body.extend_from_slice(&chunk);
          }
          Ok(None) => return body,
          Err(_) => return Vec::new(),
        }
      }
    }
  }
}

/// Performs one attempt of one call.
///
/// The future is spelled out instead of `async fn` because a public trait with `async fn` trips the
/// `async_fn_in_trait` lint, and it documents the `Send` requirement that a multithreaded runtime
/// imposes. Implementations may still use `async fn execute(..)`.
pub trait Transport {
  /// The body of a streamed attempt, as this transport reads one.
  type Stream: ReplyStream;

  /// Performs one attempt of one call.
  fn execute(&self, call: &Call) -> impl Future<Output = Result<Reply, Error>> + Send;

  /// Opens one attempt and hands its body back as it arrives.
  ///
  /// A non-`2xx` reply is a stream too: the status is already known by then, and
  /// [`ReplyStream::error_body`] reads the bounded body so that the layer above can classify it
  /// exactly as it does for [`Reply`].
  fn execute_stream(&self, call: &Call)
  -> impl Future<Output = Result<Self::Stream, Error>> + Send;
}

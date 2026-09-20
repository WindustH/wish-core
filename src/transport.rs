//! The HTTP reader of the protocol layer's attempt contract.
//!
//! [`Call`](crate::protocol::wire::Call) and [`Reply`](crate::protocol::wire::Reply) belong to the
//! protocol layer; what lives here is the `reqwest` implementation of them, plus the two framings a
//! streamed body is read through: SSE text records, and bedrock's binary event-stream frames.
//!
//! One upstream attempt per call: no hidden retry in this layer, and classification of an error
//! reply left to whoever knows the protocol. Deliberately absent: retries, backoff, concurrency and
//! cancellation policy - they belong above this layer, because folding them in here is what turns an
//! executor into an unreadable one.

mod aws_eventstream;
mod error;
mod http;
mod sse;

pub use aws_eventstream::{
  BedrockRecord, BedrockStream, EventStreamError, EventStreamFrame, EventStreamParser,
  EventStreamValue,
};
pub use error::TransportError;
pub use http::{HttpBodyStream, ReqwestTransport};
pub use sse::{SseError, SseEvent, SseParser, SseStream};

use crate::protocol::error::Error;

/// Proxy policy for the HTTP client.
#[derive(Clone, Debug)]
pub enum Proxy {
  /// Whatever the client does by default.
  Environment,
  /// Never proxy.
  Disabled,
  /// One explicit proxy, optionally with basic auth.
  Manual { url: String, basic_auth: Option<(String, String)> },
}

/// Reading the body failed: the connection broke, or the attempt outlived its limits.
pub(super) fn build_read_error(message: &str) -> Error {
  TransportError::ReadBody(truncate(message)).into()
}

/// The body broke its own framing or crossed a ceiling.
///
/// The same call would do it again, so this is a dead end rather than something to retry: it is a
/// payload that does not fit our reading, not a network failure.
pub(super) fn build_payload_error(message: &str) -> Error {
  Error::Malformed(truncate(message))
}

/// Caps a message that came from outside.
pub(super) fn truncate(message: &str) -> String {
  message.chars().take(256).collect()
}

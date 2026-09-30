//! The HTTP reader of the protocol layer's attempt contract.
//!
//! [`Call`](crate::protocol::attempt::Call) and [`Reply`](crate::protocol::attempt::Reply) belong to the
//! protocol layer; what lives here is the `reqwest` implementation of them, plus the reading of a
//! streamed body as [`Record`](record::Record)s through one of its two framings: SSE text records, and Bedrock's
//! binary event-stream frames.
//!
//! One upstream attempt per call: no hidden retry in this layer, and classification of an error
//! reply left to whoever knows the protocol. Deliberately absent: retries, backoff, concurrency and
//! cancellation policy - they belong above this layer, because folding them in here is what turns an
//! executor into an unreadable one.

mod aws_eventstream;
mod error;
mod http;
mod record;
mod sse;

use error::TransportError;
pub use http::ReqwestTransport;
pub(crate) use http::apply_proxy;
pub use record::{Framing, RecordStream};

use crate::protocol::error::Error;

/// Proxy policy for the HTTP client.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Proxy {
  /// Whatever the client does by default.
  Environment,
  /// Never proxy.
  Disabled,
  /// One explicit proxy, optionally with basic auth.
  Manual { url: String, basic_auth: Option<(String, String)> },
}

/// Reading the body failed: the connection broke, or the attempt outlived its limits.
fn build_read_error(message: &str) -> Error {
  TransportError::ReadBody(cap_message(message)).into()
}

/// The body broke its own framing or crossed a ceiling.
///
/// The same call would do it again, so this is a dead end rather than something to retry: it is a
/// payload that does not fit our reading, not a network failure.
fn build_payload_error(message: &str) -> Error {
  Error::Malformed(cap_message(message))
}

/// Caps a message that came from outside, so an upstream cannot flood a log line with text of its
/// own choosing.
fn cap_message(message: &str) -> String {
  message.chars().take(256).collect()
}

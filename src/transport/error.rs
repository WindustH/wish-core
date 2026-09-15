//! What can go wrong in one attempt, in this layer's own words.
//!
//! The protocol layer defines the attempt contract and the failure vocabulary everything above
//! matches on; this module holds the network half of it, divided by how far the attempt got. That
//! division is also the retry judgment, and the judgment is not "was it the network": only a
//! failure before the request left deserves another attempt. Once the service has the call, the
//! upstream may already be generating - and billing - the answer a replay would ask for again, so
//! the failure is handed up instead of being retried behind the caller's back.
//!
//! A body that arrived but does not read (framing, ceilings) is not a network failure at all: it is
//! deterministic, and it lands in [`Error::Malformed`](crate::protocol::error::Error::Malformed).

use std::error::Error as StdError;
use std::fmt;

use crate::protocol::error::{Error, TransportFailure};

/// A failure of one attempt, in the phase of the attempt it happened in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransportError {
  /// Establishing TCP/TLS: nothing of the request reached the service.
  Connect(String),
  /// Waiting for the response head: the request did reach the service, and it has not answered yet.
  AwaitHeaders(String),
  /// Reading the body: the service started answering and the answer stopped arriving, or the attempt
  /// outlived the total limit.
  ReadBody(String),
}

impl TransportError {
  /// Whether sending the same call again could plausibly succeed *and* be safe.
  ///
  /// Only when nothing of the request left: a fresh attempt then costs nothing extra and is the one
  /// thing worth trying. A connect failure still has to be a *transient* one for the retry to be
  /// worth anything (a name that does not resolve will not start resolving unless time passes), but
  /// this layer cannot tell those apart from outside, so it considers the whole phase retryable and
  /// leaves how many attempts that is worth to the caller's policy.
  pub fn retryable(&self) -> bool {
    match self {
      TransportError::Connect(_) => true,
      TransportError::AwaitHeaders(_) | TransportError::ReadBody(_) => false,
    }
  }
}

impl fmt::Display for TransportError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    // The phase of the attempt, in words, then the report itself.
    let phase = match self {
      TransportError::Connect(_) => "connect",
      TransportError::AwaitHeaders(_) => "await headers",
      TransportError::ReadBody(_) => "read body",
    };
    let message = match self {
      TransportError::Connect(message)
      | TransportError::AwaitHeaders(message)
      | TransportError::ReadBody(message) => message,
    };
    write!(f, "{phase}: {message}")
  }
}

impl StdError for TransportError {}

impl From<TransportError> for Error {
  fn from(error: TransportError) -> Self {
    Error::Transport(TransportFailure { retryable: error.retryable(), message: error.to_string() })
  }
}

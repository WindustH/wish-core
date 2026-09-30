//! Errors, and the one place that decides whether they are worth retry.

/// Everything that can go wrong on the way to a reply, divided by where it came from: `Build` is a
/// request this crate refuses to render, `Unsupported` is a feature the protocol it was asked of
/// does not carry, `Malformed` is an upstream payload that does not fit its wire, `Upstream` is a
/// failure the service reported, `Transport` is a failure of the network itself, and `Renewal` is
/// credentials that had expired before the call could be sent. [`Error::is_retryable`] judges
/// whether a second attempt could help.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, thiserror::Error)]
pub enum Error {
  /// The request cannot be rendered for this protocol: an axis the wire has no spelling for, a
  /// mandatory field left out, or a conversation the wire forbids.
  #[error("request build failed: {0}")]
  Build(String),
  /// A feature was asked of a protocol that does not carry it: a read the client was never given
  /// a protocol for, an account state that rides replies instead of a request of its own, a
  /// compaction or a count a wire does not have, or a renewal a key cannot make. This is
  /// configuration, not failure: sending the same ask again cannot make the protocol grow the
  /// feature.
  #[error("unsupported {feature} on `{subject}`: {reason}")]
  Unsupported {
    /// The feature that was asked for, in the words its own module uses.
    feature: String,
    /// What it was asked of: the protocol, or the auth scheme, that cannot serve it.
    subject: String,
    /// Why the ask cannot be served, in the words the feature's own module states it.
    reason: String,
  },
  /// The upstream payload does not fit the protocol or this crate's reading of it: a body that is
  /// not the JSON it promised, a delta for a block that never opened, or a body that broke its own
  /// framing or crossed a byte ceiling before it could be read.
  #[error("malformed upstream payload: {0}")]
  Malformed(String),
  /// The service reported a failure, either with an HTTP status of its own or from inside a `2xx`
  /// body or stream event.
  #[error("{}", match code {
    Some(code) => format!("upstream error ({code}): {message}"),
    None => format!("upstream error: {message}"),
  })]
  Upstream {
    /// HTTP status of the reply the failure came from; `None` when the protocol reported it inside
    /// a `2xx` body or a stream event.
    status: Option<u16>,
    code: Option<String>,
    message: String,
    /// Upstream-provided `retry-after`, in milliseconds.
    retry_after_ms: Option<u64>,
  },
  /// The attempt itself failed: the layer that performs it reported a failure of the network, in
  /// its own words.
  #[error("transport failed: {0}")]
  Transport(TransportFailure),
  /// The credentials had already run out before anything was sent: the endpoint refused them,
  /// because their expiry had passed against the `now` the caller read. The move is a renewal
  /// ([`Client::refresh_credentials`](crate::client::Client::refresh_credentials))
  /// and credentials installed afresh - the same call over the same credentials will be refused
  /// the same way.
  #[error("credential renewal needed: the material expired at {expires_at}")]
  Renewal {
    /// The unix time the credentials stopped being accepted.
    expires_at: u64,
  },
}

impl Error {
  /// Explicit upstream context rejection, never inferred from a generic 400/413 response.
  pub fn is_context_length_exceeded(&self) -> bool {
    let Self::Upstream { status, code, message, .. } = self else {
      return false;
    };
    if code.as_deref().is_some_and(|code| {
      matches!(
        code,
        "context_length_exceeded" | "model_context_window_exceeded" | "prompt_too_long"
      )
    }) {
      return true;
    }
    if !matches!(status, None | Some(400) | Some(413)) {
      return false;
    }
    let message = message.to_ascii_lowercase();
    message.contains("prompt is too long")
      || message.contains("exceeds the maximum context length")
      || message.contains("exceeded model token limit")
      || (message.contains("input token count")
        && message.contains("exceeds")
        && message.contains("maximum"))
  }

  /// An ask a protocol cannot serve, with the reason its feature states.
  pub(crate) fn build_unsupported(
    feature: &'static str,
    subject: impl std::fmt::Display,
    reason: &'static str,
  ) -> Self {
    Self::Unsupported {
      feature: feature.into(),
      subject: subject.to_string(),
      reason: reason.into(),
    }
  }

  /// An upstream failure carried by an HTTP reply.
  pub(crate) fn from_http(status: u16, code: Option<String>, message: String) -> Self {
    Self::Upstream { status: Some(status), code, message, retry_after_ms: None }
  }

  /// An upstream failure the protocol reported itself: inside a `2xx` body or a stream event, with
  /// no HTTP status to judge it by.
  pub(crate) fn from_in_band(code: Option<String>, message: String) -> Self {
    Self::Upstream { status: None, code, message, retry_after_ms: None }
  }

  /// Whether sending the same call again could plausibly succeed.
  ///
  /// Only transient conditions qualify: a reply in the `429`/`5xx` range, or a transport failure
  /// the transport itself judged worth another attempt (it knows which phases of an attempt are
  /// safe to replay). A failure reported inside a successful reply, a body that does not fit the
  /// protocol and a request that could not be built are all deterministic.
  #[must_use]
  pub fn is_retryable(&self) -> bool {
    match self {
      Error::Build(_) | Error::Unsupported { .. } | Error::Malformed(_) | Error::Renewal { .. } => {
        false
      }
      Error::Transport(failure) => failure.retryable,
      Error::Upstream { status: Some(status), .. } => {
        *status == 429 || (500..=599).contains(status)
      }
      Error::Upstream { status: None, .. } => false,
    }
  }

  /// Upstream-provided `retry-after`, in milliseconds, when the failure carried one.
  #[must_use]
  pub fn get_retry_after_ms(&self) -> Option<u64> {
    match self {
      Error::Upstream { retry_after_ms, .. } => *retry_after_ms,
      _ => None,
    }
  }

  /// Attaches a reply's `retry-after` to an upstream error; a no-op for every other kind.
  pub(crate) fn with_retry_after(mut self, retry_after_ms: Option<u64>) -> Self {
    if let Error::Upstream { retry_after_ms: slot, .. } = &mut self {
      *slot = retry_after_ms;
    }
    self
  }
}

/// A transport failure as it crosses out of the transport layer.
///
/// The transport spells the phase of the attempt and the report in its own error type; what the
/// layers above need from a network failure is this, and no more: the report, and whether another
/// attempt could help. Keeping the crossing narrow is what lets the transport own its own failure
/// vocabulary while both layers still meet in one [`Error`].
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct TransportFailure {
  /// Whether sending the same call again could plausibly succeed.
  pub retryable: bool,
  /// The transport's report, phase included.
  pub message: String,
}

impl std::fmt::Display for TransportFailure {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.write_str(&self.message)
  }
}

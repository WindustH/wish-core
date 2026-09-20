//! Persisted observations of logical model calls. Counts belong to a call, not each message.
use crate::{
  protocol::{StopReason, Usage},
  session::GenerationId,
};

/// Unix time in milliseconds. Ordering is defined by history sequence numbers, not wall time.
#[derive(
  Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct Timestamp(pub u64);
impl Timestamp {
  pub fn now() -> Self {
    Self(
      std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64,
    )
  }
}

/// Local to one session; never reused, including across run invocations.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct ModelCallId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ModelCallStatus {
  Running,
  /// A complete protocol response was received; it need not have been accepted into context.
  Completed,
  Interrupted,
  Failed,
}

/// One logical call, including retries performed internally by ModelCaller.
/// A Running record after restart has an unknown outcome; it must not be automatically retried.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ModelCallRecord {
  pub id: ModelCallId,
  pub generation: GenerationId,
  pub model: String,
  pub stream: bool,
  /// Active-generation prefix used to construct the request.
  pub input_entry_count: u64,
  pub started_at: Timestamp,
  /// First decoded stream event, not the first network byte or necessarily first text token.
  /// Buffered responses do not have an event timestamp.
  pub first_event_at: Option<Timestamp>,
  pub finished_at: Option<Timestamp>,
  /// Monotonic duration of the caller invocation, including stream consumption and batch writes.
  pub elapsed_ms: Option<u64>,
  pub status: ModelCallStatus,
  /// Missing fields remain None. Cumulative stream updates replace previous usage.
  pub usage: Usage,
  pub stop_reason: Option<StopReason>,
}

#[derive(Default)]
pub(crate) struct CallObservation {
  pub first_event_at: Option<Timestamp>,
  pub finished_at: Option<Timestamp>,
  pub elapsed_ms: Option<u64>,
  pub usage: Usage,
  pub stop_reason: Option<StopReason>,
}

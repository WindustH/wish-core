use crate::{
  Error,
  protocol::{Response, model_use::stream::PartialResponse},
};
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub enum RunOutcome {
  Completed,
  Interrupted,
  /// Historical persisted outcome only. The runner no longer enforces a turn limit.
  #[doc(hidden)]
  #[serde(rename = "TurnLimit")]
  LegacyTurnLimit,
  /// The response was not accepted. The caller decides whether to continue or compact.
  ModelStopped(Box<Response>),
  Failed(Error),
  /// Failed stream with protocol-owned diagnostics; no partial messages were accepted.
  StreamFailed(Box<PartialResponse>),
  /// A tool may have caused a side effect; explicit caller reconciliation is required.
  ToolOutcomeUnknown,
}
impl RunOutcome {
  pub(crate) fn is_context_length_exceeded(&self) -> bool {
    match self {
      Self::Failed(error) => error.is_context_length_exceeded(),
      Self::ModelStopped(response) => {
        response.stop_reason == crate::protocol::StopReason::ContextLengthExceeded
      }
      Self::StreamFailed(partial) => match &partial.reason {
        crate::protocol::model_use::stream::IncompleteReason::Failed(error) => {
          error.is_context_length_exceeded()
        }
        _ => false,
      },
      _ => false,
    }
  }
}

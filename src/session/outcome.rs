use crate::{
  Error,
  protocol::{Response, model_use::stream::PartialResponse},
};
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub enum RunOutcome {
  Completed,
  Interrupted,
  TurnLimit,
  /// The response was not accepted. The caller decides whether to continue or compact.
  ModelStopped(Box<Response>),
  Failed(Error),
  /// Failed stream with protocol-owned diagnostics; no partial messages were accepted.
  StreamFailed(Box<PartialResponse>),
  /// A tool may have caused a side effect; explicit caller reconciliation is required.
  ToolOutcomeUnknown,
}

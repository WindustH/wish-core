//! Context compaction: the active context replaced by a smaller one, validated and switched to
//! atomically, leaving the complete history as it was.
//!
//! [`trigger`] decides when a cutover is due. A caller that compacts upstream hands the whole
//! context to its provider ([`upstream`]). Any other caller keeps a standby generation of
//! summaries, prepared beside the run's work ([`standby`]), and at cutover trims standby plus the
//! unsummarized tail to the target ([`trim`]). [`handoff`] writes the readable handoff an encrypted
//! compaction item carries for providers that cannot read it.
pub(crate) mod handoff;
mod standby;
mod trigger;
mod trim;
mod upstream;

pub(super) use standby::{StandbySummarizer, StandbyWait};
pub(super) use trigger::{CutoverPlan, find_cutover, force_cutover};

use super::{ExecutionControl, model::ModelCaller, observe::deliver_new_events, run::run_scoped};
use crate::{
  Error,
  protocol::{ContentBlock, Message, Request, Response, StopReason},
  session::{
    CompactionConfig, CompactionReason, RunOutcome, Session, SessionError, SessionEvent,
    TokenMeasurement, TokenMeasurementSource, statistics::ModelCallStatus,
  },
  storage::StorageError,
};

/// Compacts now (`Manual`), at a stable boundary, honoring the caller's cancellation. A failed or
/// interrupted compaction leaves the active context as it was and returns its outcome.
pub async fn compact(
  caller: &impl ModelCaller,
  session: &mut Session,
  control: &ExecutionControl,
  mut observe: impl FnMut(&SessionEvent) + Send,
) -> Result<RunOutcome, SessionError> {
  session.require_stable()?;
  if session.get_config().compaction.is_none() {
    return Err(SessionError::InvalidCompaction("no compaction budgets configured".into()));
  }
  let outcome = run_scoped(session, control, async |session, control| {
    let mut delivered = session.reader().get_history().len()?;
    force_cutover(caller, session, control, &mut delivered, &mut observe, CompactionReason::Manual)
      .await
  })
  .await?;
  Ok(outcome.unwrap_or(RunOutcome::Completed))
}

/// Replaces the active context inside `Compacting`: upstream when the caller compacts upstream,
/// otherwise by catching the standby up and trimming. A failure leaves the active context as it
/// was and finishes the run with its outcome, which is returned.
pub(super) async fn cutover(
  caller: &impl ModelCaller,
  session: &mut Session,
  control: &ExecutionControl,
  delivered: &mut u64,
  observe: &mut (impl FnMut(&SessionEvent) + Send),
  plan: CutoverPlan,
) -> Result<Option<RunOutcome>, SessionError> {
  let upstream = caller.supports_upstream_compaction();
  session.begin_compaction()?;
  deliver_new_events(session, delivered, observe)?;
  let result = if upstream {
    upstream::replace_with_upstream(caller, session, control, delivered, observe, plan).await
  } else {
    match standby::catch_up(caller, session, control, delivered, observe).await {
      Ok(()) => trim::trim_context(caller, session, control, plan).await,
      Err(failure) => Err(failure),
    }
  };
  session.end_compaction()?;
  let outcome = match result {
    Ok(()) => None,
    Err(failure) => Some(failure.settle(session)?),
  };
  deliver_new_events(session, delivered, observe)?;
  Ok(outcome)
}

/// Why a compaction step stopped: a session error, which fails the run, or an outcome that
/// finishes it.
pub(super) enum Failure {
  Session(SessionError),
  Outcome(RunOutcome),
}
impl From<SessionError> for Failure {
  fn from(error: SessionError) -> Self {
    Self::Session(error)
  }
}
impl From<StorageError> for Failure {
  fn from(error: StorageError) -> Self {
    Self::Session(error.into())
  }
}
impl From<Error> for Failure {
  fn from(error: Error) -> Self {
    Self::Outcome(RunOutcome::Failed(error))
  }
}
impl Failure {
  /// Finishes the run with the failure's outcome and returns it; a session error stays an error.
  fn settle(self, session: &mut Session) -> Result<RunOutcome, SessionError> {
    match self {
      Self::Outcome(outcome) => {
        session.finish_run(outcome.clone())?;
        Ok(outcome)
      }
      Self::Session(error) => Err(error),
    }
  }
}

fn check_cancelled(control: &ExecutionControl) -> Result<(), Failure> {
  if control.is_cancelled() { Err(Failure::Outcome(RunOutcome::Interrupted)) } else { Ok(()) }
}

/// The input size of `request`: the provider's count when the caller has a count endpoint,
/// otherwise the configured estimate, scaled by `calibration` when there is one.
async fn measure(
  caller: &impl ModelCaller,
  control: &ExecutionControl,
  config: &CompactionConfig,
  request: &Request,
  calibration: Option<f64>,
) -> Result<TokenMeasurement, Failure> {
  check_cancelled(control)?;
  let Some(count) = control.run_until_cancelled(caller.count_tokens(request)).await else {
    return Err(Failure::Outcome(RunOutcome::Interrupted));
  };
  if let Some(count) = count? {
    Ok(TokenMeasurement {
      tokens: count.input_tokens,
      source: TokenMeasurementSource::ProviderCount,
    })
  } else {
    let estimate = config.estimator.estimate_request(request);
    Ok(TokenMeasurement {
      tokens: calibration.map(|ratio| (estimate as f64 * ratio).ceil() as u64).unwrap_or(estimate),
      source: if calibration.is_some() {
        TokenMeasurementSource::CalibratedEstimate
      } else {
        TokenMeasurementSource::Estimate
      },
    })
  }
}

/// The prefix local compaction keeps as it is: the fixed instructions, then the encrypted
/// compaction items right after them. Another provider reads such an item through its handoff,
/// and the provider that made it still reads it after switching back, so it is neither summarized
/// nor trimmed.
fn count_kept_prefix(conversation: &[Message]) -> usize {
  let fixed = conversation.iter().take_while(|message| message.is_fixed_instruction()).count();
  fixed
    + conversation[fixed..]
      .iter()
      .take_while(|message| matches!(message, Message::UpstreamCompaction { .. }))
      .count()
}

/// The assistant content of a completed side call, a summary or a handoff (`what`): refused when
/// the model stopped for another reason than finishing, or answered with anything but assistant
/// content and reasoning, which is left out.
fn collect_assistant_content(
  response: &Response,
  what: &str,
) -> Result<Vec<ContentBlock>, RunOutcome> {
  if response.stop_reason != StopReason::Stop {
    return Err(RunOutcome::ModelStopped(Box::new(response.clone())));
  }
  let mut content = Vec::new();
  for message in &response.messages {
    match message {
      Message::Assistant { content: blocks, .. } => content.extend(blocks.iter().cloned()),
      Message::Reasoning { .. } => {}
      _ => {
        return Err(RunOutcome::Failed(Error::Malformed(format!(
          "{what} response contains non-assistant content"
        ))));
      }
    }
  }
  Ok(content)
}

/// The status the record of a compaction side call settles on.
fn call_status<T>(result: &Result<T, RunOutcome>) -> ModelCallStatus {
  match result {
    Ok(_) => ModelCallStatus::Completed,
    Err(RunOutcome::Interrupted) => ModelCallStatus::Interrupted,
    Err(_) => ModelCallStatus::Failed,
  }
}

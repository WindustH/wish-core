//! When a cutover is due: once the input the last completed conversation call sent reaches the
//! trigger, or at once for a reason forced on it.
use super::cutover;
use crate::executor::{ExecutionControl, model::ModelCaller};
use crate::protocol::Request;
use crate::session::{
  CompactionConfig, CompactionReason, Generation, RunOutcome, Session, SessionError, SessionEvent,
  statistics::{ModelCallPurpose, ModelCallRecord, ModelCallStatus},
};
use std::sync::Arc;

/// The active context, and what the last completed conversation call said about it.
pub(super) struct UsageReading {
  pub active: Arc<Generation>,
  /// The request the active context builds now.
  pub request: Request,
  /// The last completed conversation call of the active generation made with the request's model.
  pub last_call: Option<Arc<ModelCallRecord>>,
  /// That call's actual input over the estimate of the same request, to calibrate estimates by.
  pub calibration: Option<f64>,
}

pub(super) fn read_usage(session: &Session) -> Result<UsageReading, SessionError> {
  let active = session.get_active_generation()?;
  let request = session.build_request()?;
  let calls = session.reader().get_model_calls();
  let mut last_call = None;
  for position in (0..calls.len()?).rev() {
    let Some(call) = calls.get(position)? else {
      continue;
    };
    if call.generation != active.id {
      break;
    }
    if call.model == request.model
      && call.purpose == ModelCallPurpose::Conversation
      && matches!(call.status, ModelCallStatus::Completed)
    {
      last_call = Some(call);
      break;
    }
  }
  let calibration = last_call.as_ref().and_then(|call| {
    call
      .last_request_input_tokens
      .zip(call.last_request_estimated_tokens)
      .filter(|(_, estimate)| *estimate > 0)
      .map(|(actual, estimate)| actual as f64 / estimate as f64)
  });
  Ok(UsageReading { active, request, last_call, calibration })
}

/// What a cutover works from.
pub(in crate::executor) struct CutoverPlan {
  pub reason: CompactionReason,
  /// The request the active context builds now.
  pub request: Request,
  pub config: CompactionConfig,
  /// Calibrates fallback estimates; see [`UsageReading::calibration`].
  pub calibration: Option<f64>,
}

/// The cutover due now: for `forced`, or for `Usage` once the input the last completed conversation
/// call sent reached the trigger. None without compaction configured.
pub(in crate::executor) fn find_cutover(
  session: &Session,
  forced: Option<CompactionReason>,
) -> Result<Option<CutoverPlan>, SessionError> {
  let Some(config) = session.get_config().compaction.clone() else {
    return Ok(None);
  };
  let UsageReading { request, last_call, calibration, .. } = read_usage(session)?;
  let reason = forced.or_else(|| {
    last_call.as_ref().and_then(|call| {
      (call.last_request_input_tokens? >= config.trigger_tokens).then_some(CompactionReason::Usage)
    })
  });
  Ok(reason.map(|reason| CutoverPlan { reason, request, config, calibration }))
}

/// Cuts over now for `reason`, whatever the usage: a manual compaction, or a context the model
/// rejected as too long. `Interrupted` without a cutover when the run is already cancelled.
pub(in crate::executor) async fn force_cutover(
  caller: &impl ModelCaller,
  session: &mut Session,
  control: &ExecutionControl,
  delivered: &mut u64,
  observe: &mut (impl FnMut(&SessionEvent) + Send),
  reason: CompactionReason,
) -> Result<Option<RunOutcome>, SessionError> {
  if control.is_cancelled() {
    return Ok(Some(RunOutcome::Interrupted));
  }
  let Some(plan) = find_cutover(session, Some(reason))? else {
    return Ok(None);
  };
  cutover(caller, session, control, delivered, observe, plan).await
}

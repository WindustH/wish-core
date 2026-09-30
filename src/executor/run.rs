//! A session run: the loop that asks the Session state machine for its next action and carries it
//! out - a model call, a tool batch or the end of the run - with cutovers at stable points and a
//! standby summary prepared beside the work.
use super::{
  ExecutionControl,
  compaction::{self, StandbySummarizer, StandbyWait},
  model::{self, ModelCaller, ModelResult},
  observe::deliver_new_events,
  tool::{self, ToolExecutor},
};
use crate::protocol::StopReason;
use crate::session::{
  CompactionReason, RunOutcome, Session, SessionAction, SessionError, SessionEvent, SessionState,
};

/// Drive a session until idle or suspended. Resume a suspended session explicitly before calling.
/// Observers receive events from this invocation. Stream events are delivered immediately and
/// ephemeral; other events are recorded before delivery. History retains finalized messages.
/// Signal cancellation and await completion; dropping this future during I/O leaves an active
/// state that rejects another run rather than silently repeating potentially effective work.
#[allow(dead_code)] // wish-test
pub async fn run(
  model_caller: &impl ModelCaller,
  session: &mut Session,
  tool_executor: &impl ToolExecutor,
  control: &ExecutionControl,
  observe: impl FnMut(&SessionEvent) + Send,
) -> Result<RunOutcome, SessionError> {
  run_with_boundary(model_caller, session, tool_executor, control, observe, NoBoundary).await
}

/// What applying a [`RunBoundary`] did.
pub enum BoundaryResult {
  Unchanged,
  /// The configuration changed; a standby summary prepared for the old one is discarded.
  Changed,
  /// The run was interrupted meanwhile, and finishes as `Interrupted`.
  Interrupted,
}

/// Application-owned configuration changes, applied only at a run's stable action boundaries.
pub trait RunBoundary: Send {
  /// Applies pending changes before the run's next action. `delivered` - how much of the history
  /// the observer has had - and the observer let a long change publish its own events before it
  /// awaits I/O.
  fn apply(
    &mut self,
    session: &mut Session,
    control: &ExecutionControl,
    delivered: &mut u64,
    observe: &mut (impl FnMut(&SessionEvent) + Send),
  ) -> impl std::future::Future<Output = Result<BoundaryResult, SessionError>> + Send;
}
#[allow(dead_code)] // wish-test
struct NoBoundary;
impl RunBoundary for NoBoundary {
  async fn apply(
    &mut self,
    _session: &mut Session,
    _control: &ExecutionControl,
    _delivered: &mut u64,
    _observe: &mut (impl FnMut(&SessionEvent) + Send),
  ) -> Result<BoundaryResult, SessionError> {
    Ok(BoundaryResult::Unchanged)
  }
}

/// [`run`], with `boundary` applied at every stable action boundary, the first included, so the
/// model call or tool batch under way always finishes first.
pub async fn run_with_boundary(
  model_caller: &impl ModelCaller,
  session: &mut Session,
  tool_executor: &impl ToolExecutor,
  control: &ExecutionControl,
  mut observe: impl FnMut(&SessionEvent) + Send,
  boundary: impl RunBoundary,
) -> Result<RunOutcome, SessionError> {
  session.require_stable()?;
  if matches!(session.get_state(), SessionState::Suspended { .. }) {
    return Err(SessionError::Suspended);
  }
  run_scoped(session, control, async |session, control| {
    run_session(model_caller, session, tool_executor, control, &mut observe, boundary).await
  })
  .await
}

/// Runs `body` as a run of the session, on a child of `control`: the caller's cancellation reaches
/// it, and `body` is still awaited then, so it settles what it started. The child is cancelled
/// however this ends, a dropped future included, so work scoped to the run ends with it.
pub(super) async fn run_scoped<T>(
  session: &mut Session,
  control: &ExecutionControl,
  body: impl AsyncFnOnce(&mut Session, &ExecutionControl) -> T,
) -> T {
  let scoped = control.child();
  let _cancel = scoped.cancel_on_drop();
  body(session, &scoped).await
}

async fn run_session(
  model_caller: &impl ModelCaller,
  session: &mut Session,
  tool_executor: &impl ToolExecutor,
  control: &ExecutionControl,
  mut observe: impl FnMut(&SessionEvent) + Send,
  mut boundary: impl RunBoundary,
) -> Result<RunOutcome, SessionError> {
  let mut delivered = session.reader().get_history().len()?;
  let mut standby = StandbySummarizer::new(model_caller, control);
  loop {
    if session.get_state().is_stable() {
      match boundary.apply(session, control, &mut delivered, &mut observe).await? {
        BoundaryResult::Changed => standby.discard(),
        BoundaryResult::Interrupted => {
          return finish_interrupted(session, &mut delivered, &mut observe);
        }
        BoundaryResult::Unchanged => {}
      }
    }
    session.collect_inputs()?;
    deliver_new_events(session, &mut delivered, &mut observe)?;
    if matches!(session.get_state(), SessionState::Ready { .. })
      && let Some(plan) = compaction::find_cutover(session, None)?
    {
      if let StandbyWait::Cancelled = standby.settle(session, &mut delivered, &mut observe).await? {
        return finish_interrupted(session, &mut delivered, &mut observe);
      }
      let cutover =
        compaction::cutover(model_caller, session, control, &mut delivered, &mut observe, plan);
      if let Some(outcome) = cutover.await? {
        return Ok(outcome);
      }
    }
    if control.is_cancelled() && matches!(session.get_state(), SessionState::Ready { .. }) {
      standby.discard();
      session.finish_run(RunOutcome::Interrupted)?;
    }
    let action = session.advance()?;
    deliver_new_events(session, &mut delivered, &mut observe)?;
    standby.start_next(session, &mut delivered, &mut observe).await?;

    match action {
      SessionAction::Finished(outcome) => {
        // A standby summary still in flight never holds back new input: input queued meanwhile is
        // collected at once, and the summary goes on beside the next model call or tool batch.
        return match standby.wait_after_finish(session, &mut delivered, &mut observe).await? {
          StandbyWait::Input => continue,
          StandbyWait::Settled => Ok(outcome),
          StandbyWait::Cancelled => finish_interrupted(session, &mut delivered, &mut observe),
        };
      }
      SessionAction::CallModel(request) => {
        let estimator = session.get_config().compaction.as_ref().map(|config| config.estimator);
        let calling =
          model::execute_model(model_caller, &request, control, estimator, &mut observe);
        let ((result, observation), summary) = standby.beside(calling).await;
        // Input queued during the call.
        deliver_new_events(session, &mut delivered, &mut observe)?;
        if let Some(summary) = summary {
          summary.settle(session, &mut delivered, &mut observe)?;
        }
        let context_rejected = match &result {
          ModelResult::Complete(response) => {
            response.stop_reason == StopReason::ContextLengthExceeded
          }
          ModelResult::Failed(outcome) => outcome.is_context_length_exceeded(),
          ModelResult::Interrupted(_) => false,
        };
        match result {
          ModelResult::Complete(response) => session.accept_response(response, observation)?,
          ModelResult::Interrupted(partial) => session.accept_interruption(partial, observation)?,
          ModelResult::Failed(outcome) => session.fail_model_call(outcome, observation)?,
        }
        if context_rejected && session.get_config().compaction.is_some() && !control.is_cancelled()
        {
          if let StandbyWait::Cancelled =
            standby.settle(session, &mut delivered, &mut observe).await?
          {
            return finish_interrupted(session, &mut delivered, &mut observe);
          }
          recover_context_rejection(model_caller, session, control, &mut delivered, &mut observe)
            .await?;
        }
      }
      SessionAction::ExecuteTools(calls) => {
        let executing = tool::execute_tools(
          tool_executor,
          session,
          &calls,
          control,
          &mut delivered,
          &mut observe,
        );
        let (executed, summary) = standby.beside(executing).await;
        executed?;
        if let Some(summary) = summary {
          summary.settle(session, &mut delivered, &mut observe)?;
        }
        session.complete_tools()?;
      }
    }
  }
}

/// Finishes the run as interrupted and delivers what that recorded.
fn finish_interrupted(
  session: &mut Session,
  delivered: &mut u64,
  observe: &mut (impl FnMut(&SessionEvent) + Send),
) -> Result<RunOutcome, SessionError> {
  session.finish_run(RunOutcome::Interrupted)?;
  deliver_new_events(session, delivered, observe)?;
  Ok(RunOutcome::Interrupted)
}

/// Compacts after the model rejected the context as too long. The failed call already finished
/// the run; a new generation resumes it, otherwise that failure stands.
async fn recover_context_rejection(
  model_caller: &impl ModelCaller,
  session: &mut Session,
  control: &ExecutionControl,
  delivered: &mut u64,
  observe: &mut (impl FnMut(&SessionEvent) + Send),
) -> Result<(), SessionError> {
  let previous = session.get_active_generation()?.id;
  compaction::force_cutover(
    model_caller,
    session,
    control,
    delivered,
    observe,
    CompactionReason::ContextRejected,
  )
  .await?;
  if session.get_active_generation()?.id != previous {
    session.resume()?;
  }
  Ok(())
}

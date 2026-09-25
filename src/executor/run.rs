use super::{
  ExecutionControl,
  model::{self, ModelCaller, ModelResult},
  observe::notify_observers,
  tool::{self, ToolExecutor},
};
use crate::session::{
  RunOutcome, Session, SessionAction, SessionError, SessionEvent, SessionState,
};

/// Drive a session until idle or suspended. Resume a suspended session explicitly before calling.
/// Observers receive events from this invocation. Stream events are delivered immediately and
/// ephemeral; other events are recorded before delivery. History retains finalized messages.
/// Signal cancellation and await completion; dropping this future during I/O leaves an active
/// state that rejects another run rather than silently repeating potentially effective work.
pub async fn run(
  model_caller: &impl ModelCaller,
  session: &mut Session,
  tool_executor: &impl ToolExecutor,
  control: &ExecutionControl,
  observe: impl FnMut(&SessionEvent) + Send,
) -> Result<RunOutcome, SessionError> {
  run_with_boundary(model_caller, session, tool_executor, control, observe, NoBoundary).await
}

/// A selection change runs only while the session is at a stable action boundary.
/// The observer cursor lets a long handoff publish its start before awaiting the old provider.
pub enum BoundaryResult {
  Unchanged,
  Changed,
  Interrupted,
}
pub trait RunBoundary: Send {
  fn apply(
    &mut self,
    session: &mut Session,
    control: &ExecutionControl,
    cursor: &mut u64,
    observe: &mut (impl FnMut(&SessionEvent) + Send),
  ) -> impl std::future::Future<Output = Result<BoundaryResult, SessionError>> + Send;
}
struct NoBoundary;
impl RunBoundary for NoBoundary {
  async fn apply(
    &mut self,
    _session: &mut Session,
    _control: &ExecutionControl,
    _cursor: &mut u64,
    _observe: &mut (impl FnMut(&SessionEvent) + Send),
  ) -> Result<BoundaryResult, SessionError> {
    Ok(BoundaryResult::Unchanged)
  }
}

/// Apply application-owned configuration changes only between model/tool actions.
/// Return true when changed; any in-flight standby plan built for the old config is discarded.
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
  let registration = session.begin_run_control();
  let check_session = registration.create_interruption_check();
  let parent = control.clone();
  let execution = ExecutionControl::inherit(move || parent.is_cancelled() || check_session());
  let _scope = execution.create_scope();
  let interrupted = async {
    let session_interrupt = registration.wait_for_interruption();
    let executor_cancel = control.wait_for_cancellation();
    futures_util::pin_mut!(session_interrupt, executor_cancel);
    let _ = futures_util::future::select(session_interrupt, executor_cancel).await;
  };
  let running = run_session(model_caller, session, tool_executor, &execution, &mut observe, boundary);
  futures_util::pin_mut!(interrupted, running);
  match futures_util::future::select(interrupted, running).await {
    futures_util::future::Either::Left(((), running)) => {
      execution.cancel();
      running.await
    }
    futures_util::future::Either::Right((result, _)) => result,
  }
}

use std::future::Future;
use std::pin::Pin;
use futures_util::{
  future::{Either, select},
  pin_mut,
};
use crate::session::statistics::Timestamp;

/// A standby summary is speculative maintenance. Keep its failure visible in history and live
/// state, but do not fail or suspend the foreground conversation that it was preparing for.
fn record_standby_failure(
  session: &mut Session,
  outcome: RunOutcome,
  cursor: &mut u64,
  observe: &mut (impl FnMut(&SessionEvent) + Send),
) -> Result<(), SessionError> {
  session.record_events(vec![(
    Timestamp::now(),
    SessionEvent::CompactionSummaryFailed { outcome },
  )])?;
  notify_observers(session, cursor, observe)
}

async fn await_standby_task(
  task: Option<
    Pin<
      Box<
        dyn Future<
            Output = Result<
              super::compaction::StandbySummaryResult,
              super::compaction::Failure,
            >,
          > + Send
          + '_,
      >,
    >,
  >,
  session: &mut Session,
  control: &ExecutionControl,
  cursor: &mut u64,
  observe: &mut (impl FnMut(&SessionEvent) + Send),
) -> Result<Option<RunOutcome>, SessionError> {
  if let Some(task) = task {
    let cancelled = control.wait_for_cancellation();
    pin_mut!(cancelled, task);
    match select(cancelled, task).await {
      Either::Left(_) => {
        session.finish_run(RunOutcome::Interrupted)?;
        return Ok(Some(RunOutcome::Interrupted));
      }
      Either::Right((Ok(result), _)) => {
        super::compaction::commit_standby_summary(session, result, cursor, observe)?;
      }
      Either::Right((Err(super::compaction::Failure::Outcome(outcome)), _)) => {
        record_standby_failure(session, outcome, cursor, observe)?;
      }
      Either::Right((Err(super::compaction::Failure::Session(err)), _)) => {
        return Err(err);
      }
    }
  }
  Ok(None)
}

async fn run_session(
  model_caller: &impl ModelCaller,
  session: &mut Session,
  tool_executor: &impl ToolExecutor,
  control: &ExecutionControl,
  mut observe: impl FnMut(&SessionEvent) + Send,
  mut boundary: impl RunBoundary,
) -> Result<RunOutcome, SessionError> {
  let mut cursor = session.get_history().len()?;
  let mut standby_task: Option<
    Pin<
      Box<
        dyn Future<
            Output = Result<
              super::compaction::StandbySummaryResult,
              super::compaction::Failure,
            >,
          > + Send
          + '_,
      >,
    >,
  > = None;
  loop {
    if session.get_state().is_stable() {
      match boundary.apply(session, control, &mut cursor, &mut observe).await? {
        BoundaryResult::Changed => standby_task = None,
        BoundaryResult::Interrupted => {
          session.finish_run(RunOutcome::Interrupted)?;
          notify_observers(session, &mut cursor, &mut observe)?;
          return Ok(RunOutcome::Interrupted);
        }
        BoundaryResult::Unchanged => {}
      }
    }
    session.collect_inputs()?;
    notify_observers(session, &mut cursor, &mut observe)?;
    if matches!(session.get_state(), SessionState::Ready { .. }) {
      if let Some((reason, calibration, request, fixed)) =
        super::compaction::check_compaction_reason(session, None)?
      {
        if let Some(outcome) = await_standby_task(
          standby_task.take(),
          session,
          control,
          &mut cursor,
          &mut observe,
        )
        .await?
        {
          return Ok(outcome);
        }
        let outcome = super::compaction::cutover_compaction(
          model_caller,
          session,
          control,
          &mut cursor,
          &mut observe,
          reason,
          calibration,
          request,
          fixed,
        )
        .await?;
        if let Some(outcome) = outcome {
          return Ok(outcome);
        }
      }
    }
    if control.is_cancelled() && matches!(session.get_state(), SessionState::Ready { .. }) {
      standby_task = None;
      session.finish_run(RunOutcome::Interrupted)?;
    }
    let action = session.advance()?;
    notify_observers(session, &mut cursor, &mut observe)?;

    if standby_task.is_none()
      && session.get_config().compaction.is_some()
      && !model_caller.supports_upstream_compaction()
      && !control.is_cancelled()
    {
      if let Ok(Some(plan)) =
        super::compaction::plan_standby_summary(model_caller, session, control).await
      {
        let started_at = Timestamp::now();
        session.record_events(vec![(
          started_at,
          SessionEvent::CompactionSummaryStarted {
            source_start: plan.start,
            source_end: plan.end,
            measurement: plan.measurement.clone(),
          },
        )])?;
        notify_observers(session, &mut cursor, &mut observe)?;
        standby_task = Some(Box::pin(super::compaction::execute_standby_summary(
          model_caller,
          control,
          plan,
        )));
      }
    }

    match action {
      SessionAction::Finished(outcome) => {
        if let Some(finished_outcome) = await_standby_task(
          standby_task.take(),
          session,
          control,
          &mut cursor,
          &mut observe,
        )
        .await?
        {
          return Ok(finished_outcome);
        }
        return Ok(outcome);
      }
      SessionAction::CallModel(request) => {
        let ((result, observation), completed_summary) = if let Some(mut task) = standby_task.take() {
          let (model_res, completed_summary) = {
            let model_future = model::execute_model(
              model_caller,
              &request,
              session,
              control,
              &mut cursor,
              &mut observe,
            );
            pin_mut!(model_future);
            let either = select(task.as_mut(), model_future.as_mut()).await;
            match either {
              Either::Left((summary_res, _)) => {
                let model_res = model_future.await;
                (model_res, Some(summary_res))
              }
              Either::Right((model_res, _)) => {
                standby_task = Some(task);
                (model_res, None)
              }
            }
          };
          (model_res?, completed_summary)
        } else {
          (
            model::execute_model(
              model_caller,
              &request,
              session,
              control,
              &mut cursor,
              &mut observe,
            )
            .await?,
            None,
          )
        };
        if let Some(summary) = completed_summary {
          match summary {
            Ok(res) => {
              let _ = super::compaction::commit_standby_summary(
                session,
                res,
                &mut cursor,
                &mut observe,
              );
            }
            Err(super::compaction::Failure::Outcome(outcome)) => {
              record_standby_failure(session, outcome, &mut cursor, &mut observe)?;
            }
            Err(super::compaction::Failure::Session(error)) => return Err(error),
          }
        }

        let context_rejected = match &result {
          ModelResult::Complete(response) => {
            response.stop_reason == crate::protocol::StopReason::ContextLengthExceeded
          }
          ModelResult::Failed(outcome) => outcome.is_context_length_exceeded(),
          _ => false,
        };
        match result {
          ModelResult::Complete(response) => session.accept_response(response, observation)?,
          ModelResult::Interrupted(partial) => session.accept_interruption(partial, observation)?,
          ModelResult::Failed(outcome) => session.fail_model_call(outcome, observation)?,
        }
        if context_rejected && session.get_config().compaction.is_some() && !control.is_cancelled()
        {
          if let Some(outcome) = await_standby_task(
            standby_task.take(),
            session,
            control,
            &mut cursor,
            &mut observe,
          )
          .await?
          {
            return Ok(outcome);
          }
          let previous = session.get_active_generation()?.id;
          super::compaction::maintain(
            model_caller,
            session,
            control,
            &mut cursor,
            &mut observe,
            Some(crate::session::CompactionReason::ContextRejected),
          )
          .await?;
          if session.get_active_generation()?.id != previous {
            session.resume()?;
          }
        }
      }
      SessionAction::ExecuteTools(calls) => {
        let completed_summary = if let Some(mut task) = standby_task.take() {
          let (tool_res, completed_summary) = {
            let tool_future = tool::execute_tools(
              tool_executor,
              session,
              &calls,
              control,
              &mut cursor,
              &mut observe,
            );
            pin_mut!(tool_future);
            let either = select(task.as_mut(), tool_future.as_mut()).await;
            match either {
              Either::Left((summary_res, _)) => {
                let tool_res = tool_future.await;
                (tool_res, Some(summary_res))
              }
              Either::Right((tool_res, _)) => {
                standby_task = Some(task);
                (tool_res, None)
              }
            }
          };
          tool_res?;
          completed_summary
        } else {
          tool::execute_tools(
            tool_executor,
            session,
            &calls,
            control,
            &mut cursor,
            &mut observe,
          )
          .await?;
          None
        };
        if let Some(summary) = completed_summary {
          match summary {
            Ok(res) => {
              let _ = super::compaction::commit_standby_summary(
                session,
                res,
                &mut cursor,
                &mut observe,
              );
            }
            Err(super::compaction::Failure::Outcome(outcome)) => {
              record_standby_failure(session, outcome, &mut cursor, &mut observe)?;
            }
            Err(super::compaction::Failure::Session(error)) => return Err(error),
          }
        }
        session.complete_tools()?;
      }
    }
  }
}

use super::{
  ExecutionControl,
  model::{self, ModelCaller, ModelResult},
  observe::notify_observers,
  tool::{self, ToolExecutor},
};
use crate::session::{
  RunOutcome, Session, SessionError, SessionEvent, SessionState, state::SessionAction,
};

/// Drive a session until idle or suspended. Resume a suspended session explicitly before calling.
/// Observers receive events from this invocation. Stream events are delivered immediately and
/// persisted in batches; other events are recorded before delivery. Use history for older records.
/// Signal cancellation and await completion; dropping this future during I/O leaves an active
/// state that rejects another run rather than silently repeating potentially effective work.
pub async fn run(
  model_caller: &impl ModelCaller,
  session: &mut Session,
  tool_executor: &impl ToolExecutor,
  control: &ExecutionControl,
  mut observe: impl FnMut(&SessionEvent) + Send,
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
  let running = run_session(model_caller, session, tool_executor, &execution, &mut observe);
  futures_util::pin_mut!(interrupted, running);
  match futures_util::future::select(interrupted, running).await {
    futures_util::future::Either::Left(((), running)) => {
      execution.cancel();
      running.await
    }
    futures_util::future::Either::Right((result, _)) => result,
  }
}

async fn run_session(
  model_caller: &impl ModelCaller,
  session: &mut Session,
  tool_executor: &impl ToolExecutor,
  control: &ExecutionControl,
  mut observe: impl FnMut(&SessionEvent) + Send,
) -> Result<RunOutcome, SessionError> {
  let mut cursor = session.get_history().len()?;
  loop {
    session.collect_inputs()?;
    notify_observers(session, &mut cursor, &mut observe)?;
    if matches!(session.get_state(), SessionState::Ready { .. }) {
      super::compaction::maintain(model_caller, session, control, &mut cursor, &mut observe, None)
        .await?;
    }
    if control.is_cancelled() && matches!(session.get_state(), SessionState::Ready { .. }) {
      session.finish_run(RunOutcome::Interrupted)?;
    }
    let action = session.advance()?;
    notify_observers(session, &mut cursor, &mut observe)?;
    match action {
      SessionAction::Finished(outcome) => return Ok(outcome),
      SessionAction::CallModel(request) => {
        let (result, observation) =
          model::execute_model(model_caller, &request, session, control, &mut cursor, &mut observe)
            .await?;
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
        tool::execute_tools(tool_executor, session, &calls, control, &mut cursor, &mut observe)
          .await?;
        session.complete_tools()?;
      }
    }
  }
}

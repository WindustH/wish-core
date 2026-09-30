//! Running a session: starting an operation on request, scheduling a run when input arrives,
//! interrupting, and the operation itself.
//!
//! An operation holds the session from start to end and registers an `ExecutionControl`, which an
//! interrupt cancels. Starting one is serialized against shutdown (`Lifecycle::while_open`), so
//! none starts once shutdown has begun. The schedule epoch orders scheduled runs against
//! interrupts: a run scheduled before an interrupt does not start after it.
use super::{
  SessionSlot,
  selection::{SelectionBoundary, SwitchingModel},
  status::{SessionStatus, web_outcome},
};
use crate::server::{app::App, error::ApiError, provider::Provider};
use crate::{
  executor::{self, BoundaryResult, ExecutionControl},
  session::{RunOutcome, Session, SessionError},
  utils::time::Timestamp,
};
use serde_json::json;
use std::sync::{Arc, atomic::Ordering};
use tokio::sync::OwnedMutexGuard;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Operation {
  /// Consume the queued input and run turns until the model stops.
  Run,
  /// Compact the context now.
  Compact,
}

impl SessionSlot {
  /// The session, unless an operation holds it. An operation that has just ended keeps it a moment
  /// longer - its status already reads as stopped while it records its end - and a request arriving
  /// then waits that out instead of being told the session is running.
  pub async fn lock_idle(&self) -> Option<OwnedMutexGuard<Session>> {
    if let Ok(guard) = self.session.clone().try_lock_owned() {
      return Some(guard);
    }
    if self.control.lock().unwrap().is_some() {
      return None;
    }
    tokio::time::timeout(std::time::Duration::from_millis(500), self.session.clone().lock_owned())
      .await
      .ok()
  }
  /// The session, or a conflict while an operation holds it.
  pub async fn lock_idle_or_conflict(&self) -> Result<OwnedMutexGuard<Session>, ApiError> {
    self.lock_idle().await.ok_or_else(|| ApiError::conflict("session is running"))
  }
  /// Settles a session an operation left mid-step - the process stopped, or the run failed before
  /// it could settle - unless an operation still holds it. Returns whether the session is stable.
  fn settle_abandoned(&self, session: &mut Session) -> Result<bool, ApiError> {
    if session.get_state().is_stable() {
      return Ok(true);
    }
    if self.control.lock().unwrap().is_some() {
      return Ok(false);
    }
    session.settle_interrupted().map_err(ApiError::internal)?;
    self.refresh_status(session);
    Ok(true)
  }
  /// Starts an operation on request, refusing when the session is running or cannot run it.
  pub async fn start(self: &Arc<Self>, app: &App, operation: Operation) -> Result<(), ApiError> {
    let provider = self.execution_provider(app)?;
    let mut session = self.lock_idle_or_conflict().await?;
    self.require_live()?;
    if !self.settle_abandoned(&mut session)? {
      return Err(ApiError::conflict("session has unfinished execution; inspect persisted state"));
    }
    if operation == Operation::Compact && session.get_config().compaction.is_none() {
      return Err(ApiError::bad_request("session has no compaction config"));
    }
    app
      .lifecycle
      .while_open(|| -> Result<(), ApiError> {
        let control = self.begin_operation(&mut session, operation)?;
        app.lifecycle.tasks.spawn(self.clone().execute(provider, session, control, operation));
        Ok(())
      })
      .unwrap_or_else(|| Err(ApiError::shutting_down()))
  }
  /// Runs the queued input once the session is free, unless an interrupt comes first. A failure to
  /// start is reported to live clients.
  pub fn schedule(self: &Arc<Self>, app: &Arc<App>) {
    let epoch = self.get_schedule_epoch();
    let (slot, app) = (self.clone(), app.clone());
    let tasks = app.lifecycle.tasks.clone();
    tasks.spawn(async move {
      let mut session = slot.session.clone().lock_owned().await;
      let begun = app.lifecycle.while_open(|| -> Result<_, ApiError> {
        if epoch != slot.get_schedule_epoch() {
          return Ok(None);
        }
        slot.require_live()?;
        if session.get_queue_head() >= session.reader().get_message_queue().len()? {
          return Ok(None);
        }
        if !slot.settle_abandoned(&mut session)? {
          return Err(ApiError::conflict("session has unfinished execution"));
        }
        let provider = slot.execution_provider(&app)?;
        let control = slot.begin_operation(&mut session, Operation::Run)?;
        Ok(Some((provider, control)))
      });
      match begun.unwrap_or(Ok(None)) {
        Ok(Some((provider, control))) => {
          slot.execute(provider, session, control, Operation::Run).await
        }
        Ok(None) => {}
        Err(error) => {
          let _ = slot.events.send(json!({"type":"operation_failed","error":error.to_string()}));
        }
      }
    });
  }
  /// Cancels the running operation, if one runs, and any run scheduled so far. Returns whether one
  /// ran.
  pub fn interrupt(&self) -> bool {
    self.schedule_epoch.fetch_add(1, Ordering::SeqCst);
    if let Some(control) = self.control.lock().unwrap().as_ref() {
      control.cancel();
      true
    } else {
      false
    }
  }
  /// Interrupts on request: cancels the running operation, or settles a session one left mid-step.
  /// Returns whether there was either.
  pub fn interrupt_or_settle(&self) -> Result<bool, ApiError> {
    if self.interrupt() {
      return Ok(true);
    }
    if let Ok(mut session) = self.session.try_lock()
      && !session.get_state().is_stable()
    {
      let settled = self.settle_abandoned(&mut session)?;
      let _ = self.persist_index();
      return Ok(settled);
    }
    Ok(false)
  }
  /// The epoch a run scheduled now would carry.
  pub(super) fn get_schedule_epoch(&self) -> u64 {
    self.schedule_epoch.load(Ordering::SeqCst)
  }
  /// Registers an operation on the session the caller holds: resumed for a run, reported as
  /// running, and saved so.
  fn begin_operation(
    &self,
    session: &mut Session,
    operation: Operation,
  ) -> Result<ExecutionControl, ApiError> {
    if operation == Operation::Run {
      session.resume()?;
    }
    self.refresh_status(session);
    let control = ExecutionControl::new();
    *self.control.lock().unwrap() = Some(control.clone());
    self.status.lock().unwrap().begin_operation();
    if let Err(error) = self.persist_index() {
      *self.control.lock().unwrap() = None;
      self.refresh_status(session);
      return Err(error);
    }
    Ok(control)
  }
  async fn execute(
    self: Arc<Self>,
    provider: Arc<Provider>,
    mut session: OwnedMutexGuard<Session>,
    control: ExecutionControl,
    operation: Operation,
  ) {
    let model = SwitchingModel::new(self.make_model(provider));
    let result = match operation {
      Operation::Run => {
        let boundary = SelectionBoundary { slot: &self, model: &model };
        let observe = |event: &_| self.observe(event);
        executor::run_with_boundary(&model, &mut session, &self.tools, &control, observe, boundary)
          .await
      }
      Operation::Compact => self.compact(&mut session, &model, &control).await,
    };
    self.finish_operation(&mut session, result);
  }
  /// Compacts the context, with a pending selection applied first so the model that continues the
  /// session writes the summary.
  async fn compact(
    &self,
    session: &mut Session,
    model: &SwitchingModel,
    control: &ExecutionControl,
  ) -> Result<RunOutcome, SessionError> {
    let mut delivered = session.reader().get_history().len()?;
    let observe = &mut |event: &_| self.observe(event);
    match self.apply_selection(session, model, control, &mut delivered, observe).await? {
      BoundaryResult::Interrupted => {
        session.finish_run(RunOutcome::Interrupted)?;
        executor::deliver_new_events(session, &mut delivered, observe)?;
        Ok(RunOutcome::Interrupted)
      }
      _ => {
        executor::compaction::compact(model, session, control, |event| self.observe(event)).await
      }
    }
  }
  /// Ends an operation: settles a session a failure left mid-step, reports the ending in the status
  /// and to live clients, and saves the model calls and the index record.
  fn finish_operation(&self, session: &mut Session, result: Result<RunOutcome, SessionError>) {
    *self.control.lock().unwrap() = None;
    if result.is_err() && !session.get_state().is_stable() {
      let _ = session.settle_interrupted();
    }
    let ending = match result {
      Ok(outcome) => json!({"type":"operation_finished","outcome":web_outcome(&outcome)}),
      Err(error) => json!({"type":"operation_failed","error":error.to_string()}),
    };
    *self.status.lock().unwrap() = SessionStatus::after_operation(session, ending.clone());
    self.descriptor.write().unwrap().updated_at = Timestamp::now().0;
    let calls = self.reader.get_model_calls();
    if let Err(error) = self.management.save_calls(&self.get_descriptor(), &calls) {
      eprintln!("call index: {error}");
    }
    if let Err(error) = self.persist_index() {
      eprintln!("session index: {error}");
    }
    let _ = self.events.send(ending);
  }
}

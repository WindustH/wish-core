//! What a session's status reports, kept current from the engine's events, and the events a web
//! client is sent: display deltas and short outcomes, never replay data.
use super::{Descriptor, SessionSlot};
use crate::{
  protocol::StreamEvent,
  session::{
    GenerationId, RunOutcome, Session, SessionConfig, SessionEvent, SessionPhase, SessionState,
    statistics::{ModelCallPurpose, ModelCallStatus},
  },
};
use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::broadcast;

/// The engine's part of the status, as the session last reported it.
#[derive(Clone, Serialize)]
pub struct SessionStatus {
  pub phase: SessionPhase,
  /// Absent while an operation runs, when the state changes under it.
  #[serde(skip_serializing_if = "Option::is_none")]
  state: Option<SessionState>,
  active_generation: Option<GenerationId>,
  pub metadata: Value,
  pub config: SessionConfig,
  queue_head: u64,
  pub running: bool,
  standby_preparing: bool,
  context_tokens: Option<u64>,
  /// How the last operation ended, until the status is next read from the session.
  #[serde(skip_serializing_if = "Option::is_none")]
  last_operation: Option<Value>,
}
impl SessionStatus {
  /// The status of a session no operation runs on.
  pub fn read(session: &Session) -> Self {
    Self {
      phase: session.get_state().get_phase(),
      state: Some(session.get_state().clone()),
      active_generation: session.get_active_generation().ok().map(|generation| generation.id),
      metadata: session.get_metadata().clone(),
      config: session.get_config().clone(),
      queue_head: session.get_queue_head(),
      running: false,
      standby_preparing: false,
      context_tokens: context_tokens(session),
      last_operation: None,
    }
  }
  /// Marks an operation begun.
  pub fn begin_operation(&mut self) {
    self.running = true;
    self.state = None;
  }
  /// The status once an operation ended, reporting how.
  pub fn after_operation(session: &Session, ending: Value) -> Self {
    Self { last_operation: Some(ending), ..Self::read(session) }
  }
}

/// The status as the API shows it: with the queue's length, the open questions, and a pending
/// model selection shown as made.
#[derive(Serialize)]
pub struct StatusView {
  #[serde(flatten)]
  pub status: SessionStatus,
  pub queue_count: u64,
  pending_questions: Vec<Value>,
  background_tools: Vec<Value>,
  #[serde(skip_serializing_if = "std::ops::Not::not")]
  selection_pending: bool,
}

#[derive(Default)]
pub(super) struct ObservationState {
  running: bool,
  calls: bool,
  index: bool,
}

impl SessionSlot {
  /// Retry progress is transient and snapshot-backed, never added to model context/history.
  pub fn retry_observer(&self) -> crate::client::RetryObserver {
    let (live, events) = (self.live.clone(), self.events.clone());
    std::sync::Arc::new(move |status| {
      let mut live = live.lock().unwrap();
      live.observe_retry(status);
      let _ =
        events.send(json!({"type":"upstream_retry", "retry":status, "revision":live.revision}));
    })
  }
  /// Reads the status afresh from the session, which no operation holds.
  pub fn refresh_status(&self, session: &Session) {
    *self.status.lock().unwrap() = SessionStatus::read(session);
  }
  pub fn get_status(&self) -> StatusView {
    self.view_status(&self.get_descriptor())
  }
  /// The status as the API shows it for this descriptor.
  pub(super) fn view_status(&self, descriptor: &Descriptor) -> StatusView {
    let mut status = self.status.lock().unwrap().clone();
    let queued = self.sender.queued_total();
    let queue_count = queued.saturating_sub(status.queue_head);
    let pending_questions = self.tools.ask_user.snapshot();
    let selection_pending = match &descriptor.pending_selection {
      Some(pending) => {
        status.config = pending.config.clone();
        true
      }
      None => false,
    };
    StatusView {
      status,
      queue_count,
      pending_questions,
      background_tools: self.tools.background.snapshot(),
      selection_pending,
    }
  }
  /// Follows an engine event: the status, the live preview, and the event's web form to live
  /// clients. A phase change is saved to the index.
  pub fn observe(self: &std::sync::Arc<Self>, event: &SessionEvent) {
    // These events are delivered after the call record commits. Publish usage at call
    // boundaries, not only after an entire multi-turn operation finishes (nor per delta).
    let calls_changed = matches!(
      event,
      SessionEvent::TurnStarted { .. }
        | SessionEvent::ResponseAccepted { .. }
        | SessionEvent::ResponseInterrupted(_)
        | SessionEvent::ResponseRejected(_)
        | SessionEvent::CompactionSummary { .. }
        | SessionEvent::CompactionSummaryFailed { .. }
        | SessionEvent::UpstreamCompactionCompleted(_)
        | SessionEvent::CompactionTranslationCompleted { .. }
        | SessionEvent::Finished(_)
    );
    let phase_changed = {
      let mut status = self.status.lock().unwrap();
      match event {
        SessionEvent::GenerationActivated { active, .. } => {
          status.active_generation = Some(*active);
          status.context_tokens = None;
        }
        SessionEvent::InputsConsumed { queue_end, .. } => status.queue_head = *queue_end,
        SessionEvent::StateChanged { to, .. } => status.phase = *to,
        SessionEvent::CompactionSummaryStarted { .. } => status.standby_preparing = true,
        SessionEvent::CompactionSummary { .. }
        | SessionEvent::CompactionSummaryFailed { .. }
        | SessionEvent::ContextCompacted { .. } => status.standby_preparing = false,
        _ => {}
      }
      matches!(event, SessionEvent::StateChanged { .. })
    };
    if calls_changed || phase_changed {
      self.schedule_observation(calls_changed, phase_changed);
    }
    let mut live = self.live.lock().unwrap();
    live.observe(event);
    if let Some(event) = web_event(event) {
      let _ =
        self.events.send(json!({"type":"session_event","event":event,"revision":live.revision}));
    }
  }
  fn schedule_observation(self: &std::sync::Arc<Self>, calls: bool, index: bool) {
    let mut state = self.observation.lock().unwrap();
    state.calls |= calls;
    state.index |= index;
    if state.running {
      return;
    }
    state.running = true;
    let slot = self.clone();
    self.tasks.spawn(async move {
      loop {
        let (calls, index) = {
          let mut state = slot.observation.lock().unwrap();
          if !state.calls && !state.index {
            state.running = false;
            return;
          }
          (std::mem::take(&mut state.calls), std::mem::take(&mut state.index))
        };
        let current = slot.clone();
        let result = crate::application::error::blocking(move || {
          current.require_live()?;
          if calls {
            // Serialize against provider changes, which flush the old provider's calls.
            let descriptor = current.descriptor.read().unwrap();
            current.require_live()?;
            let records = current.reader.get_model_calls();
            crate::application::sampling::persist_model_calls(
              &current.management,
              &descriptor.id,
              &descriptor.provider,
              &records,
            )?;
            let (generation, model) = {
              let status = current.status.lock().unwrap();
              (status.active_generation, status.config.model.clone())
            };
            let tokens =
              generation.and_then(|generation| latest_context_tokens(&records, generation, &model));
            let mut status = current.status.lock().unwrap();
            if status.active_generation == generation && status.config.model == model {
              status.context_tokens = tokens;
            }
          }
          if index {
            current.persist_index()?;
          }
          Ok(())
        })
        .await;
        if let Err(error) = result {
          eprintln!("session observation: {error}");
        }
      }
    });
  }
  /// Notes that a selection applied at a boundary: its config is the session's now, and a standby
  /// prepared for the old model is gone.
  pub(super) fn selection_applied(&self, config: &SessionConfig) {
    let mut status = self.status.lock().unwrap();
    status.config = config.clone();
    status.standby_preparing = false;
  }
  pub fn subscribe_live(&self) -> (broadcast::Receiver<Value>, Value) {
    // Subscription and snapshot share the publication lock: no gap or duplicate deltas.
    let live = self.live.lock().unwrap();
    (self.events.subscribe(), live.snapshot(self.describe()))
  }
}

/// The web only needs display deltas and a concise outcome. Opaque replay data stays
/// in session storage, where it can be inspected without flooding every live client.
pub fn web_event(event: &SessionEvent) -> Option<Value> {
  match event {
    SessionEvent::ModelStream(
      StreamEvent::ReasoningCiphertextDelta { .. }
      | StreamEvent::ReasoningSignatureDelta { .. }
      | StreamEvent::ReasoningReplayItem { .. }
      | StreamEvent::UpstreamCompaction { .. },
    ) => None,
    SessionEvent::Finished(outcome) => Some(json!({"Finished":web_outcome(outcome)})),
    SessionEvent::ResponseInterrupted(_) => Some(json!({"ResponseInterrupted":{}})),
    SessionEvent::ResponseRejected(_) => Some(json!({"ResponseRejected":{}})),
    SessionEvent::UpstreamCompactionCompleted(_) => Some(json!({"UpstreamCompactionCompleted":{}})),
    SessionEvent::CompactionSummary { source_start, source_end, .. } => {
      Some(json!({"CompactionSummary":{"source_start":source_start,"source_end":source_end}}))
    }
    SessionEvent::CompactionSummaryFailed { outcome } => {
      Some(json!({"CompactionSummaryFailed":{"outcome":web_outcome(outcome)}}))
    }
    SessionEvent::CompactionTranslationFailed { .. } => {
      Some(json!({"CompactionTranslationFailed":{}}))
    }
    SessionEvent::ToolStarted(call) => Some(json!({"ToolStarted":{"name":call.name}})),
    SessionEvent::ToolFinished { .. } => Some(json!({"ToolFinished":{}})),
    SessionEvent::MetadataUpdated(_) => Some(json!({"MetadataUpdated":{}})),
    _ => Some(json!(event)),
  }
}
/// An outcome without the partial or full response it may carry.
pub(super) fn web_outcome(outcome: &RunOutcome) -> Value {
  match outcome {
    RunOutcome::StreamFailed(partial) => json!({"StreamFailed":{"reason":partial.reason}}),
    RunOutcome::ModelStopped(response) => {
      json!({"ModelStopped":{"stop_reason":response.stop_reason}})
    }
    _ => json!(outcome),
  }
}
/// The input size compaction compares with its trigger: the last completed
/// conversation call of the active generation made with the configured model.
fn context_tokens(session: &Session) -> Option<u64> {
  let active = session.get_active_generation().ok()?.id;
  let model = &session.get_config().model;
  latest_context_tokens(&session.reader().get_model_calls(), active, model)
}
fn latest_context_tokens(
  calls: &crate::storage::ReadList<crate::session::statistics::ModelCallRecord>,
  active: GenerationId,
  model: &str,
) -> Option<u64> {
  for position in (0..calls.len().ok()?).rev() {
    let Some(call) = calls.get(position).ok()? else { continue };
    if call.generation != active {
      break;
    }
    if call.model == model
      && call.purpose == ModelCallPurpose::Conversation
      && matches!(call.status, ModelCallStatus::Completed)
    {
      return call.last_request_input_tokens;
    }
  }
  None
}

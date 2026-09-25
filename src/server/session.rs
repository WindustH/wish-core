mod live;
pub mod selection;
mod tools;
use crate::server::{error::ApiError, provider::Provider};
use crate::{
  executor::{self, ExecutionControl},
  protocol::StreamEvent,
  session::{
    Entry, EntryId, Generation, HistoryReader, RunOutcome, Session, SessionConfig, SessionEvent,
    SessionHandle,
    statistics::{ModelCallPurpose, ModelCallRecord, ModelCallStatus},
  },
  storage::ReadList,
  tool::{
    shell::{ShellConfig, ShellTool},
    view_image::ViewImageTool,
  },
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
  path::PathBuf,
  sync::{Arc, Mutex, RwLock},
};
use tokio::sync::{Mutex as AsyncMutex, broadcast};
use tools::SessionTools;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Descriptor {
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub pending_selection: Option<selection::PendingSelection>,
  pub name: String,
  pub updated_at: u64,
  pub revision: u64,
  pub id: String,
  pub provider: String,
  pub cwd: PathBuf,
  pub shell: bool,
  pub created_at: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateSession {
  #[serde(default)]
  pub initial_messages: Vec<crate::protocol::Message>,
  #[serde(default)]
  pub name: String,
  pub provider: String,
  pub config: SessionConfig,
  pub cwd: PathBuf,
  #[serde(default)]
  pub shell: bool,
  #[serde(default)]
  pub metadata: Value,
}

pub struct SessionSlot {
  pub descriptor: RwLock<Descriptor>,
  pub index: Arc<crate::server::management::ManagementStore>,
  pub global_events: broadcast::Sender<Value>,
  pub auto_run_generation: std::sync::atomic::AtomicU64,
  pub deleted: std::sync::atomic::AtomicBool,
  pub session: Arc<AsyncMutex<Session>>,
  pub selection_edit: Arc<AsyncMutex<()>>,
  pub handle: SessionHandle,
  pub history: HistoryReader,
  pub entries: ReadList<Entry>,
  pub generations: ReadList<Generation>,
  pub calls: ReadList<ModelCallRecord>,
  pub queue: Mutex<ReadList<EntryId>>,
  pub tools: SessionTools,
  pub image_dir: PathBuf,
  tasks: tokio_util::task::TaskTracker,
  app: std::sync::Weak<crate::server::app::App>,
  pub events: broadcast::Sender<Value>,
  live: Mutex<live::LivePreview>,
  pub status: Mutex<Value>,
  pub control: Mutex<Option<ExecutionControl>>,
}
impl SessionSlot {
  pub async fn open(
    descriptor: Descriptor,
    mut session: Session,
    data_dir: PathBuf,
    index: Arc<crate::server::management::ManagementStore>,
    global_events: broadcast::Sender<Value>,
    app: std::sync::Weak<crate::server::app::App>,
  ) -> Result<Arc<Self>, ApiError> {
    if !session.get_state().is_stable() {
      session.settle_interrupted().map_err(ApiError::internal)?;
    }
    let shell = if descriptor.shell {
      let mut config =
        ShellConfig::new(&descriptor.cwd, data_dir.join("shell").join(&descriptor.id));
      if let Some(app) = app.upgrade() {
        config.command = Arc::clone(&app.shell);
      }
      Some(ShellTool::new(config).await.map_err(ApiError::internal)?)
    } else {
      None
    };
    let image_dir = std::path::absolute(data_dir.join("blobs").join(&descriptor.id))
      .map_err(ApiError::internal)?;
    let tasks = app.upgrade().expect("session application is alive").tasks.clone();
    let tools =
      SessionTools::new(shell, &session, app.clone(), descriptor.id.clone(), image_dir.clone());
    let status = Mutex::new(snapshot(&session));
    let (events, _) = broadcast::channel(256);
    Ok(Arc::new(Self {
      descriptor: RwLock::new(descriptor),
      index,
      global_events,
      auto_run_generation: std::sync::atomic::AtomicU64::new(0),
      deleted: std::sync::atomic::AtomicBool::new(false),
      handle: session.create_handle(),
      history: session.create_history_reader(),
      entries: session.get_entries(),
      generations: session.get_generations(),
      calls: session.get_model_calls(),
      queue: Mutex::new(session.get_message_queue()),
      session: Arc::new(AsyncMutex::new(session)),
      selection_edit: Arc::new(AsyncMutex::new(())),
      tools,
      image_dir,
      tasks,
      app,
      events,
      live: Mutex::new(live::LivePreview::default()),
      status,
      control: Mutex::new(None),
    }))
  }
  pub fn configure_tools(&self, mut config: SessionConfig) -> Result<SessionConfig, ApiError> {
    for tool in &config.tools {
      let is_valid = match tool.name.as_str() {
        "view_image" | "history_search" | "history_read" | "history_query" => true,
        "shell_start" | "shell_edit" | "shell_poll" | "shell_write" | "shell_kill" => {
          self.tools.shell.is_some()
        }
        _ => false,
      };
      if !is_valid {
        return Err(ApiError::bad_request(format!("no executor for tool {}", tool.name)));
      }
    }
    config.tools.clear();
    config.tools.extend(self.tools.get_history_specifications());
    config.tools.push(ViewImageTool.get_specification());
    if let Some(shell) = &self.tools.shell {
      config.tools.extend(shell.get_specifications());
    }
    Ok(config)
  }
  pub fn describe(&self) -> Value {
    {
      let mut status = self.status.lock().unwrap().clone();
      status["queue_count"] = json!(
        self
          .queue
          .lock()
          .unwrap()
          .len()
          .unwrap_or(0)
          .saturating_sub(status["queue_head"].as_u64().unwrap_or(0))
      );
      let mut descriptor = self.get_descriptor();
      if let Some(pending) = &descriptor.pending_selection {
        descriptor.provider = pending.provider.clone();
        status["config"] = json!(pending.config);
        status["selection_pending"] = json!(true);
      }
      json!({"session":descriptor,"status":status})
    }
  }
  pub fn get_descriptor(&self) -> Descriptor {
    self.descriptor.read().unwrap().clone()
  }
  pub fn require_live(&self) -> Result<(), ApiError> {
    if self.deleted.load(std::sync::atomic::Ordering::Acquire) {
      Err(ApiError::not_found())
    } else {
      Ok(())
    }
  }
  pub fn persist_index(&self) -> Result<(), ApiError> {
    self.require_live()?;
    let mut record = self.describe();
    // `describe` previews a pending provider to the UI. Persist the actual provider so a restart
    // can still ask it to translate an encrypted compaction item before applying the selection.
    record["session"] = json!(self.get_descriptor());
    self.index.save(&record)?;
    let _ =
      self.global_events.send(json!({"type":"session_changed","id":self.get_descriptor().id}));
    Ok(())
  }
  pub fn update_snapshot(&self, session: &Session) {
    *self.queue.lock().unwrap() = session.get_message_queue();
    *self.status.lock().unwrap() = snapshot(session);
  }
  pub fn interrupt(&self) -> bool {
    self.auto_run_generation.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    if let Some(control) = self.control.lock().unwrap().as_ref() {
      control.cancel();
      true
    } else {
      false
    }
  }
  pub fn observe(&self, event: &SessionEvent) {
    if let SessionEvent::InputsConsumed { queue_end, .. } = event {
      self.status.lock().unwrap()["queue_head"] = json!(queue_end);
    }
    if let SessionEvent::StateChanged { to, .. } = event {
      self.status.lock().unwrap()["phase"] = json!(to);
      if let Err(error) = self.persist_index() {
        eprintln!("session index: {error}");
      }
    }
    if let SessionEvent::CompactionSummaryStarted { .. } = event {
      self.status.lock().unwrap()["standby_preparing"] = json!(true);
    }
    if matches!(
      event,
      SessionEvent::CompactionSummary { .. } | SessionEvent::CompactionSummaryFailed { .. }
    ) {
      self.status.lock().unwrap()["standby_preparing"] = json!(false);
    }
    if let SessionEvent::ContextCompacted { .. } = event {
      self.status.lock().unwrap()["standby_preparing"] = json!(false);
    }
    let mut live = self.live.lock().unwrap();
    live.observe(event);
    if let Some(event) = web_event(event) {
      let _ = self.events.send(json!({"type":"session_event","event":event,"revision":live.revision}));
    }
  }
  pub fn subscribe_live(&self) -> (broadcast::Receiver<Value>, Value) {
    // Subscription and snapshot share the publication lock: no gap or duplicate deltas.
    let live = self.live.lock().unwrap();
    (self.events.subscribe(), live.snapshot(self.describe()))
  }
  pub async fn execute(
    self: Arc<Self>,
    provider: Arc<Provider>,
    provider_id: String,
    mut session: tokio::sync::OwnedMutexGuard<Session>,
    control: ExecutionControl,
    compact: bool,
  ) {
    let model = selection::SwitchingModel::new(self.make_model(provider, provider_id));
    let result = if compact {
      let prepared = match session.get_history().len() {
        Ok(mut cursor) => self.apply_selection(
          &mut session, &model, &control, &mut cursor, &mut |event| self.observe(event),
        ).await,
        Err(error) => Err(error.into()),
      };
      match prepared {
        Ok(executor::BoundaryResult::Interrupted) => {
          session.finish_run(RunOutcome::Interrupted).map(|_| RunOutcome::Interrupted)
        }
        Ok(_) => executor::compaction::compact(&model, &mut session, &control, |event| self.observe(event)).await,
        Err(error) => Err(error),
      }
    } else {
      executor::run_with_boundary(
        &model,
        &mut session,
        &self.tools,
        &control,
        |event| self.observe(event),
        selection::SelectionBoundary { slot: &self, model: &model },
      )
      .await
    };
    *self.control.lock().unwrap() = None;
    if result.is_err() && !session.get_state().is_stable() {
      let _ = session.settle_interrupted();
    }
    let event = match result {
      Ok(outcome) => json!({"type":"operation_finished","outcome":web_outcome(&outcome)}),
      Err(error) => json!({"type":"operation_failed","error":error.to_string()}),
    };
    {
      let mut status = self.status.lock().unwrap();
      *status = snapshot(&session);
      status["last_operation"] = event.clone();
    }
    {
      self.descriptor.write().unwrap().updated_at = crate::session::statistics::Timestamp::now().0;
    }
    if let Err(error) = self.index.save_calls(&self.get_descriptor(), &self.calls) {
      eprintln!("call index: {error}");
    }
    if let Err(error) = self.persist_index() {
      eprintln!("session index: {error}");
    }
    let _ = self.events.send(event);
  }
}
/// The web only needs display deltas and a concise outcome. Opaque replay data stays
/// in session storage, where it can be inspected without flooding every live client.
pub(crate) fn web_event(event: &SessionEvent) -> Option<Value> {
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
    SessionEvent::CompactionSummary { source_start, source_end, .. } =>
      Some(json!({"CompactionSummary":{"source_start":source_start,"source_end":source_end}})),
    SessionEvent::CompactionSummaryFailed { outcome } =>
      Some(json!({"CompactionSummaryFailed":{"outcome":web_outcome(outcome)}})),
    SessionEvent::CompactionTranslationFailed { .. } =>
      Some(json!({"CompactionTranslationFailed":{}})),
    SessionEvent::ToolStarted(call) => Some(json!({"ToolStarted":{"name":call.name}})),
    SessionEvent::ToolFinished { .. } => Some(json!({"ToolFinished":{}})),
    SessionEvent::MetadataUpdated(_) => Some(json!({"MetadataUpdated":{}})),
    _ => Some(json!(event)),
  }
}
fn web_outcome(outcome: &RunOutcome) -> Value {
  match outcome {
    RunOutcome::StreamFailed(partial) => json!({"StreamFailed":{"reason":partial.reason}}),
    RunOutcome::ModelStopped(response) =>
      json!({"ModelStopped":{"stop_reason":response.stop_reason}}),
    _ => json!(outcome),
  }
}
fn snapshot(session: &Session) -> Value {
  json!({"phase":session.get_state().get_phase(),"state":session.get_state(),
    "active_generation":session.get_active_generation().ok().map(|g|g.id),"metadata":session.get_metadata(),"config":session.get_config(),"queue_head":session.get_queue_head(),"running":false,"standby_preparing":false,
    "context_tokens":context_tokens(session)})
}
/// The input size compaction compares with its trigger: the last completed
/// conversation call of the active generation made with the configured model.
fn context_tokens(session: &Session) -> Option<u64> {
  let active = session.get_active_generation().ok()?.id;
  let model = &session.get_config().model;
  let calls = session.get_model_calls();
  for position in (0..calls.len().ok()?).rev() {
    let Some(call) = calls.get(position).ok()? else { continue };
    if call.generation != active {
      break;
    }
    if &call.model == model
      && call.purpose == ModelCallPurpose::Conversation
      && matches!(call.status, ModelCallStatus::Completed)
    {
      return call.last_request_input_tokens;
    }
  }
  None
}

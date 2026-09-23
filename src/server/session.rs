mod tools;
mod live;
pub mod selection;
use crate::server::{error::ApiError, provider::Provider};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
  path::PathBuf,
  sync::{Arc, Mutex, RwLock},
};
use tokio::sync::{Mutex as AsyncMutex, broadcast};
use tools::SessionTools;
use crate::{
  executor::{self, ExecutionControl},
  session::{
    Entry, EntryId, Generation, HistoryReader, Session, SessionConfig, SessionEvent, SessionHandle,
    statistics::ModelCallRecord,
  },
  storage::ReadList,
  tool::{
    shell::{ShellConfig, ShellTool},
    view_image::ViewImageTool,
  },
};

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
    mut descriptor: Descriptor,
    mut session: Session,
    data_dir: PathBuf,
    index: Arc<crate::server::management::ManagementStore>,
    global_events: broadcast::Sender<Value>,
    app: std::sync::Weak<crate::server::app::App>,
  ) -> Result<Arc<Self>, ApiError> {
    if !session.get_state().is_stable() {
      session.settle_interrupted().map_err(ApiError::internal)?;
    }
    if let Some(pending) = descriptor.pending_selection.take() {
      session.set_config(pending.config).map_err(ApiError::internal)?;
      descriptor.provider = pending.provider;
    }
    let shell = if descriptor.shell {
      Some(
        ShellTool::new(ShellConfig::new(
          &descriptor.cwd,
          data_dir.join("shell").join(&descriptor.id),
        ))
        .await
        .map_err(ApiError::internal)?,
      )
    } else {
      None
    };
    let image_dir = std::path::absolute(data_dir.join("blobs").join(&descriptor.id))
      .map_err(ApiError::internal)?;
    let tasks = app.upgrade().expect("session application is alive").tasks.clone();
    let tools = SessionTools::new(shell, &session, app.clone(), descriptor.id.clone(), image_dir.clone());
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
        "shell_start" | "shell_poll" | "shell_write" | "shell_kill" => self.tools.shell.is_some(),
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
    self.index.save(&self.describe())?;
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
    if let SessionEvent::CompactionSummary { .. } = event {
      self.status.lock().unwrap()["standby_preparing"] = json!(false);
    }
    if let SessionEvent::ContextCompacted { .. } = event {
      self.status.lock().unwrap()["standby_preparing"] = json!(false);
    }
    let mut live = self.live.lock().unwrap();
    live.observe(event);
    let _ = self.events.send(json!({"type":"session_event","event":event,"revision":live.revision}));
  }
  pub fn subscribe_live(&self) -> (broadcast::Receiver<Value>, Value) {
    // Subscription and snapshot share the publication lock: no gap or duplicate deltas.
    let live = self.live.lock().unwrap();
    (self.events.subscribe(), live.snapshot(self.describe()))
  }
  pub async fn execute(
    self: Arc<Self>,
    provider: Arc<Provider>,
    mut session: tokio::sync::OwnedMutexGuard<Session>,
    control: ExecutionControl,
    compact: bool,
  ) {
    let model = selection::SwitchingModel::new(self.make_model(provider, self.get_descriptor().provider));
    let result = if compact {
      executor::compaction::compact(&model, &mut session, &control, |event| self.observe(event))
        .await
    } else {
      executor::run_with_boundary(&model, &mut session, &self.tools, &control, |event| self.observe(event), |session| self.apply_selection(session, &model)).await
    };
    *self.control.lock().unwrap() = None;
    if result.is_err() && !session.get_state().is_stable() {
      let _ = session.settle_interrupted();
    }
    let event = match result {
      Ok(outcome) => json!({"type":"operation_finished","outcome":outcome}),
      Err(error) => json!({"type":"operation_failed","error":error.to_string()}),
    };
    {
      let mut status = self.status.lock().unwrap();
      *status = snapshot(&session);
      status["last_operation"] = event.clone();
    }
    {
      self.descriptor.write().unwrap().updated_at =
        crate::session::statistics::Timestamp::now().0;
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
fn snapshot(session: &Session) -> Value {
  json!({"phase":session.get_state().get_phase(),"state":session.get_state(),
    "active_generation":session.get_active_generation().ok().map(|g|g.id),"metadata":session.get_metadata(),"config":session.get_config(),"queue_head":session.get_queue_head(),"running":false,"standby_preparing":false})
}

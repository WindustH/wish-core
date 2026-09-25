use crate::server::{
  config::Config,
  error::{ApiError, blocking},
  provider::Provider,
  session::{CreateSession, Descriptor, SessionSlot},
};
use crate::{
  session::Session,
  storage::{Storage, StorageOptions},
};
use std::{
  collections::BTreeMap,
  path::PathBuf,
  sync::{Arc, Mutex, RwLock},
};
use tokio::sync::Mutex as AsyncMutex;
use tokio_util::{sync::CancellationToken, task::TaskTracker};

pub struct App {
  pub storage: Storage,
  pub index: Arc<crate::server::management::ManagementStore>,
  pub configuration: AsyncMutex<crate::server::configuration::Configuration>,
  pub codex_login: crate::server::codex_login::LoginManager,
  pub events: tokio::sync::broadcast::Sender<serde_json::Value>,
  pub started: std::time::Instant,
  pub providers: RwLock<BTreeMap<String, Arc<Provider>>>,
  /// Shared with every session's shell tool; a configuration save replaces its value.
  pub shell: Arc<RwLock<crate::tool::shell::ShellCommand>>,
  pub sessions: AsyncMutex<BTreeMap<String, Arc<SessionSlot>>>,
  pub data_dir: PathBuf,
  pub token: Option<String>,
  pub stop: CancellationToken,
  pub tasks: TaskTracker,
  /// Serializes operation registration against shutdown, never held across awaits.
  pub closing: Mutex<bool>,
}
impl App {
  pub async fn open(config: &Config, config_path: PathBuf) -> Result<Arc<Self>, ApiError> {
    config.proxy.validate()?;
    let shell = Arc::new(RwLock::new(config.shell.resolve()?));
    tokio::fs::create_dir_all(&config.data_dir).await.map_err(ApiError::internal)?;
    let path = config.data_dir.join("wish.sqlite");
    let index_path = config.data_dir.join("management.sqlite");
    let (storage, index) = blocking(move || {
      Ok((
        Storage::open(path, StorageOptions::default())?,
        Arc::new(crate::server::management::ManagementStore::open(&index_path)?),
      ))
    })
    .await?;
    let tasks = TaskTracker::new();
    let mut providers = BTreeMap::new();
    for (id, provider_config) in &config.providers {
      let mut provider = Provider::build(provider_config.clone(), &config.proxy)?;
      provider.client = crate::server::sampling::observe_client(
        &provider.client,
        index.clone(),
        tasks.clone(),
        id.clone(),
        None,
      );
      providers.insert(id.clone(), Arc::new(provider));
    }
    let configuration =
      crate::server::configuration::Configuration::new(config_path, config.clone())?;
    let (events, _) = tokio::sync::broadcast::channel(256);
    let token = config
      .bearer_token_env
      .as_ref()
      .map(|key| crate::server::config::read_secret(key).map_err(ApiError::bad_request))
      .transpose()?;
    let app = Arc::new(Self {
      storage,
      index,
      configuration: AsyncMutex::new(configuration),
      codex_login: crate::server::codex_login::LoginManager::default(),
      events,
      started: std::time::Instant::now(),
      providers: RwLock::new(providers),
      shell,
      sessions: AsyncMutex::new(BTreeMap::new()),
      data_dir: config.data_dir.clone(),
      token,
      stop: CancellationToken::new(),
      tasks,
      closing: Mutex::new(false),
    });
    crate::server::codex_login::start_refresh_worker(&app);
    Ok(app)
  }
  pub fn get_provider(&self, id: &str) -> Result<Arc<Provider>, ApiError> {
    self
      .providers
      .read()
      .unwrap()
      .get(id)
      .filter(|p| p.config.enabled)
      .cloned()
      .ok_or_else(ApiError::not_found)
  }
  pub fn require_open(&self) -> Result<(), ApiError> {
    if *self.closing.lock().unwrap() {
      Err(ApiError::conflict("server is shutting down"))
    } else {
      Ok(())
    }
  }
  pub async fn create_session(
    self: &Arc<Self>,
    mut input: CreateSession,
  ) -> Result<Arc<SessionSlot>, ApiError> {
    self.require_open()?;
    self.get_provider(&input.provider)?;
    if !input.cwd.is_absolute() {
      return Err(ApiError::bad_request("cwd must be absolute"));
    }
    if !tokio::fs::metadata(&input.cwd)
      .await
      .map_err(|e| ApiError::bad_request(e.to_string()))?
      .is_dir()
    {
      return Err(ApiError::bad_request("cwd must be a directory"));
    }
    if !input.config.tools.is_empty() {
      return Err(ApiError::bad_request(
        "tools are installed by the server; pass an empty tools array",
      ));
    }
    let descriptor = Descriptor {
      pending_selection: None,
      id: uuid::Uuid::new_v4().to_string(),
      provider: input.provider,
      cwd: input.cwd,
      shell: input.shell,
      created_at: crate::session::statistics::Timestamp::now().0,
      updated_at: crate::session::statistics::Timestamp::now().0,
      name: input.name,
      revision: 0,
    };
    for message in &mut input.initial_messages {
      message.normalize_new_input();
    }
    crate::server::media::apply_agent_instructions(&mut input.initial_messages);
    let storage = self.storage.clone();
    let id = descriptor.id.clone();
    let session = blocking(move || {
      let mut session = Session::create(storage, &id, input.config)?;
      session.set_metadata(input.metadata)?;
      session.import_history(input.initial_messages)?;
      Ok(session)
    })
    .await?;
    let slot = SessionSlot::open(
      descriptor.clone(),
      session,
      self.data_dir.clone(),
      self.index.clone(),
      self.events.clone(),
      Arc::downgrade(self),
    )
    .await?;
    {
      let mut session = slot.session.lock().await;
      let config = slot.configure_tools(session.get_config().clone())?;
      session.set_config(config)?;
      slot.update_snapshot(&session);
    }
    slot.persist_index()?;
    self.sessions.lock().await.insert(slot.get_descriptor().id.clone(), slot.clone());
    Ok(slot)
  }
  pub async fn get_session(self: &Arc<Self>, id: &str) -> Result<Arc<SessionSlot>, ApiError> {
    let mut sessions = self.sessions.lock().await;
    if let Some(slot) = sessions.get(id) {
      slot.require_live()?;
      return Ok(slot.clone());
    }
    let storage = self.storage.clone();
    let index = self.index.clone();
    let id = id.to_owned();
    let (descriptor, session) = blocking(move || {
      let value = index.read(&id)?;
      let descriptor =
        serde_json::from_value(value["session"].clone()).map_err(ApiError::internal)?;
      let session = Session::load(storage, &id)?;
      Ok((descriptor, session))
    })
    .await?;
    let slot = SessionSlot::open(
      descriptor,
      session,
      self.data_dir.clone(),
      self.index.clone(),
      self.events.clone(),
      Arc::downgrade(self),
    )
    .await?;
    {
      let mut session = slot.session.lock().await;
      if session.get_state().is_stable() {
        let config = slot.configure_tools(session.get_config().clone())?;
        session.set_config(config)?;
        slot.update_snapshot(&session);
      }
    }
    slot.persist_index()?;
    sessions.insert(slot.get_descriptor().id.clone(), slot.clone());
    Ok(slot)
  }
  pub async fn begin_shutdown(&self) {
    *self.closing.lock().unwrap() = true;
    self.stop.cancel();
    for slot in self.sessions.lock().await.values() {
      slot.interrupt();
    }
  }
  pub async fn finish_shutdown(&self) -> Result<(), ApiError> {
    self.tasks.close();
    self.tasks.wait().await;
    for slot in self.sessions.lock().await.values() {
      if let Some(shell) = &slot.tools.shell {
        shell.shutdown().await.map_err(ApiError::internal)?;
      }
    }
    let storage = self.storage.clone();
    blocking(move || {
      storage.shutdown()?;
      Ok(())
    })
    .await
  }
}

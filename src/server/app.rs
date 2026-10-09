//! The application: what every handler reaches through its state - storage and the management
//! index, the configuration and what is built from it (providers, the shell, MCP servers, search
//! providers), the open sessions - and its lifecycle from startup to shutdown.
use crate::server::{
  bridge::Bridge,
  codex_login::LoginManager,
  config::Config,
  config_file::ConfigFile,
  copilot_login::CopilotLoginManager,
  data_dir::DataDir,
  error::{ApiError, blocking},
  management::ManagementStore,
  mcp::McpHub,
  provider::{self, Provider, Providers, read_secret},
  search::{PreparedSearch, SearchHub},
  session::{CreateSession, Descriptor, SessionSlot, ToolSwitches},
  session_model::apply_agent_instructions,
};
use crate::{
  session::{EntryOrigin, Session},
  storage::{Storage, StorageOptions},
  tool::shell::ShellCommand,
  utils::time::Timestamp,
};
use serde_json::{Value, json};
use std::{
  collections::BTreeMap,
  path::PathBuf,
  sync::{Arc, Mutex, RwLock},
};
use tokio::sync::Mutex as AsyncMutex;
use tokio_util::{sync::CancellationToken, task::TaskTracker};

pub struct App {
  pub storage: Storage,
  pub management: Arc<ManagementStore>,
  pub config_file: AsyncMutex<ConfigFile>,
  pub codex_login: LoginManager,
  pub copilot_login: CopilotLoginManager,
  /// Changes to the session list and the configuration, for `GET /events`.
  pub events: tokio::sync::broadcast::Sender<Value>,
  pub started: std::time::Instant,
  providers: RwLock<Providers>,
  /// The shell sessions without their own run commands under. A configuration save replaces it
  /// and hands the new one to those sessions (`SessionSlot::follow_global_shell`).
  pub shell: RwLock<ShellCommand>,
  /// The sessions opened since startup, by id. A session is opened on first use and stays open
  /// until it is deleted.
  pub sessions: AsyncMutex<BTreeMap<String, Arc<SessionSlot>>>,
  pub data_dir: DataDir,
  pub bearer_token: Option<String>,
  pub lifecycle: Lifecycle,
  /// How sessions' shells reach this server: `wish mcp` and `wish skill` go through it.
  pub bridge: Bridge,
  /// Wish's own skills: `skills` beside the configuration file.
  pub skills_dir: PathBuf,
  /// MCP servers and their live connections.
  pub mcp: McpHub,
  /// The search providers `web_search` asks.
  pub search: SearchHub,
}

/// Startup to shutdown. Shutdown begins by refusing new work and cancelling long waits, then waits
/// for every background task before storage closes.
pub struct Lifecycle {
  /// Cancelled when shutdown begins.
  pub stop: CancellationToken,
  /// Every background task: operations, schedulers, samplers, login and refresh workers.
  pub tasks: TaskTracker,
  /// Set when shutdown begins. Starting an operation holds it, so none starts after; never held
  /// across awaits.
  closing: Mutex<bool>,
}
impl Lifecycle {
  fn new() -> Self {
    Self { stop: CancellationToken::new(), tasks: TaskTracker::new(), closing: Mutex::new(false) }
  }
  pub fn require_open(&self) -> Result<(), ApiError> {
    if *self.closing.lock().unwrap() { Err(ApiError::shutting_down()) } else { Ok(()) }
  }
  /// Runs `register` unless shutdown has begun, which waits for it to finish.
  pub fn while_open<T>(&self, register: impl FnOnce() -> T) -> Option<T> {
    let closing = self.closing.lock().unwrap();
    (!*closing).then(register)
  }
  /// Awaits `future`, unless shutdown begins first.
  pub async fn until_shutdown<F: Future>(&self, future: F) -> Result<F::Output, ApiError> {
    tokio::select! {
      _ = self.stop.cancelled() => Err(ApiError::shutting_down()),
      output = future => Ok(output),
    }
  }
  fn begin_shutdown(&self) {
    *self.closing.lock().unwrap() = true;
    self.stop.cancel();
  }
}

/// A configuration, checked and built, waiting for its file to be written.
struct PreparedConfig {
  providers: Providers,
  search: PreparedSearch,
  shell: ShellCommand,
}

impl App {
  pub async fn open(config: &Config, config_path: PathBuf) -> Result<Arc<Self>, ApiError> {
    config.validate()?;
    let search = SearchHub::new(&config.search, &config.proxy)?;
    let shell = config.shell.resolve()?;
    let data_dir = DataDir::new(config.data_dir.clone());
    tokio::fs::create_dir_all(data_dir.root()).await.map_err(ApiError::internal)?;
    let (database, management_database) = (data_dir.database(), data_dir.management_database());
    let sample_limit = config.usage.stream_sample_limit;
    let (storage, management) = blocking(move || {
      let management = ManagementStore::open(&management_database)?;
      management.set_stream_sample_limit(sample_limit);
      // A limit lowered while the server was stopped applies now.
      if let Err(error) = management.merge_stream_samples() {
        eprintln!("merging stream samples failed: {error:?}");
      }
      Ok((Storage::open(database, StorageOptions::default())?, Arc::new(management)))
    })
    .await?;
    let lifecycle = Lifecycle::new();
    let providers =
      provider::build_all(&config.providers, &config.proxy, &management, &lifecycle.tasks)?;
    let skills_dir =
      std::path::absolute(&config_path).map_err(ApiError::internal)?.with_file_name("skills");
    // Made at once, so there is a place to put skills in.
    let _ = std::fs::create_dir_all(&skills_dir);
    let config_file = ConfigFile::open(config_path, config.clone())?;
    let (events, _) = tokio::sync::broadcast::channel(256);
    let bearer_token = config
      .bearer_token_env
      .as_ref()
      .map(|key| read_secret(key).map_err(ApiError::bad_request))
      .transpose()?;
    let mcp = McpHub::new(config.mcp.clone(), config.proxy.clone(), config.defaults.cwd.clone());
    let bridge = Bridge::new(&data_dir);
    let app = Arc::new(Self {
      storage,
      management,
      config_file: AsyncMutex::new(config_file),
      codex_login: LoginManager::default(),
      copilot_login: CopilotLoginManager::default(),
      events,
      started: std::time::Instant::now(),
      providers: RwLock::new(providers),
      shell: RwLock::new(shell),
      sessions: AsyncMutex::new(BTreeMap::new()),
      data_dir,
      bearer_token,
      lifecycle,
      bridge,
      skills_dir,
      mcp,
      search,
    });
    crate::server::codex_login::start_refresh_worker(&app);
    let reaper = app.clone();
    app.lifecycle.tasks.spawn(async move {
      loop {
        tokio::select! {
          _ = reaper.lifecycle.stop.cancelled() => break,
          _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => reaper.mcp.reap(),
        }
      }
    });
    Ok(app)
  }
  /// An enabled provider.
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
  /// Every provider, enabled or not, as they stand now.
  pub fn get_providers(&self) -> Providers {
    self.providers.read().unwrap().clone()
  }
  pub async fn create_session(
    self: &Arc<Self>,
    mut input: CreateSession,
  ) -> Result<Arc<SessionSlot>, ApiError> {
    self.lifecycle.require_open()?;
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
    // Without a word from the request: no shell and no web search, and questions to the user and
    // MCP allowed. Web search stays off while no search provider could answer it.
    let mut tools = ToolSwitches {
      shell: false,
      ask_user: true,
      mcp: true,
      web_search: false,
      skills: true,
      sessions: true,
    }
    .apply_changes(input.tools);
    tools.web_search &= self.search.is_available(&self.providers.read().unwrap());
    let folder = input.folder.take();
    if let Some(folder) = &folder
      && self.management.folder(folder)?.is_none()
    {
      return Err(ApiError::bad_request(format!("no folder `{folder}`")));
    }
    let descriptor = Descriptor {
      pending_selection: None,
      created_by: input.created_by,
      id: uuid::Uuid::new_v4().to_string(),
      provider: input.provider,
      cwd: input.cwd,
      tools,
      shell_override: None,
      created_at: Timestamp::now().0,
      updated_at: Timestamp::now().0,
      name: input.name,
      revision: 0,
    };
    for message in &mut input.initial_messages {
      message.normalize_new_input();
    }
    // Agent instructions may be inserted at the front; origins line up from the back.
    apply_agent_instructions(&mut input.initial_messages, &input.config.tools);
    let inserted = input.initial_messages.len().saturating_sub(input.initial_origins.len());
    let origins = std::iter::repeat_n(EntryOrigin::Imported, inserted).chain(input.initial_origins);
    let initial: Vec<_> = input.initial_messages.into_iter().zip(origins).collect();
    let storage = self.storage.clone();
    let id = descriptor.id.clone();
    let session = blocking(move || {
      let mut session = Session::create(storage, &id, input.config)?;
      session.set_metadata(input.metadata)?;
      session.import_history(initial)?;
      Ok(session)
    })
    .await?;
    let slot = self.open_slot(descriptor, session).await?;
    if let Some(folder) = folder {
      self.management.place(&[slot.get_descriptor().id], Some(&folder))?;
    }
    self.sessions.lock().await.insert(slot.get_descriptor().id.clone(), slot.clone());
    Ok(slot)
  }
  /// An open session, opened from storage when it is not open yet.
  pub async fn get_session(self: &Arc<Self>, id: &str) -> Result<Arc<SessionSlot>, ApiError> {
    let mut sessions = self.sessions.lock().await;
    if let Some(slot) = sessions.get(id) {
      slot.require_live()?;
      return Ok(slot.clone());
    }
    let storage = self.storage.clone();
    let management = self.management.clone();
    let id = id.to_owned();
    let (descriptor, session) = blocking(move || {
      let value = management.read(&id)?;
      let descriptor =
        serde_json::from_value(value["session"].clone()).map_err(ApiError::internal)?;
      let session = Session::load(storage, &id)?;
      Ok((descriptor, session))
    })
    .await?;
    let slot = self.open_slot(descriptor, session).await?;
    sessions.insert(slot.get_descriptor().id.clone(), slot.clone());
    Ok(slot)
  }
  /// Opens a slot for a session and installs the tools its switches give it, then saves its index
  /// record.
  async fn open_slot(
    self: &Arc<Self>,
    descriptor: Descriptor,
    session: Session,
  ) -> Result<Arc<SessionSlot>, ApiError> {
    let slot = SessionSlot::open(self, descriptor, session).await?;
    {
      let mut session = slot.session.lock().await;
      if session.get_state().is_stable() {
        let current = session.get_config().clone();
        let config = slot.configure_tools(current.clone())?;
        // Reopening usually installs the tools the session already has; only a change is
        // recorded, since each record holds the whole configuration.
        if serde_json::to_value(&config).ok() != serde_json::to_value(&current).ok() {
          session.set_config(config)?;
        }
        slot.refresh_status(&session);
      }
    }
    slot.persist_index()?;
    Ok(slot)
  }

  /// Saves the configuration a settings page sends, named at the revision it read, and puts it in
  /// place. Nothing is written or changed unless all of it checks out.
  pub async fn save_config(&self, revision: String, value: Value) -> Result<Value, ApiError> {
    let mut file = self.config_file.lock().await;
    let next = file.read_save(&revision, value).await?;
    let prepared = self.prepare_config(&next, &file.config)?;
    file.write(next).await?;
    self.apply_config(prepared, &file.config).await;
    let _ = self.events.send(json!({"type":"configuration_changed"}));
    Ok(file.redacted())
  }
  /// Checks a configuration a save sends against the current one, and builds what it describes.
  fn prepare_config(&self, next: &Config, current: &Config) -> Result<PreparedConfig, ApiError> {
    next.validate()?;
    let search = SearchHub::prepare(&next.search, &next.proxy)?;
    let shell = next.shell.resolve()?;
    if next.listen != current.listen
      || next.data_dir != current.data_dir
      || next.bearer_token_env != current.bearer_token_env
      || next.web_dir != current.web_dir
      || next.allowed_hosts != current.allowed_hosts
    {
      return Err(ApiError::bad_request(
        "listen, data_dir, bearer_token_env, web_dir and allowed_hosts are startup settings; edit \
         the file and restart",
      ));
    }
    let providers =
      provider::build_all(&next.providers, &next.proxy, &self.management, &self.lifecycle.tasks)?;
    if !next.defaults.provider.is_empty() && !providers.contains_key(&next.defaults.provider) {
      return Err(ApiError::bad_request("default provider does not exist"));
    }
    if !next.defaults.cwd.is_absolute() {
      return Err(ApiError::bad_request("default cwd must be absolute"));
    }
    // Validate core budgets without creating a persistent session.
    let _ = Session::new(next.defaults.session_config())?;
    Ok(PreparedConfig { providers, search, shell })
  }
  async fn apply_config(&self, prepared: PreparedConfig, config: &Config) {
    let PreparedConfig { providers, search, shell } = prepared;
    self.mcp.apply(
      config.mcp.clone(),
      config.proxy.clone(),
      config.defaults.cwd.clone(),
      &providers,
    );
    self.search.apply(search);
    self.management.set_stream_sample_limit(config.usage.stream_sample_limit);
    let management = self.management.clone();
    self.lifecycle.tasks.spawn_blocking(move || {
      if let Err(error) = management.merge_stream_samples() {
        eprintln!("merging stream samples failed: {error:?}");
      }
    });
    *self.providers.write().unwrap() = providers;
    *self.shell.write().unwrap() = shell.clone();
    for slot in self.sessions.lock().await.values() {
      slot.follow_global_shell(&shell);
    }
  }

  pub async fn begin_shutdown(&self) {
    self.lifecycle.begin_shutdown();
    for slot in self.sessions.lock().await.values() {
      slot.interrupt();
    }
  }
  pub async fn finish_shutdown(&self) -> Result<(), ApiError> {
    self.lifecycle.tasks.close();
    self.lifecycle.tasks.wait().await;
    self.mcp.close_all().await;
    for slot in self.sessions.lock().await.values() {
      if let Some(shell) = slot.tools.shell() {
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

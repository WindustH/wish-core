//! The application: what every handler reaches through its state - storage and the management
//! index, the configuration and what is built from it (providers, the shell, MCP servers, search
//! providers), the open sessions - and its lifecycle from startup to shutdown.
use super::accounts::{AccountRegistry, AccountRuntime};
use crate::application::{
  bridge::Bridge,
  codex_login::LoginManager,
  config::Config,
  config_file::ConfigFile,
  copilot_login::CopilotLoginManager,
  error::{ApplicationError, blocking},
  identity::read_secret,
  mcp::McpHub,
  provider::{self, Provider, Providers},
  search::{PreparedSearch, SearchHub},
  session::{Descriptor, SessionSlot},
};
use crate::storage::data_dir::DataDir;
use crate::storage::management::ManagementStore;
use crate::{
  session::Session,
  storage::{Storage, StorageOptions},
  task::ShellCommand,
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
  pub(super) storage: Storage,
  pub(super) management: Arc<ManagementStore>,
  pub(super) config_file: AsyncMutex<ConfigFile>,
  pub codex_login: LoginManager,
  pub(super) copilot_login: CopilotLoginManager,
  /// Changes to the session list and the configuration, for `GET /events`.
  pub(super) events: tokio::sync::broadcast::Sender<Value>,
  pub(super) started: std::time::Instant,
  providers: RwLock<Providers>,
  /// The shell sessions without their own run commands under. A configuration save replaces it
  /// and hands the new one to those sessions (`SessionSlot::follow_global_shell`).
  pub(super) shell: RwLock<ShellCommand>,
  /// Serializes creation with first-input retries, including requests whose response was lost.
  pub(super) creation_lock: AsyncMutex<()>,
  session_open_locks: Mutex<BTreeMap<String, std::sync::Weak<AsyncMutex<()>>>>,
  /// The sessions opened since startup, by id. A session is opened on first use and stays open
  /// until it is deleted.
  pub(super) sessions: AsyncMutex<BTreeMap<String, Arc<SessionSlot>>>,
  pub(super) data_dir: DataDir,
  pub bearer_token: Option<String>,
  pub lifecycle: Lifecycle,
  /// How sessions' shells reach this server: `wish mcp` and `wish skill` go through it.
  pub bridge: Bridge,
  /// Wish's own skills: `skills` beside the configuration file.
  pub(super) skills_dir: PathBuf,
  /// MCP servers and their live connections.
  pub(super) mcp: McpHub,
  /// The search providers `web_search` asks.
  pub(super) search: SearchHub,
  pub(super) fetch: super::fetch::FetchHub,
  pub(super) accounts: RwLock<AccountRegistry>,
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
  pub fn require_open(&self) -> Result<(), ApplicationError> {
    if *self.closing.lock().unwrap() { Err(ApplicationError::shutting_down()) } else { Ok(()) }
  }
  /// Runs `register` unless shutdown has begun, which waits for it to finish.
  pub fn while_open<T>(&self, register: impl FnOnce() -> T) -> Option<T> {
    let closing = self.closing.lock().unwrap();
    (!*closing).then(register)
  }
  /// Awaits `future`, unless shutdown begins first.
  pub async fn until_shutdown<F: Future>(&self, future: F) -> Result<F::Output, ApplicationError> {
    tokio::select! {
      _ = self.stop.cancelled() => Err(ApplicationError::shutting_down()),
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
  accounts: AccountRegistry,
  providers: Providers,
  search: PreparedSearch,
  fetch: super::fetch::PreparedFetch,
  shell: ShellCommand,
}

impl App {
  pub async fn open(config: &Config, config_path: PathBuf) -> Result<Arc<Self>, ApplicationError> {
    let mut canonical = config.clone();
    canonical.migrate_accounts()?;
    let migrated = serde_json::to_value(config).map_err(ApplicationError::internal)?
      != serde_json::to_value(&canonical).map_err(ApplicationError::internal)?;
    let config = &canonical;
    config.validate()?;
    let search = SearchHub::new(&config.resolved_search()?, &config.proxy)?;
    let shell = config.shell.resolve()?;
    let data_dir = DataDir::new(config.data_dir.clone());
    tokio::fs::create_dir_all(data_dir.root()).await.map_err(ApplicationError::internal)?;
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
    let providers = provider::build_all(
      &config.resolved_providers()?,
      &config.proxy,
      &management,
      &lifecycle.tasks,
    )?;
    let skills_dir = std::path::absolute(&config_path)
      .map_err(ApplicationError::internal)?
      .with_file_name("skills");
    // Made at once, so there is a place to put skills in.
    tokio::fs::create_dir_all(&skills_dir).await.map_err(ApplicationError::internal)?;
    let accounts = config.build_accounts()?;
    let mut config_file = ConfigFile::open(config_path, config.clone())?;
    if migrated {
      config_file.write(config.clone()).await?;
    }
    let (events, _) = tokio::sync::broadcast::channel(256);
    let bearer_token = config
      .bearer_token_env
      .as_ref()
      .map(|key| read_secret(key).map_err(ApplicationError::invalid))
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
      accounts: RwLock::new(accounts),
      shell: RwLock::new(shell),
      creation_lock: AsyncMutex::new(()),
      session_open_locks: Mutex::new(BTreeMap::new()),
      sessions: AsyncMutex::new(BTreeMap::new()),
      data_dir,
      bearer_token,
      lifecycle,
      bridge,
      skills_dir,
      mcp,
      search,
      fetch: super::fetch::FetchHub::new(config.fetch.clone(), config.proxy.clone())?,
    });
    crate::application::credentials::start_refresh_worker(&app);
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
  /// Resolve an account ID or a legacy model-service ID for sign-in and renewal.
  pub fn get_login_account(&self, id: &str) -> Result<Arc<AccountRuntime>, ApplicationError> {
    let account_id = if id.starts_with("account:") {
      id.to_owned()
    } else {
      let provider = self.get_provider(id)?;
      format!(
        "account:{}",
        provider
          .config
          .credential_id
          .as_ref()
          .ok_or_else(|| ApplicationError::invalid("provider has no account for sign-in"))?
      )
    };
    self.accounts.read().unwrap().get(&account_id).cloned().ok_or_else(ApplicationError::not_found)
  }
  pub fn get_service_accounts(&self) -> AccountRegistry {
    self.accounts.read().unwrap().clone()
  }
  pub fn get_provider(&self, id: &str) -> Result<Arc<Provider>, ApplicationError> {
    self
      .providers
      .read()
      .unwrap()
      .get(id)
      .filter(|p| p.config.enabled)
      .cloned()
      .ok_or_else(ApplicationError::not_found)
  }
  /// Every provider, enabled or not, as they stand now.
  pub fn get_providers(&self) -> Providers {
    self.providers.read().unwrap().clone()
  }
  /// An open session, opened from storage when it is not open yet.
  pub async fn get_session(
    self: &Arc<Self>,
    id: &str,
  ) -> Result<Arc<SessionSlot>, ApplicationError> {
    if let Some(slot) = self.sessions.lock().await.get(id) {
      slot.require_live()?;
      return Ok(slot.clone());
    }
    let opening = {
      let mut locks = self.session_open_locks.lock().unwrap();
      locks.retain(|_, lock| lock.strong_count() > 0);
      let entry = locks.entry(id.to_owned()).or_default();
      match entry.upgrade() {
        Some(lock) => lock,
        None => {
          let lock = Arc::new(AsyncMutex::new(()));
          *entry = Arc::downgrade(&lock);
          lock
        }
      }
    };
    let _opening = opening.lock().await;
    if let Some(slot) = self.sessions.lock().await.get(id) {
      slot.require_live()?;
      return Ok(slot.clone());
    }
    self.lifecycle.require_open()?;
    let storage = self.storage.clone();
    let management = self.management.clone();
    let id = id.to_owned();
    let (descriptor, session) = blocking(move || {
      let value = management.read(&id)?;
      let descriptor =
        serde_json::from_value(value["session"].clone()).map_err(ApplicationError::internal)?;
      let session = Session::load(storage, &id)?;
      Ok((descriptor, session))
    })
    .await?;
    let slot = self.open_slot(descriptor, session).await?;
    self.sessions.lock().await.insert(slot.get_descriptor().id.clone(), slot.clone());
    Ok(slot)
  }
  /// Opens a slot for a session and installs the tools its switches give it, then saves its index
  /// record.
  pub(super) async fn open_slot(
    self: &Arc<Self>,
    descriptor: Descriptor,
    session: Session,
  ) -> Result<Arc<SessionSlot>, ApplicationError> {
    let slot = SessionSlot::open(self, descriptor, session).await?;
    {
      let mut session = slot.session.lock().await;
      if session.get_state().is_stable() {
        let current = session.get_config().clone();
        let config = slot.configure_tools(current.clone())?;
        crate::application::instructions::persist_agent_instructions(
          &mut session,
          config,
          slot.get_descriptor().tools,
        )?;
        slot.refresh_status(&session);
      }
    }
    slot.persist_index()?;
    Ok(slot)
  }

  /// Saves the configuration a settings page sends, named at the revision it read, and puts it in
  /// place. Nothing is written or changed unless all of it checks out.
  pub async fn save_config(
    &self,
    revision: String,
    value: Value,
  ) -> Result<Value, ApplicationError> {
    let mut file = self.config_file.lock().await;
    let next = file.read_save(&revision, value).await?;
    let prepared = self.prepare_config(&next, &file.config)?;
    file.write(next).await?;
    self.apply_config(prepared, &file.config).await;
    let _ = self.events.send(json!({"type":"configuration_changed"}));
    file.redacted()
  }
  /// Checks a configuration a save sends against the current one, and builds what it describes.
  fn prepare_config(
    &self,
    next: &Config,
    current: &Config,
  ) -> Result<PreparedConfig, ApplicationError> {
    next.validate()?;
    let search = SearchHub::prepare(&next.resolved_search()?, &next.proxy)?;
    let fetch = super::fetch::FetchHub::prepare(&next.fetch, &next.proxy)?;
    let shell = next.shell.resolve()?;
    if next.listen != current.listen
      || next.data_dir != current.data_dir
      || next.bearer_token_env != current.bearer_token_env
      || next.web_dir != current.web_dir
      || next.allowed_hosts != current.allowed_hosts
    {
      return Err(ApplicationError::invalid(
        "listen, data_dir, bearer_token_env, web_dir and allowed_hosts are startup settings; edit \
         the file and restart",
      ));
    }
    let providers = provider::build_all(
      &next.resolved_providers()?,
      &next.proxy,
      &self.management,
      &self.lifecycle.tasks,
    )?;
    if !next.defaults.provider.is_empty() && !providers.contains_key(&next.defaults.provider) {
      return Err(ApplicationError::invalid("default provider does not exist"));
    }
    if !next.defaults.cwd.is_absolute() {
      return Err(ApplicationError::invalid("default cwd must be absolute"));
    }
    // Validate core budgets without creating a persistent session.
    let _ = Session::new(next.defaults.session_config())?;
    let accounts = next.build_accounts()?;
    Ok(PreparedConfig { accounts, providers, search, fetch, shell })
  }
  async fn apply_config(&self, prepared: PreparedConfig, config: &Config) {
    let PreparedConfig { accounts, providers, search, fetch, shell } = prepared;
    self.mcp.apply(
      config.mcp.clone(),
      config.proxy.clone(),
      config.defaults.cwd.clone(),
      &providers,
    );
    self.search.apply(search);
    self.fetch.apply(fetch);
    self.management.set_stream_sample_limit(config.usage.stream_sample_limit);
    let management = self.management.clone();
    self.lifecycle.tasks.spawn(async move {
      let result = crate::utils::blocking::io(move || management.merge_stream_samples()).await;
      if !matches!(result, Ok(Ok(_))) {
        eprintln!("merging stream samples failed: {result:?}");
      }
    });
    *self.accounts.write().unwrap() = accounts;
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
  pub async fn finish_shutdown(&self) -> Result<(), ApplicationError> {
    self.lifecycle.tasks.close();
    self.lifecycle.tasks.wait().await;
    self.mcp.close_all().await;
    for slot in self.sessions.lock().await.values() {
      if let Some(shell) = slot.tools.tasks() {
        shell.shutdown().await.map_err(ApplicationError::internal)?;
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

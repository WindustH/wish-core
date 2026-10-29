//! The built-in tools of a session: which optional ones it has (its switches), the executor that
//! runs them, and the shell its commands run under.
mod extensions;
use super::{SessionSlot, ToolChanges, ToolSwitches};
use crate::application::{
  config::ShellSettings,
  error::{ApplicationError, blocking},
};
use crate::capability::Capability;
use crate::storage::blobs;
use crate::{
  executor::{ExecutionControl, tool::ToolExecutor},
  protocol::ContentBlock,
  session::{Session, SessionConfig, ToolCall, ToolOutcome},
  task::{ShellCommand, TaskConfig},
  tool::{
    ask_user::AskUserTool,
    history::HistoryTools,
    shell::ShellTool,
    view_image::ViewImageTool,
    web_search::{SearchBackend, WebSearchTool},
  },
};
use serde_json::json;
use std::sync::{
  Arc, Weak,
  atomic::{AtomicBool, Ordering},
};

pub struct SessionTools {
  pub background: crate::application::background_tools::BackgroundTools,
  /// Started the first time the shell is switched on, and kept while it is off so background
  /// commands finish and report as usual.
  tasks: tokio::sync::OnceCell<crate::task::TaskManager>,
  shell_config: TaskConfig,
  shell_on: AtomicBool,
  ask_user_on: AtomicBool,
  pub ask_user: AskUserTool,
  web_search_on: AtomicBool,
  web_fetch_on: AtomicBool,
  image_generate_on: AtomicBool,
  video_generate_on: AtomicBool,
  audio_generate_on: AtomicBool,
  audio_transcribe_on: AtomicBool,
  image_generate: crate::tool::image_generate::ImageGenerateTool,
  pub web_fetch: crate::application::pages::PageReader,
  pub web_search: WebSearchTool,
  pub history: HistoryTools,
  history_on: AtomicBool,
  slot: Weak<SessionSlot>,
}
impl SessionTools {
  pub(crate) fn specifications(&self, capability: Capability) -> Vec<crate::protocol::Tool> {
    match capability {
      Capability::History => self.history.get_specifications(),
      Capability::Image => vec![ViewImageTool.get_specification()],
      Capability::Sessions => vec![crate::application::session_access::specification()],
      Capability::Task => vec![crate::application::task_access::specification()],
      Capability::Shell => self.shell().map(|shell| shell.get_specifications()).unwrap_or_default(),
      Capability::AskUser => vec![self.ask_user.get_specification()],
      Capability::ExtensionDiscovery => vec![extensions::specification()],
      Capability::WebFetch
      | Capability::WebSearch
      | Capability::ImageGenerate
      | Capability::VideoGenerate
      | Capability::AudioGenerate
      | Capability::AudioTranscribe => Vec::new(),
    }
  }

  pub fn new(
    slot: Weak<SessionSlot>,
    shell_config: TaskConfig,
    session: &Session,
    session_id: &str,
  ) -> Self {
    let web_fetch =
      make_fetch(slot.clone(), session_id, shell_config.capture_dir.join("web-fetch"));
    Self {
      background: Default::default(),
      tasks: tokio::sync::OnceCell::new(),
      shell_config,
      shell_on: AtomicBool::new(false),
      ask_user_on: AtomicBool::new(false),
      ask_user: AskUserTool::new(questions_changed(slot.clone())),
      web_search_on: AtomicBool::new(false),
      web_fetch_on: AtomicBool::new(false),
      image_generate_on: AtomicBool::new(false),
      video_generate_on: AtomicBool::new(false),
      audio_generate_on: AtomicBool::new(false),
      audio_transcribe_on: AtomicBool::new(false),
      image_generate: make_image_generate(slot.clone()),
      web_fetch,
      web_search: WebSearchTool::new(
        search_backend(slot.clone()),
        session_id.to_owned(),
        provider_listing(slot.clone(), Capability::WebSearch),
      ),
      history: HistoryTools::new(session),
      history_on: AtomicBool::new(true),
      slot,
    }
  }
  pub fn tasks(&self) -> Option<&crate::task::TaskManager> {
    self.tasks.get()
  }
  /// The shell tool, once it has been switched on.
  pub fn shell(&self) -> Option<ShellTool> {
    self.tasks.get().cloned().map(ShellTool::from_tasks)
  }
  pub async fn task_manager(&self) -> Result<&crate::task::TaskManager, ApplicationError> {
    self
      .tasks
      .get_or_try_init(|| crate::task::TaskManager::new(self.shell_config.clone()))
      .await
      .map_err(|error| ApplicationError::invalid(format!("could not open tasks: {error}")))
  }
  /// Applies the session's switches, starting the shell tool the first time it is on.
  pub async fn switch(&self, switches: ToolSwitches) -> Result<(), ApplicationError> {
    if switches.shell {
      self
        .tasks
        .get_or_try_init(|| crate::task::TaskManager::new(self.shell_config.clone()))
        .await
        .map_err(|error| {
          ApplicationError::invalid(format!("could not start the shell: {error}"))
        })?;
    }
    self.history_on.store(switches.history_search, Ordering::Release);
    self.shell_on.store(switches.shell, Ordering::Release);
    self.ask_user_on.store(switches.ask_user, Ordering::Release);
    self.web_search_on.store(switches.web_search, Ordering::Release);
    self.web_fetch_on.store(switches.web_fetch, Ordering::Release);
    self.image_generate_on.store(switches.image_generate, Ordering::Release);
    self.video_generate_on.store(switches.video_generate, Ordering::Release);
    self.audio_generate_on.store(switches.audio_generate, Ordering::Release);
    self.audio_transcribe_on.store(switches.audio_transcribe, Ordering::Release);
    Ok(())
  }
  async fn execute_shell(&self, call: &ToolCall, control: &ExecutionControl) -> ToolOutcome {
    let Some(shell) = self.shell().filter(|_| self.shell_on.load(Ordering::Acquire)) else {
      return ToolOutcome::Failed("shell is not enabled for this session".into());
    };
    let epoch = self.slot.upgrade().map(|slot| slot.get_schedule_epoch());
    let outcome = shell.execute(call, control).await;
    if let ToolOutcome::Success(output)
    | ToolOutcome::SuccessWithMetadata { output, .. }
    | ToolOutcome::Report { output, failed: false, .. } = &outcome
    {
      self.notify_background(output, call.arguments["command"].clone(), epoch).await;
    }
    outcome
  }
  pub async fn notify_background(
    &self,
    output: &serde_json::Value,
    command: serde_json::Value,
    epoch: Option<u64>,
  ) {
    if output["process"]["status"] != "running" {
      return;
    }
    let Some(shell) = self.tasks.get() else { return };
    let Some(id) = output["execution_id"].as_str() else { return };
    if !shell.claim_notification(id) {
      return;
    }
    let Some(app) = self.slot.upgrade().and_then(|slot| slot.app.upgrade()) else { return };
    let Some(session) = self.slot.upgrade() else { return };
    let sender = session.sender.clone();
    let marker = json!({"type":"background_execution_started","id":id,"command":command});
    if let Err(error) = blocking(move || Ok(sender.record_application_event(marker)?)).await {
      eprintln!("background shell transfer journal: {error}");
      // Still supervise and deliver the live result; do not silently abandon the running task.
    }
    // A command left running in the background reports its end as a message, which starts a run
    // unless the session was interrupted since this one began.
    let (shell, id, slot, command) = (shell.clone(), id.to_owned(), self.slot.clone(), command);
    let owner = app.clone();
    app.lifecycle.tasks.spawn(async move {
      let result = tokio::select! {
        _ = owner.lifecycle.stop.cancelled() => return,
        result = shell.wait_for_completion(&id) => result,
      };
      let result = match result {
        Ok(result) => result,
        Err(error) => json!({"execution_id":id,"error":error.to_string()}),
      };
      let Some(slot) = slot.upgrade().filter(|slot| slot.require_live().is_ok()) else { return };
      let edits = shell.show(&id).ok().map(|task| task["edits"].clone());
      if let Err(error) = crate::application::background_tools::finish_shell(
        &slot.sender,
        &id,
        command,
        result,
        edits,
      )
      .await
      {
        eprintln!("background notification: {error}");
        return;
      }
      let _ = slot.events.send(json!({"type":"history_changed"}));
      if slot.require_live().is_ok() {
        let _ = owner.events.send(json!({"type":"session_changed","id":slot.get_descriptor().id}));
        if epoch == Some(slot.get_schedule_epoch()) && !owner.lifecycle.stop.is_cancelled() {
          slot.schedule(&owner);
        }
      }
    });
  }
  async fn view_image(&self, call: &ToolCall, control: &ExecutionControl) -> ToolOutcome {
    let outcome = ViewImageTool.execute(call, control).await;
    let ToolOutcome::SuccessWithInput { mut output, mut input } = outcome else {
      return outcome;
    };
    let Some(slot) = self.slot.upgrade() else {
      return ToolOutcome::Failed("the session is closed".into());
    };
    for block in &input {
      if let ContentBlock::Image { data_base64, .. } = block {
        let (directory, data) = (slot.blob_dir.clone(), data_base64.clone());
        let saved = blocking(move || {
          blobs::save_image(&directory, &data).map_err(ApplicationError::internal)
        })
        .await;
        match saved {
          Ok(path) => output["session_path"] = json!(path),
          Err(error) => return ToolOutcome::Failed(error.to_string()),
        }
      }
    }
    input.retain(|block| matches!(block, ContentBlock::Image { .. }));
    ToolOutcome::SuccessWithInput { output, input }
  }
}
/// Searches with the application's search providers as they stand at the moment of the search.
fn search_backend(slot: Weak<SessionSlot>) -> SearchBackend {
  Arc::new(move |query, provider| {
    let owner = slot.upgrade();
    let app = owner.as_ref().and_then(|slot| slot.app.upgrade());
    let observer = owner.map(|slot| slot.retry_observer());
    Box::pin(async move {
      let app = app.ok_or_else(|| vec!["the server is shutting down".to_owned()])?;
      let found = app
        .search
        .search(&app.get_service_accounts(), &query, observer, provider.as_deref())
        .await?;
      Ok(found)
    })
  })
}
/// Publishes the session's open questions whenever they change: its index record and a change
/// notice.
fn questions_changed(slot: Weak<SessionSlot>) -> impl Fn() + Send + Sync + 'static {
  move || {
    let Some(slot) = slot.upgrade() else { return };
    let tasks = slot.tasks.clone();
    tasks.spawn(async move {
      if let Err(error) = blocking(move || slot.persist_index()).await {
        eprintln!("persisting question status failed: {error}");
      }
    });
  }
}
impl ToolExecutor for SessionTools {
  fn accepts_unadvertised_call(&self, name: &str) -> bool {
    // Recognize the route here; execute checks permissions and gives a specific disabled error.
    Capability::from_tool(name).is_some_and(Capability::command_backed)
  }

  async fn execute_with_input(
    &self,
    call: &ToolCall,
    control: &ExecutionControl,
    input: Option<crate::executor::tool::ToolInput>,
  ) -> ToolOutcome {
    let Some(input) = input else {
      return self.execute(call, control).await;
    };
    if matches!(call.name.as_str(), "shell" | "shell_edit") {
      let Some(shell) = self.shell().filter(|_| self.shell_on.load(Ordering::Acquire)) else {
        return ToolOutcome::Failed("shell is not enabled for this session".into());
      };
      let epoch = self.slot.upgrade().map(|slot| slot.get_schedule_epoch());
      let outcome = shell.execute_with_input(call, control, Some(input)).await;
      if let ToolOutcome::Report { output, .. } = &outcome {
        self.notify_background(output, call.arguments["command"].clone(), epoch).await;
      }
      return outcome;
    }
    let Some(slot) = self.slot.upgrade() else {
      return ToolOutcome::Failed("session is closed".into());
    };
    self.background.execute(slot, call, control, input).await
  }

  async fn execute(&self, call: &ToolCall, control: &ExecutionControl) -> ToolOutcome {
    if crate::extensions::catalog::is_tool(&call.name) {
      return self.execute_extension(call, control).await;
    }
    match Capability::from_tool(&call.name) {
      Some(Capability::Image) => self.view_image(call, control).await,
      Some(Capability::Sessions) => {
        let Some(slot) = self.slot.upgrade() else {
          return ToolOutcome::Failed("session is closed".into());
        };
        let Some(app) = slot.app.upgrade() else {
          return ToolOutcome::Failed("server is closed".into());
        };
        let request = match crate::application::session_access::parse_request(&call.arguments) {
          Ok(request) => request,
          Err(error) => return ToolOutcome::Failed(error.to_string()),
        };
        match control
          .run_until_cancelled(crate::application::session_access::execute(app, slot, request))
          .await
        {
          Some(Ok(output)) => ToolOutcome::Success(output),
          Some(Err(error)) => ToolOutcome::Failed(error.to_string()),
          None => ToolOutcome::Cancelled,
        }
      }
      Some(Capability::Task) => {
        let Some(slot) = self.slot.upgrade() else {
          return ToolOutcome::Failed("session is closed".into());
        };
        let Some(app) = slot.app.upgrade() else {
          return ToolOutcome::Failed("server is closed".into());
        };
        let request = match crate::application::task_access::parse_request(&call.arguments) {
          Ok(request) => request,
          Err(error) => return ToolOutcome::Failed(error.to_string()),
        };
        match control
          .run_until_cancelled(crate::application::task_access::execute(&app, slot, request, false))
          .await
        {
          Some(Ok(output)) => ToolOutcome::Report {
            text: crate::tool::task_output::render(&output, false),
            output,
            metadata: serde_json::Value::Null,
            failed: false,
          },
          Some(Err(error)) => ToolOutcome::Failed(error.to_string()),
          None => ToolOutcome::Cancelled,
        }
      }
      Some(Capability::AskUser) => {
        if !self.ask_user_on.load(Ordering::Acquire) {
          return ToolOutcome::Failed("ask_user is not enabled for this session".into());
        }
        self.ask_user.execute(call, control).await
      }
      Some(Capability::VideoGenerate) => ToolOutcome::Failed("use an extension media tool".into()),
      Some(Capability::AudioTranscribe) => {
        ToolOutcome::Failed("use an extension transcription tool".into())
      }
      Some(Capability::AudioGenerate) => ToolOutcome::Failed("use an extension media tool".into()),
      Some(Capability::ImageGenerate) => {
        if !self.image_generate_on.load(Ordering::Acquire) {
          return ToolOutcome::Failed("image_generate is disabled for this session".into());
        }
        self.image_generate.execute(call, control).await
      }
      Some(Capability::WebFetch) => ToolOutcome::Failed("use an extension reader tool".into()),
      Some(Capability::WebSearch) => {
        if !self.web_search_on.load(Ordering::Acquire) {
          return ToolOutcome::Failed("web_search is not enabled for this session".into());
        }
        self.web_search.execute(call, control).await
      }
      Some(Capability::History) => {
        if !self.history_on.load(Ordering::Acquire) {
          return ToolOutcome::Failed("history search is disabled for this session".into());
        }
        self.history.execute(call, control).await
      }
      Some(Capability::Shell) => self.execute_shell(call, control).await,
      _ => ToolOutcome::Failed(format!("unknown tool: {}", call.name)),
    }
  }
}

impl SessionSlot {
  /// The session config with the tool list its switches give it. A tool the config names that the
  /// session could not run is refused.
  pub fn configure_tools(
    &self,
    mut config: SessionConfig,
  ) -> Result<SessionConfig, ApplicationError> {
    let switches = self.descriptor.read().unwrap().tools;
    if let Some(tool) = config.tools.iter().find(|tool| !switches.allow(&tool.name)) {
      return Err(ApplicationError::invalid(format!("no executor for tool {}", tool.name)));
    }
    let previous_tools = std::mem::take(&mut config.tools);
    for capability in Capability::ALL {
      if switches.advertised(capability) {
        config.tools.extend(self.tools.specifications(capability));
      }
    }
    // Preserve the active definition order and command-backed definitions until a generation
    // switch. Disabled capabilities are removed immediately by the caller's cleared target list.
    let mut ordered = Vec::new();
    for previous in previous_tools {
      if let Some(index) = config.tools.iter().position(|tool| tool.name == previous.name) {
        ordered.push(config.tools.remove(index));
      } else if Capability::from_tool(&previous.name).is_some_and(Capability::command_backed)
        && switches.allow(&previous.name)
      {
        ordered.push(previous);
      }
    }
    ordered.append(&mut config.tools);
    config.tools = ordered;
    Ok(config)
  }
  /// Turns optional tools on or off; the caller then reinstalls the session's tool list.
  pub async fn switch_tools(&self, changes: ToolChanges) -> Result<(), ApplicationError> {
    let previous = self.descriptor.read().unwrap().tools;
    let next = previous.apply_changes(changes);
    self.tools.switch(next).await?;
    self.descriptor.write().unwrap().tools = next;
    if previous.mcp
      && !next.mcp
      && let Some(app) = self.app.upgrade()
    {
      app.mcp.close_session(&self.get_descriptor().id);
    }
    Ok(())
  }
  fn environment_notice(
    &self,
    shell: &ShellCommand,
    cwd: &std::path::Path,
  ) -> Result<(), ApplicationError> {
    let text = crate::application::instructions::environment_instruction(shell, cwd);
    self.sender.enqueue_instruction_update(
      "wish_environment".into(),
      text.clone(),
      format!("Execution environment changed. Use these settings for subsequent commands:\n{text}"),
    )?;
    let _ = self.events.send(json!({"type":"history_changed"}));
    Ok(())
  }
  /// Sessions following global settings get a tail notice, never a rewritten active prefix.
  pub fn follow_global_shell(&self, global: &ShellCommand) {
    if self.descriptor.read().unwrap().shell_override.is_none() {
      let previous = self.effective_shell.read().unwrap().clone();
      if previous != *global {
        let cwd = self.get_descriptor().cwd;
        if let Err(error) = self.environment_notice(global, &cwd) {
          eprintln!("shell environment notice: {error}");
          return;
        }
        *self.effective_shell.write().unwrap() = global.clone();
      }
    }
  }
  /// Gives this session its own shell; only new commands use it.
  pub fn set_shell(
    &self,
    settings: Option<ShellSettings>,
    global: &ShellCommand,
  ) -> Result<(), ApplicationError> {
    let next = match &settings {
      Some(settings) => settings.resolve()?,
      None => global.clone(),
    };
    let previous = self.effective_shell.read().unwrap().clone();
    self.edit_descriptor(None, |descriptor| {
      descriptor.shell_override = settings;
      Ok(())
    })?;
    if previous != next {
      self.environment_notice(&next, &self.get_descriptor().cwd)?;
    }
    *self.effective_shell.write().unwrap() = next;
    Ok(())
  }
  pub fn set_working_directory(&self, cwd: std::path::PathBuf) -> Result<(), ApplicationError> {
    let cwd =
      std::fs::canonicalize(cwd).map_err(|error| ApplicationError::invalid(error.to_string()))?;
    if !cwd.is_dir() {
      return Err(ApplicationError::invalid("cwd must be a directory"));
    }
    if self.get_descriptor().cwd == cwd {
      return Ok(());
    }
    self.edit_descriptor(None, |descriptor| {
      descriptor.cwd = cwd.clone();
      Ok(())
    })?;
    self.environment_notice(&self.effective_shell.read().unwrap(), &cwd)?;
    *self.tools.shell_config.cwd.write().unwrap() = cwd;
    Ok(())
  }
}

fn make_fetch(
  slot: Weak<SessionSlot>,
  session_id: &str,
  directory: std::path::PathBuf,
) -> crate::application::pages::PageReader {
  let conversation = session_id.to_owned();
  crate::application::pages::PageReader::new(
    Arc::new(move |url, refresh, provider| {
      let owner = slot.upgrade();
      let app = owner.as_ref().and_then(|s| s.app.upgrade());
      let observer = owner.map(|s| s.retry_observer());
      let conversation = conversation.clone();
      Box::pin(async move {
        let app = app.ok_or("server is shutting down")?;
        let accounts = app.get_service_accounts();
        app
          .fetch
          .fetch(&url, refresh, &conversation, &accounts, observer, provider.as_deref())
          .await
      })
    }),
    directory,
  )
}

fn make_image_generate(slot: Weak<SessionSlot>) -> crate::tool::image_generate::ImageGenerateTool {
  let providers = provider_listing(slot.clone(), Capability::ImageGenerate);
  crate::tool::image_generate::ImageGenerateTool(
    Arc::new(move |request| {
      let owner = slot.upgrade();
      Box::pin(async move {
        let owner = owner.ok_or("session is closed")?;
        let app = owner.app.upgrade().ok_or("server is shutting down")?;
        app.images.generate(&request, &app.get_service_accounts(), &owner.blob_dir).await
      })
    }),
    providers,
  )
}

/// Read current service state on each discovery call; never embed credentials or
/// mutable provider lists in cached model tool definitions.
fn provider_listing(
  slot: Weak<SessionSlot>,
  capability: Capability,
) -> crate::tool::providers::Listing {
  Arc::new(move || {
    let owner = slot.upgrade().ok_or("session is closed")?;
    let app = owner.app.upgrade().ok_or("server is shutting down")?;
    let accounts = app.get_service_accounts();
    match capability {
      Capability::WebSearch => Ok(app.search.provider_catalog(&accounts)),
      Capability::WebFetch => Ok(app.fetch.provider_catalog(&accounts)),
      Capability::ImageGenerate => Ok(app.images.provider_catalog(&accounts)),
      Capability::VideoGenerate => Ok(app.videos.provider_catalog(&accounts)),
      Capability::AudioGenerate => Ok(app.audio.provider_catalog(&accounts)),
      Capability::AudioTranscribe => Ok(app.asr.provider_catalog(&accounts)),
      _ => Err("tool does not support provider discovery".into()),
    }
  })
}

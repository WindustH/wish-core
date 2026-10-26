//! The built-in tools of a session: which optional ones it has (its switches), the executor that
//! runs them, and the shell its commands run under.
use super::SessionSlot;
use crate::application::{
  blobs,
  config::ShellSettings,
  error::{ApplicationError, blocking},
};
use crate::{
  executor::{ExecutionControl, tool::ToolExecutor},
  protocol::{ContentBlock, Message},
  session::{Session, SessionConfig, ToolCall, ToolOutcome},
  tool::{
    ask_user::AskUserTool,
    history::HistoryTools,
    shell::{ShellCommand, ShellConfig, ShellTool},
    view_image::ViewImageTool,
    web_search::{Answer, SearchBackend, WebSearchTool},
  },
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::{
  Arc, Weak,
  atomic::{AtomicBool, Ordering},
};

const HISTORY_TOOLS: [&str; 3] = ["history_search", "history_read", "history_query"];
const SHELL_TOOLS: [&str; 6] =
  ["shell", "shell_start", "shell_edit", "shell_poll", "shell_write", "shell_kill"];

/// How an enabled capability is presented to the model.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolDelivery {
  // Compatibility for stored sessions without a delivery field; new-session defaults live in config.
  #[default]
  Tool,
  Command,
}
fn command_delivery() -> ToolDelivery {
  ToolDelivery::Command
}

/// The optional built-in tools of a session. History search and `view_image` are always there.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ToolSwitches {
  /// The simple `shell` tool and session task commands.
  pub shell: bool,
  #[serde(default = "command_delivery")]
  pub task_delivery: ToolDelivery,
  #[serde(default)]
  pub web_fetch_delivery: ToolDelivery,
  #[serde(default)]
  pub web_search_delivery: ToolDelivery,
  #[serde(default)]
  pub ask_user_delivery: ToolDelivery,
  /// Questions to the user: `ask_user`.
  pub ask_user: bool,
  /// Whether `wish mcp` in the session's shell may reach the MCP servers. The model's request is
  /// the same either way, so switching it keeps the prompt cache.
  pub mcp: bool,
  /// Searches of the web: `web_search`, answered by the configured search providers.
  pub web_search: bool,
  #[serde(default)]
  pub web_fetch: bool,
  /// Whether `wish skill` in the session's shell may read skills. Like `mcp`, it changes neither
  /// the tools nor the instructions, so switching it keeps the prompt cache.
  pub skills: bool,
  /// Whether `wish session` in the session's shell may see, make and message other sessions and
  /// groups. Like `mcp`, switching it keeps the prompt cache.
  pub sessions: bool,
}
impl ToolSwitches {
  /// These switches with a request's changes; what it leaves out stays as it is.
  pub fn apply_changes(self, changes: ToolChanges) -> Self {
    Self {
      shell: changes.shell.unwrap_or(self.shell),
      task_delivery: changes.task_delivery.unwrap_or(self.task_delivery),
      web_fetch_delivery: changes.web_fetch_delivery.unwrap_or(self.web_fetch_delivery),
      web_search_delivery: changes.web_search_delivery.unwrap_or(self.web_search_delivery),
      ask_user_delivery: changes.ask_user_delivery.unwrap_or(self.ask_user_delivery),
      ask_user: changes.ask_user.unwrap_or(self.ask_user),
      mcp: changes.mcp.unwrap_or(self.mcp),
      web_search: changes.web_search.unwrap_or(self.web_search),
      web_fetch: changes.web_fetch.unwrap_or(self.web_fetch),
      skills: changes.skills.unwrap_or(self.skills),
      sessions: changes.sessions.unwrap_or(self.sessions),
    }
  }
  /// Whether a tool of this name runs under these switches.
  fn allow(self, name: &str) -> bool {
    match name {
      "view_image" => true,
      "task" => self.shell,
      "ask_user" => self.ask_user,
      "web_search" => self.web_search,
      "web_fetch" => self.web_fetch,
      name if HISTORY_TOOLS.contains(&name) => true,
      name if SHELL_TOOLS.contains(&name) => self.shell,
      _ => false,
    }
  }
}
/// Switches a request sets, each optional.
#[derive(Clone, Copy, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolChanges {
  pub shell: Option<bool>,
  pub task_delivery: Option<ToolDelivery>,
  pub web_fetch_delivery: Option<ToolDelivery>,
  pub web_search_delivery: Option<ToolDelivery>,
  pub ask_user_delivery: Option<ToolDelivery>,
  pub ask_user: Option<bool>,
  pub mcp: Option<bool>,
  pub web_search: Option<bool>,
  pub web_fetch: Option<bool>,
  pub skills: Option<bool>,
  pub sessions: Option<bool>,
}
impl From<ToolSwitches> for ToolChanges {
  /// Every switch set as these are.
  fn from(switches: ToolSwitches) -> Self {
    Self {
      shell: Some(switches.shell),
      task_delivery: Some(switches.task_delivery),
      web_fetch_delivery: Some(switches.web_fetch_delivery),
      web_search_delivery: Some(switches.web_search_delivery),
      ask_user_delivery: Some(switches.ask_user_delivery),
      ask_user: Some(switches.ask_user),
      mcp: Some(switches.mcp),
      web_search: Some(switches.web_search),
      web_fetch: Some(switches.web_fetch),
      skills: Some(switches.skills),
      sessions: Some(switches.sessions),
    }
  }
}

pub struct SessionTools {
  /// Started the first time the shell is switched on, and kept while it is off so background
  /// commands finish and report as usual.
  shell: tokio::sync::OnceCell<ShellTool>,
  shell_config: ShellConfig,
  shell_on: AtomicBool,
  ask_user_on: AtomicBool,
  pub ask_user: AskUserTool,
  web_search_on: AtomicBool,
  web_fetch_on: AtomicBool,
  pub web_fetch: crate::tool::web_fetch::WebFetchTool,
  pub web_search: WebSearchTool,
  history: HistoryTools,
  slot: Weak<SessionSlot>,
}
impl SessionTools {
  pub fn new(
    slot: Weak<SessionSlot>,
    shell_config: ShellConfig,
    session: &Session,
    session_id: &str,
  ) -> Self {
    let web_fetch =
      make_fetch(slot.clone(), session_id, shell_config.capture_dir.join("web-fetch"));
    Self {
      shell: tokio::sync::OnceCell::new(),
      shell_config,
      shell_on: AtomicBool::new(false),
      ask_user_on: AtomicBool::new(false),
      ask_user: AskUserTool::new(questions_changed(slot.clone())),
      web_search_on: AtomicBool::new(false),
      web_fetch_on: AtomicBool::new(false),
      web_fetch,
      web_search: WebSearchTool::new(search_backend(slot.clone()), session_id.to_owned()),
      history: HistoryTools::new(session),
      slot,
    }
  }
  /// The shell tool, once it has been switched on.
  pub fn shell(&self) -> Option<&ShellTool> {
    self.shell.get()
  }
  pub async fn task_manager(&self) -> Result<&ShellTool, ApplicationError> {
    self
      .shell
      .get_or_try_init(|| ShellTool::new(self.shell_config.clone()))
      .await
      .map_err(|error| ApplicationError::invalid(format!("could not open tasks: {error}")))
  }
  /// Applies the session's switches, starting the shell tool the first time it is on.
  pub async fn switch(&self, switches: ToolSwitches) -> Result<(), ApplicationError> {
    if switches.shell {
      self.shell.get_or_try_init(|| ShellTool::new(self.shell_config.clone())).await.map_err(
        |error| ApplicationError::invalid(format!("could not start the shell: {error}")),
      )?;
    }
    self.shell_on.store(switches.shell, Ordering::Release);
    self.ask_user_on.store(switches.ask_user, Ordering::Release);
    self.web_search_on.store(switches.web_search, Ordering::Release);
    self.web_fetch_on.store(switches.web_fetch, Ordering::Release);
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
      self.notify_background(output, call.arguments["command"].clone(), epoch);
    }
    outcome
  }
  pub fn notify_background(
    &self,
    output: &serde_json::Value,
    command: serde_json::Value,
    epoch: Option<u64>,
  ) {
    if output["process"]["status"] != "running" {
      return;
    }
    let Some(shell) = self.shell() else { return };
    let Some(id) = output["execution_id"].as_str() else { return };
    if !shell.claim_notification(id) {
      return;
    }
    let Some(app) = self.slot.upgrade().and_then(|slot| slot.app.upgrade()) else { return };
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
      let message = Message::Developer {
        metadata: json!({"source":"background_execution_finished","execution_id":id,"completion":{"command":command,"result":result},"edits":shell.show(&id).ok().map(|task|task["edits"].clone())}),
        fixed: Some(false),
        content: vec![ContentBlock::Text {
          text: format!(
            "Background task {id} finished.\nCommand: {}\n{}",
            command.as_str().unwrap_or_default(), crate::task::output::render(&result, false)
          ),
        }],
      };
      let Some(slot) = slot.upgrade().filter(|slot| slot.require_live().is_ok()) else { return };
      if let Err(error) = slot.enqueue(message).await {
        eprintln!("background notification: {error}");
        return;
      }
      if slot.require_live().is_ok() {
        let _ = owner.events.send(json!({"type":"session_changed","id":slot.get_descriptor().id}));
        if epoch == Some(slot.get_schedule_epoch()) {
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
        let (directory, data) = (slot.image_dir.clone(), data_base64.clone());
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
  Arc::new(move |query| {
    let owner = slot.upgrade();
    let app = owner.as_ref().and_then(|slot| slot.app.upgrade());
    let observer = owner.map(|slot| slot.retry_observer());
    Box::pin(async move {
      let app = app.ok_or_else(|| vec!["the server is shutting down".to_owned()])?;
      let found = app.search.search(&app.get_service_accounts(), &query, observer).await?;
      Ok(Answer { provider: found.provider, name: found.name, results: found.results })
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
      let _ = slot.persist_index();
    });
  }
}
impl ToolExecutor for SessionTools {
  fn accepts_unadvertised_call(&self, name: &str) -> bool {
    // Recognize the route here; execute checks permissions and gives a specific disabled error.
    matches!(name, "task" | "web_fetch" | "web_search" | "ask_user")
  }

  async fn execute(&self, call: &ToolCall, control: &ExecutionControl) -> ToolOutcome {
    match call.name.as_str() {
      "view_image" => self.view_image(call, control).await,
      "task" => {
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
            text: crate::task::output::render(&output, false),
            output,
            metadata: serde_json::Value::Null,
            failed: false,
          },
          Some(Err(error)) => ToolOutcome::Failed(error.to_string()),
          None => ToolOutcome::Cancelled,
        }
      }
      "ask_user" => {
        if !self.ask_user_on.load(Ordering::Acquire) {
          return ToolOutcome::Failed("ask_user is not enabled for this session".into());
        }
        self.ask_user.execute(call, control).await
      }
      "web_fetch" => {
        if !self.web_fetch_on.load(Ordering::Acquire) {
          return ToolOutcome::Failed("web_fetch is not enabled for this session".into());
        }
        self.web_fetch.execute(call, control).await
      }
      "web_search" => {
        if !self.web_search_on.load(Ordering::Acquire) {
          return ToolOutcome::Failed("web_search is not enabled for this session".into());
        }
        self.web_search.execute(call, control).await
      }
      name if HISTORY_TOOLS.contains(&name) => self.history.execute(call, control).await,
      name if SHELL_TOOLS.contains(&name) => self.execute_shell(call, control).await,
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
    config.tools.extend(self.tools.history.get_specifications());
    config.tools.push(ViewImageTool.get_specification());
    if switches.shell
      && let Some(shell) = self.tools.shell()
    {
      config.tools.extend(shell.get_specifications());
    }
    if switches.shell && switches.task_delivery == ToolDelivery::Tool {
      config.tools.push(crate::application::task_access::specification());
    }
    if switches.ask_user && (!switches.shell || switches.ask_user_delivery == ToolDelivery::Tool) {
      config.tools.push(self.tools.ask_user.get_specification());
    }
    if switches.web_fetch && (!switches.shell || switches.web_fetch_delivery == ToolDelivery::Tool)
    {
      config.tools.push(self.tools.web_fetch.get_specification());
    }
    if switches.web_search
      && (!switches.shell || switches.web_search_delivery == ToolDelivery::Tool)
    {
      config.tools.push(self.tools.web_search.get_specification());
    }
    // Preserve the active definition order and command-backed definitions until a generation
    // switch. Disabled capabilities are removed immediately by the caller's cleared target list.
    let mut ordered = Vec::new();
    for previous in previous_tools {
      if let Some(index) = config.tools.iter().position(|tool| tool.name == previous.name) {
        ordered.push(config.tools.remove(index));
      } else if matches!(previous.name.as_str(), "task" | "web_fetch" | "web_search" | "ask_user")
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
) -> crate::tool::web_fetch::WebFetchTool {
  let conversation = session_id.to_owned();
  crate::tool::web_fetch::WebFetchTool::new(
    Arc::new(move |url, refresh| {
      let owner = slot.upgrade();
      let app = owner.as_ref().and_then(|s| s.app.upgrade());
      let observer = owner.map(|s| s.retry_observer());
      let conversation = conversation.clone();
      Box::pin(async move {
        let app = app.ok_or("server is shutting down")?;
        let accounts = app.get_service_accounts();
        let p = app.fetch.fetch(&url, refresh, &conversation, &accounts, observer, None).await?;
        Ok(crate::tool::web_fetch::FetchPage {
          url: p.url,
          title: p.title,
          content: p.content,
          provider: p.provider,
          fetched_at: p.fetched_at,
        })
      })
    }),
    directory,
  )
}

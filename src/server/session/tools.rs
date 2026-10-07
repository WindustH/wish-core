//! The built-in tools of a session: which optional ones it has (its switches), the executor that
//! runs them, and the shell its commands run under.
use super::SessionSlot;
use crate::server::{
  blobs,
  config::ShellSettings,
  error::{ApiError, blocking},
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
const SHELL_TOOLS: [&str; 5] =
  ["shell_start", "shell_edit", "shell_poll", "shell_write", "shell_kill"];

/// The optional built-in tools of a session. History search and `view_image` are always there.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ToolSwitches {
  /// Commands in the session's working directory: `shell_start`, `shell_edit` and the rest.
  pub shell: bool,
  /// Questions to the user: `ask_user`.
  pub ask_user: bool,
  /// Whether `wish mcp` in the session's shell may reach the MCP servers. The model's request is
  /// the same either way, so switching it keeps the prompt cache.
  pub mcp: bool,
  /// Searches of the web: `web_search`, answered by the configured search providers.
  pub web_search: bool,
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
      ask_user: changes.ask_user.unwrap_or(self.ask_user),
      mcp: changes.mcp.unwrap_or(self.mcp),
      web_search: changes.web_search.unwrap_or(self.web_search),
      skills: changes.skills.unwrap_or(self.skills),
      sessions: changes.sessions.unwrap_or(self.sessions),
    }
  }
  /// Whether a tool of this name runs under these switches.
  fn allow(self, name: &str) -> bool {
    match name {
      "view_image" => true,
      "ask_user" => self.ask_user,
      "web_search" => self.web_search,
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
  pub ask_user: Option<bool>,
  pub mcp: Option<bool>,
  pub web_search: Option<bool>,
  pub skills: Option<bool>,
  pub sessions: Option<bool>,
}
impl From<ToolSwitches> for ToolChanges {
  /// Every switch set as these are.
  fn from(switches: ToolSwitches) -> Self {
    Self {
      shell: Some(switches.shell),
      ask_user: Some(switches.ask_user),
      mcp: Some(switches.mcp),
      web_search: Some(switches.web_search),
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
    Self {
      shell: tokio::sync::OnceCell::new(),
      shell_config,
      shell_on: AtomicBool::new(false),
      ask_user_on: AtomicBool::new(false),
      ask_user: AskUserTool::new(questions_changed(slot.clone())),
      web_search_on: AtomicBool::new(false),
      web_search: WebSearchTool::new(search_backend(slot.clone()), session_id.to_owned()),
      history: HistoryTools::new(session),
      slot,
    }
  }
  /// The shell tool, once it has been switched on.
  pub fn shell(&self) -> Option<&ShellTool> {
    self.shell.get()
  }
  /// Applies the session's switches, starting the shell tool the first time it is on.
  pub async fn switch(&self, switches: ToolSwitches) -> Result<(), ApiError> {
    if switches.shell {
      self
        .shell
        .get_or_try_init(|| ShellTool::new(self.shell_config.clone()))
        .await
        .map_err(|error| ApiError::bad_request(format!("could not start the shell: {error}")))?;
    }
    self.shell_on.store(switches.shell, Ordering::Release);
    self.ask_user_on.store(switches.ask_user, Ordering::Release);
    self.web_search_on.store(switches.web_search, Ordering::Release);
    Ok(())
  }
  async fn execute_shell(&self, call: &ToolCall, control: &ExecutionControl) -> ToolOutcome {
    let Some(shell) = self.shell().filter(|_| self.shell_on.load(Ordering::Acquire)) else {
      return ToolOutcome::Failed("shell is not enabled for this session".into());
    };
    let epoch = self.slot.upgrade().map(|slot| slot.get_schedule_epoch());
    let outcome = shell.execute(call, control).await;
    let ToolOutcome::Success(output) = &outcome else {
      return outcome;
    };
    if call.name != "shell_start" || output["process"]["status"] != "running" {
      return outcome;
    }
    let Some(id) = output["execution_id"].as_str() else { return outcome };
    let Some(app) = self.slot.upgrade().and_then(|slot| slot.app.upgrade()) else {
      return outcome;
    };
    // A command left running in the background reports its end as a message, which starts a run
    // unless the session was interrupted since this one began.
    let (shell, id, slot, command) =
      (shell.clone(), id.to_owned(), self.slot.clone(), call.arguments["command"].clone());
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
        metadata: json!({"source":"background_execution_finished","execution_id":id,"completion":{"command":command,"result":result}}),
        fixed: Some(false),
        content: vec![ContentBlock::Text {
          text: format!(
            "A background shell execution reached a terminal state.\n{}",
            json!({"command":command,"result":result})
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
    outcome
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
        let saved =
          blocking(move || blobs::save_image(&directory, &data).map_err(ApiError::internal)).await;
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
    let app = slot.upgrade().and_then(|slot| slot.app.upgrade());
    Box::pin(async move {
      let app = app.ok_or_else(|| vec!["the server is shutting down".to_owned()])?;
      let found = app.search.search(&app.get_providers(), &query).await?;
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
  async fn execute(&self, call: &ToolCall, control: &ExecutionControl) -> ToolOutcome {
    match call.name.as_str() {
      "view_image" => self.view_image(call, control).await,
      "ask_user" => {
        if !self.ask_user_on.load(Ordering::Acquire) {
          return ToolOutcome::Failed("ask_user is not enabled for this session".into());
        }
        self.ask_user.execute(call, control).await
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
  pub fn configure_tools(&self, mut config: SessionConfig) -> Result<SessionConfig, ApiError> {
    let switches = self.descriptor.read().unwrap().tools;
    if let Some(tool) = config.tools.iter().find(|tool| !switches.allow(&tool.name)) {
      return Err(ApiError::bad_request(format!("no executor for tool {}", tool.name)));
    }
    config.tools.clear();
    config.tools.extend(self.tools.history.get_specifications());
    config.tools.push(ViewImageTool.get_specification());
    if switches.shell
      && let Some(shell) = self.tools.shell()
    {
      config.tools.extend(shell.get_specifications());
    }
    if switches.ask_user {
      config.tools.push(self.tools.ask_user.get_specification());
    }
    if switches.web_search {
      config.tools.push(self.tools.web_search.get_specification());
    }
    Ok(config)
  }
  /// Turns optional tools on or off; the caller then reinstalls the session's tool list.
  pub async fn switch_tools(&self, changes: ToolChanges) -> Result<(), ApiError> {
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
  /// Applies the application's new shell unless this session has its own.
  pub fn follow_global_shell(&self, global: &ShellCommand) {
    if self.descriptor.read().unwrap().shell_override.is_none() {
      *self.effective_shell.write().unwrap() = global.clone();
    }
  }
  /// Gives this session its own shell, or with None returns it to the application's.
  pub fn set_shell(
    &self,
    settings: Option<ShellSettings>,
    global: &ShellCommand,
  ) -> Result<(), ApiError> {
    if !self.descriptor.read().unwrap().tools.shell {
      return Err(ApiError::bad_request("this session has no shell tool"));
    }
    let next = match &settings {
      Some(settings) => settings.resolve()?,
      None => global.clone(),
    };
    *self.effective_shell.write().unwrap() = next;
    self.descriptor.write().unwrap().shell_override = settings;
    self.persist_index()
  }
}

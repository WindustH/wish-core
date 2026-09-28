use crate::server::{
  app::App,
  config::ToolSwitches,
  error::{ApiError, blocking},
};
use crate::{
  executor::{
    ExecutionControl,
    tool::{ToolCall, ToolExecutor, ToolOutcome},
  },
  protocol::{ContentBlock, Message},
  session::{Session, SessionHandle},
  tool::{
    ask_user::AskUserTool,
    search_history::SearchHistoryTool,
    shell::{ShellConfig, ShellTool},
    view_image::ViewImageTool,
  },
};
use serde_json::json;
use std::{
  path::PathBuf,
  sync::{
    Weak,
    atomic::{AtomicBool, Ordering},
  },
};

pub struct SessionTools {
  /// Started the first time the shell is switched on, and kept while it is off so background
  /// commands finish and report as usual.
  shell: tokio::sync::OnceCell<ShellTool>,
  shell_config: ShellConfig,
  shell_on: AtomicBool,
  ask_user_on: AtomicBool,
  pub ask_user: AskUserTool,
  history: SearchHistoryTool,
  app: Weak<App>,
  session_id: String,
  handle: SessionHandle,
  image_dir: PathBuf,
}
impl SessionTools {
  pub fn new(
    shell_config: ShellConfig,
    session: &Session,
    app: Weak<App>,
    session_id: String,
    image_dir: PathBuf,
  ) -> Self {
    Self {
      shell: tokio::sync::OnceCell::new(),
      shell_config,
      shell_on: AtomicBool::new(false),
      ask_user_on: AtomicBool::new(false),
      ask_user: AskUserTool::new(questions_changed(app.clone(), session_id.clone())),
      history: SearchHistoryTool::new(session),
      app,
      session_id,
      handle: session.create_handle(),
      image_dir,
    }
  }
  pub fn get_history_specifications(&self) -> Vec<crate::protocol::Tool> {
    self.history.get_specifications()
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
    Ok(())
  }
  async fn execute_shell(&self, call: &ToolCall, control: &ExecutionControl) -> ToolOutcome {
    let Some(shell) = self.shell().filter(|_| self.shell_on.load(Ordering::Acquire)) else {
      return ToolOutcome::Failed("shell is not enabled for this session".into());
    };
    let app = self.app.upgrade();
    let generation = match &app {
      Some(app) => app
        .get_session(&self.session_id)
        .await
        .ok()
        .map(|slot| slot.auto_run_generation.load(Ordering::SeqCst)),
      None => None,
    };
    let outcome = shell.execute(call, control).await;
    let ToolOutcome::Success(output) = &outcome else {
      return outcome;
    };
    let is_start = call.name == "shell_start";
    if !is_start || output["process"]["status"] != "running" {
      return outcome;
    }
    let (Some(id), Some(app)) = (output["execution_id"].as_str(), app) else {
      return outcome;
    };
    let (shell, id, handle, session_id, command) = (
      shell.clone(),
      id.to_owned(),
      self.handle.clone(),
      self.session_id.clone(),
      call.arguments["command"].clone(),
    );
    let owner = app.clone();
    app.tasks.spawn(async move {
      let result = tokio::select! {
        _ = owner.stop.cancelled() => return,
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
      if let Err(error) = blocking(move || Ok(handle.enqueue_message(message)?)).await {
        eprintln!("background notification: {error}");
        return;
      }
      if let Ok(slot) = owner.get_session(&session_id).await {
        let _ = owner.events.send(json!({"type":"session_changed","id":session_id}));
        if generation == Some(slot.auto_run_generation.load(Ordering::SeqCst)) {
          crate::server::http::content::schedule(owner, slot);
        }
      }
    });
    outcome
  }
}
/// Publishes the session's open questions whenever they change: its index record and a change
/// notice.
fn questions_changed(app: Weak<App>, session_id: String) -> impl Fn() + Send + Sync + 'static {
  move || {
    let Some(app) = app.upgrade() else { return };
    let (owner, session_id) = (app.clone(), session_id.clone());
    app.tasks.spawn(async move {
      if let Ok(slot) = owner.get_session(&session_id).await {
        let _ = slot.persist_index();
      }
    });
  }
}
impl ToolExecutor for SessionTools {
  async fn execute(&self, call: &ToolCall, control: &ExecutionControl) -> ToolOutcome {
    match call.name.as_str() {
      "view_image" => {
        let outcome = ViewImageTool.execute(call, control).await;
        if let ToolOutcome::SuccessWithInput { mut output, mut input } = outcome {
          for block in &input {
            if let ContentBlock::Image { data_base64, .. } = block {
              let (directory, data) = (self.image_dir.clone(), data_base64.clone());
              match blocking(move || {
                crate::server::media::save_image(&directory, &data)
                  .map_err(crate::server::error::ApiError::internal)
              })
              .await
              {
                Ok(path) => output["session_path"] = json!(path),
                Err(error) => return ToolOutcome::Failed(error.to_string()),
              }
            }
          }
          input.retain(|block| matches!(block, ContentBlock::Image { .. }));
          ToolOutcome::SuccessWithInput { output, input }
        } else {
          outcome
        }
      }
      "history_search" | "history_read" | "history_query" => {
        self.history.execute(call, control).await
      }
      "ask_user" => {
        if !self.ask_user_on.load(Ordering::Acquire) {
          return ToolOutcome::Failed("ask_user is not enabled for this session".into());
        }
        self.ask_user.execute(call, control).await
      }
      "shell_start" | "shell_edit" | "shell_poll" | "shell_write" | "shell_kill" => {
        self.execute_shell(call, control).await
      }
      _ => ToolOutcome::Failed(format!("unknown tool: {}", call.name)),
    }
  }
}

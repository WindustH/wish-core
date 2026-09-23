use crate::server::{app::App, error::blocking};
use serde_json::json;
use std::{
  path::PathBuf,
  sync::{Weak, atomic::Ordering},
};
use crate::{
  executor::{
    ExecutionControl,
    tool::{ToolCall, ToolExecutor, ToolOutcome},
  },
  protocol::{ContentBlock, Message},
  session::{Session, SessionHandle},
  tool::{search_history::SearchHistoryTool, shell::ShellTool, view_image::ViewImageTool},
};

pub struct SessionTools {
  pub shell: Option<ShellTool>,
  history: SearchHistoryTool,
  app: Weak<App>,
  session_id: String,
  handle: SessionHandle,
  image_dir: PathBuf,
}
impl SessionTools {
  pub fn new(
    shell: Option<ShellTool>,
    session: &Session,
    app: Weak<App>,
    session_id: String,
    image_dir: PathBuf,
  ) -> Self {
    Self {
      shell,
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
  async fn execute_shell(&self, call: &ToolCall, control: &ExecutionControl) -> ToolOutcome {
    let Some(shell) = &self.shell else {
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
      "shell_start" | "shell_poll" | "shell_write" | "shell_kill" => {
        self.execute_shell(call, control).await
      }
      _ => ToolOutcome::Failed(format!("unknown tool: {}", call.name)),
    }
  }
}

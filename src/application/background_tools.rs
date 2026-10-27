//! Ownership transfer for direct tools. Input wakes through a watch subscription, never a timer.
//! Detached work has a session/service lifetime, not the model run's cancellation lifetime.
use super::{
  error::{ApplicationError, blocking},
  session::SessionSlot,
};
use crate::{
  executor::{
    ExecutionControl,
    tool::{ToolExecutor, ToolInput},
  },
  protocol::{ContentBlock, Message},
  session::{SessionEvent, SessionSender, ToolCall, ToolOutcome},
};
use serde_json::{Value, json};
use std::{
  collections::HashMap,
  sync::{Arc, Mutex},
};
use tokio::sync::{Mutex as AsyncMutex, Semaphore, oneshot};
use tokio_util::task::TaskTracker;

const MAX_JOBS: usize = 32;
#[derive(Clone)]
pub struct BackgroundTools(Arc<Runtime>);
struct Runtime {
  registry: Mutex<Registry>,
  capacity: Arc<Semaphore>,
  tasks: TaskTracker,
}
#[derive(Default)]
struct Registry {
  closed: bool,
  jobs: HashMap<String, Job>,
}
struct Job {
  control: ExecutionControl,
  call: ToolCall,
  detached: bool,
}
enum Phase {
  Foreground,
  Detached,
  Completed,
}
struct ForegroundOwner {
  control: ExecutionControl,
  transferred: bool,
}
impl Drop for ForegroundOwner {
  fn drop(&mut self) {
    if !self.transferred {
      self.control.cancel();
    }
  }
}
struct Registration {
  owner: BackgroundTools,
  id: String,
}
impl Drop for Registration {
  fn drop(&mut self) {
    self.owner.0.registry.lock().unwrap().jobs.remove(&self.id);
  }
}
impl Default for BackgroundTools {
  fn default() -> Self {
    Self(Arc::new(Runtime {
      registry: Mutex::new(Registry::default()),
      capacity: Arc::new(Semaphore::new(MAX_JOBS)),
      tasks: TaskTracker::new(),
    }))
  }
}
impl BackgroundTools {
  pub fn is_active(&self) -> bool {
    !self.0.registry.lock().unwrap().jobs.is_empty()
  }
  pub fn snapshot(&self) -> Vec<Value> {
    self.0.registry.lock().unwrap().jobs.iter().filter(|(_, job)| job.detached)
      .map(|(id, job)| json!({"id":id,"call_id":job.call.call_id,"name":job.call.name,"status":"running"})).collect()
  }
  pub async fn shutdown(&self) {
    {
      let mut registry = self.0.registry.lock().unwrap();
      registry.closed = true;
      for job in registry.jobs.values() {
        job.control.cancel();
      }
    }
    self.0.tasks.close();
    self.0.tasks.wait().await;
  }
  pub async fn execute(
    &self,
    slot: Arc<SessionSlot>,
    call: &ToolCall,
    control: &ExecutionControl,
    mut input: ToolInput,
  ) -> ToolOutcome {
    if control.is_cancelled() {
      return ToolOutcome::Cancelled;
    }
    let Some(app) = slot.app.upgrade() else {
      return ToolOutcome::Failed("server is closed".into());
    };
    let Ok(permit) = self.0.capacity.clone().try_acquire_owned() else {
      return ToolOutcome::Failed("Too many running tools in this session; wait for background work to finish before starting more.".into());
    };
    let id = uuid::Uuid::new_v4().to_string();
    let worker_control = ExecutionControl::new();
    let mut ownership = ForegroundOwner { control: worker_control.clone(), transferred: false };
    let registration;
    let task_token;
    {
      let mut registry = self.0.registry.lock().unwrap();
      if registry.closed {
        return ToolOutcome::Cancelled;
      }
      registry.jobs.insert(
        id.clone(),
        Job { control: worker_control.clone(), call: call.clone(), detached: false },
      );
      registration = Registration { owner: self.clone(), id: id.clone() };
      task_token = self.0.tasks.token();
    }
    let epoch = slot.get_schedule_epoch();
    let phase = Arc::new(AsyncMutex::new(Phase::Foreground));
    let (tx, mut rx) = oneshot::channel();
    let worker = worker_control.clone();
    let (owner, caller, job_id, gate, server) =
      (slot.clone(), call.clone(), id.clone(), phase.clone(), app.clone());
    app.lifecycle.tasks.spawn(async move {
      let (_permit, _registration, _task_token) = (permit, registration, task_token);
      let work = owner.tools.execute(&caller, &worker);
      tokio::pin!(work);
      let outcome = tokio::select! {
        biased;
        _ = server.lifecycle.stop.cancelled() => { worker.cancel(); work.await }
        outcome = &mut work => outcome,
      };
      let mut phase = gate.lock().await;
      if matches!(*phase, Phase::Detached) {
        if let Err(error) = finish(&owner.sender, &job_id, &caller, &outcome).await {
          // The durable detached marker remains unresolved. Reopening reports Unknown, never replays.
          eprintln!("background tool completion: {error}");
        } else if owner.require_live().is_ok() {
          let _ = owner.events.send(json!({"type":"history_changed"}));
          let _ =
            server.events.send(json!({"type":"session_changed","id":owner.get_descriptor().id}));
          if epoch == owner.get_schedule_epoch() && !server.lifecycle.stop.is_cancelled() {
            owner.schedule(&server);
          }
        }
      }
      *phase = Phase::Completed;
      let _ = tx.send(outcome);
    });
    let wait_result = |result: Result<ToolOutcome, oneshot::error::RecvError>| {
      result.unwrap_or_else(|_| {
        ToolOutcome::Unknown(
          "Tool worker stopped without a confirmed result; inspect its effects before retrying."
            .into(),
        )
      })
    };
    tokio::select! {
      biased;
      _ = control.wait_for_cancellation() => { worker_control.cancel(); wait_result(rx.await) }
      result = &mut rx => wait_result(result),
      arrived = input.wait() => {
        if let Err(error) = arrived {
          eprintln!("input notification: {error}");
        } else {
          let mut state = phase.lock().await;
          if matches!(*state, Phase::Foreground) && !control.is_cancelled() {
            let sender = slot.sender.clone();
            let event = json!({"type":"background_tool_started","id":id,"call":call});
            match blocking(move || Ok(sender.record_application_event(event)?)).await {
              Ok(_) => {
                *state = Phase::Detached;
                ownership.transferred = true;
                if let Some(job) = self.0.registry.lock().unwrap().jobs.get_mut(&id) { job.detached = true; }
                return ToolOutcome::Report {
                  text: format!("Tool {} continues in background (job {id}). Read the newly queued input now. Do not repeat this operation; its result will be delivered when it finishes.", call.name),
                  output: json!({"background":true,"yielded_for_input":true,"job_id":id,"status":"running","call_id":call.call_id}),
                  metadata: json!({"background_tool":true,"job_id":id}), failed: false,
                };
              }
              Err(error) => eprintln!("background transfer refused: {error}"),
            }
          }
        }
        // A failed transfer leaves the original owner responsible for cancellation and settlement.
        tokio::select! {
          biased;
          _ = control.wait_for_cancellation() => { worker_control.cancel(); wait_result(rx.await) }
          result = &mut rx => wait_result(result),
        }
      }
    }
  }
}

fn completion(id: &str, call: &ToolCall, outcome: &ToolOutcome) -> Message {
  let result = outcome.encode_content();
  let rendered =
    result["model_text"].as_str().map(str::to_owned).unwrap_or_else(|| result.to_string());
  let mut text: String = rendered.chars().take(50 * 1024).collect();
  if text.len() < rendered.len() {
    text.push_str("\n[Full result retained in history]");
  }
  let mut content = vec![ContentBlock::Text {
    text: format!(
      "Background tool {} (job {id}, original call {}) finished.\n{text}",
      call.name, call.call_id
    ),
  }];
  if let ToolOutcome::SuccessWithInput { input, .. } = outcome {
    content.extend(input.clone());
  }
  Message::User {
    metadata: json!({"source":"background_tool_finished","job_id":id,
    "call_id":call.call_id,"tool_name":call.name,"result":result,"tool_metadata":outcome.metadata()}),
    content,
  }
}
async fn finish(
  sender: &SessionSender,
  id: &str,
  call: &ToolCall,
  outcome: &ToolOutcome,
) -> Result<(), ApplicationError> {
  let message = completion(id, call, outcome);
  let event = json!({"type":"background_tool_finished","id":id,"call":call,"result":outcome.encode_content(),"metadata":outcome.metadata()});
  let sender = sender.clone();
  blocking(move || {
    sender.record_application_input(event, message)?;
    Ok(())
  })
  .await
}
/// Runs before a reopened session can execute. A detached task cannot survive process death;
/// pair its durable marker with an Unknown completion instead of repeating possible side effects.
pub async fn recover(slot: &SessionSlot) -> Result<(), ApplicationError> {
  let reader = slot.reader.clone();
  let sender = &slot.sender;
  let (unfinished, shells) = blocking(move || {
    let mut pending = HashMap::<String, ToolCall>::new();
    let mut shells = HashMap::<String, Value>::new();
    for page in reader.get_events().pages(0) {
      for event in page?.items {
        if let SessionEvent::Application(value) = event.as_ref() {
          let Some(id) = value["id"].as_str() else { continue };
          match value["type"].as_str() {
            Some("background_execution_started") => {
              shells.insert(id.to_owned(), value["command"].clone());
            }
            Some("background_execution_finished") => {
              shells.remove(id);
            }
            Some("background_tool_started") => {
              let call = serde_json::from_value(value["call"].clone())
                .map_err(ApplicationError::internal)?;
              pending.insert(id.to_owned(), call);
            }
            Some("background_tool_finished") => {
              pending.remove(id);
            }
            _ => {}
          }
        }
      }
    }
    Ok((pending, shells))
  })
  .await?;
  for (id, command) in shells {
    // Opening the task manager settles persisted Running tasks as interrupted; never respawn.
    let shell = slot.tools.task_manager().await?;
    let result = shell.wait_for_completion(&id).await.unwrap_or_else(
      |error| json!({"execution_id":id,"error":error.to_string(),"process":{"status":"unknown"}}),
    );
    let edits = shell.show(&id).ok().map(|task| task["edits"].clone());
    finish_shell(sender, &id, command, result, edits).await?;
  }
  for (id, call) in unfinished {
    finish(sender, &id, &call, &ToolOutcome::Unknown(
      "Wish stopped before this background tool's completion was recorded. Inspect its effects before retrying; it was not restarted.".into())).await?;
  }
  Ok(())
}

/// Native shell completions keep their existing UI protocol, with the completion marker and
/// queued message committed in one transaction just like direct tools.
pub async fn finish_shell(
  sender: &SessionSender,
  id: &str,
  command: Value,
  result: Value,
  edits: Option<Value>,
) -> Result<(), ApplicationError> {
  let message = Message::Developer {
    metadata: json!({"source":"background_execution_finished","execution_id":id,"completion":{"command":command,"result":result},"edits":edits}),
    fixed: Some(false),
    content: vec![ContentBlock::Text {
      text: format!(
        "Background task {id} finished.\nCommand: {}\n{}",
        command.as_str().unwrap_or_default(),
        crate::tool::task_output::render(&result, false)
      ),
    }],
  };
  let event =
    json!({"type":"background_execution_finished","id":id,"command":command,"result":result});
  let sender = sender.clone();
  blocking(move || {
    sender.record_application_input(event, message)?;
    Ok(())
  })
  .await
}

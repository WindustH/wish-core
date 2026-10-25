//! Session creation, configuration, input, execution and history mutations.
use super::input::{self, Input};
use super::{config::ShellSettings, session::Operation};
use crate::application::error::{ApplicationError, blocking};
use crate::application::session_edit;
use crate::application::{
  app::App,
  session::{CreateSession, ToolChanges},
};
use crate::session::{EntryId, SessionError};
use crate::{
  protocol::Message,
  session::SessionConfig,
  tool::ask_user::{Delivery, late_answer_message},
};
use serde::Deserialize;
use serde_json::Value;
use serde_json::json;
use std::sync::Arc;

/// Body: any of `name`, `provider` (with `config`), `config` and `metadata`, with `If-Match` naming
/// the revision the change was made against. A rename alone never waits for an operation; while
/// one runs, only the model selection may change, and it applies at the operation's next boundary.
pub async fn update_session(
  app: Arc<App>,
  id: String,
  revision: Option<String>,
  input: Value,
) -> Result<Value, ApplicationError> {
  app.lifecycle.require_open()?;
  let slot = app.get_session(&id).await?;
  session_edit::update(app, slot, revision, input).await
}
pub async fn delete_session(app: Arc<App>, id: String) -> Result<(), ApplicationError> {
  session_edit::delete(&app, &id).await?;
  Ok(())
}
pub async fn cancel_input(
  app: Arc<App>,
  (id, entry): (String, usize),
) -> Result<(), ApplicationError> {
  let slot = app.get_session(&id).await?;
  blocking(move || {
    slot.require_live()?;
    slot.sender.cancel_queued_input(EntryId(entry)).map_err(|error| match error {
      SessionError::InvalidEntry(_) => {
        ApplicationError::conflict("input has already been consumed or cancelled")
      }
      error => error.into(),
    })?;
    slot.persist_index()?;
    Ok(())
  })
  .await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MoveInput {
  before: Option<usize>,
}
pub async fn move_input(
  app: Arc<App>,
  (id, entry): (String, usize),
  input: MoveInput,
) -> Result<(), ApplicationError> {
  let slot = app.get_session(&id).await?;
  blocking(move || {
    slot.require_live()?;
    slot.sender.move_queued_input(EntryId(entry), input.before.map(EntryId)).map_err(|error| {
      match error {
        SessionError::InvalidEntry(_) => {
          ApplicationError::conflict("queued input has already been consumed or cancelled")
        }
        error => error.into(),
      }
    })?;
    slot.persist_index()?;
    Ok(())
  })
  .await
}
/// Starts a new context generation that keeps only the fixed instructions at its front.
pub async fn clear_context(app: Arc<App>, id: String) -> Result<Value, ApplicationError> {
  let slot = app.get_session(&id).await?;
  let mut session = slot.lock_idle_or_conflict().await?;
  blocking(move || {
    slot.require_live()?;
    let generation = session.get_active_generation()?;
    let entries = session.reader().get_generation_entry_ids(generation.id)?;
    let mut prefix = Vec::new();
    let mut position = 0;
    while let Some(id) = entries.get(position)? {
      let entry = session.reader().get_entry(*id)?.ok_or_else(ApplicationError::not_found)?;
      if !entry.message.is_fixed_instruction() {
        break;
      }
      prefix.push(*id);
      position += 1;
    }
    session.prepare_standby_generation(prefix)?;
    session.activate_standby_generation()?;
    Ok(slot.publish(&session)?)
  })
  .await
}
/// Creates a copy of the session with its current context, settings and tool switches.
pub async fn fork(app: Arc<App>, id: String) -> Result<Value, ApplicationError> {
  let slot = app.get_session(&id).await?;
  let session = slot.lock_idle_or_conflict().await?;
  let descriptor = slot.get_descriptor();
  let mut config = session.get_config().clone();
  config.tools.clear();
  let request = session.build_request()?;
  // Summaries and compaction items stay context in the copy, as they are here.
  let active = session.get_active_generation()?;
  let mut initial_origins = Vec::new();
  for id in session.reader().read_generation_entry_ids(active.id)? {
    let entry = session.reader().get_entry(id)?.ok_or(SessionError::InvalidEntry(id))?;
    initial_origins.push(entry.origin);
  }
  let input = CreateSession {
    created_by: None,
    folder: app.management.folder_of(&descriptor.id)?,
    name: format!("{} (copy)", descriptor.name),
    provider: descriptor.provider,
    cwd: descriptor.cwd,
    tools: ToolChanges::from(descriptor.tools),
    config,
    metadata: session.get_metadata().clone(),
    initial_messages: request.conversation,
    initial_origins,
  };
  drop(session);
  Ok(app.create_session(input).await?.describe())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Answer {
  call_id: String,
  #[serde(default)]
  answers: Option<Vec<Value>>,
  #[serde(default)]
  skip: bool,
}
pub async fn create(app: Arc<App>, input: CreateSession) -> Result<Value, ApplicationError> {
  Ok(app.create_session(input).await?.describe())
}
pub async fn enqueue(
  app: Arc<App>,
  id: String,
  mut message: Message,
) -> Result<Value, ApplicationError> {
  app.lifecycle.require_open()?;
  message.normalize_new_input();
  let slot = app.get_session(&id).await?;
  let entry = slot.enqueue(message).await?;
  slot.touch()?;
  Ok(json!({"entry":entry}))
}
pub async fn set_config(
  app: Arc<App>,
  id: String,
  config: SessionConfig,
) -> Result<Value, ApplicationError> {
  app.lifecycle.require_open()?;
  let slot = app.get_session(&id).await?;
  let config = slot.configure_tools(config)?;
  let mut session = slot.lock_idle_or_conflict().await?;
  blocking(move || {
    slot.require_live()?;
    crate::application::instructions::persist_agent_instructions(&mut session, config)?;
    Ok(slot.publish(&session)?)
  })
  .await
}
pub async fn set_tools(
  app: Arc<App>,
  id: String,
  changes: ToolChanges,
) -> Result<Value, ApplicationError> {
  app.lifecycle.require_open()?;
  let slot = app.get_session(&id).await?;
  let mut session = slot.lock_idle_or_conflict().await?;
  slot.require_live()?;
  slot.switch_tools(changes).await?;
  // The tool list is rebuilt from the switches, so a tool just switched off is not kept.
  let mut config = session.get_config().clone();
  config.tools.clear();
  let config = slot.configure_tools(config)?;
  blocking(move || {
    crate::application::instructions::persist_agent_instructions(&mut session, config)?;
    Ok(slot.publish(&session)?)
  })
  .await
}
pub async fn answer(app: Arc<App>, id: String, input: Answer) -> Result<Value, ApplicationError> {
  app.lifecycle.require_open()?;
  let slot = app.get_session(&id).await?;
  let delivered =
    match slot.tools.ask_user.answer(&input.call_id, input.answers.as_deref(), input.skip)? {
      Delivery::Now => "now",
      Delivery::Dropped => "dropped",
      // The call timed out and the agent moved on: the answers follow as a message, like a
      // background command's report, and wake the session.
      Delivery::Later { questions, answers } => {
        slot.enqueue(late_answer_message(&input.call_id, &questions, &answers)).await?;
        slot.schedule(&app);
        "later"
      }
    };
  slot.touch()?;
  Ok(json!({"delivered": delivered}))
}
pub async fn set_shell(
  app: Arc<App>,
  id: String,
  settings: Option<ShellSettings>,
) -> Result<Value, ApplicationError> {
  app.lifecycle.require_open()?;
  let slot = app.get_session(&id).await?;
  let global = app.shell.read().unwrap().clone();
  blocking(move || {
    slot.require_live()?;
    slot.set_shell(settings, &global)?;
    Ok(slot.describe())
  })
  .await
}
pub async fn set_metadata(
  app: Arc<App>,
  id: String,
  metadata: Value,
) -> Result<Value, ApplicationError> {
  app.lifecycle.require_open()?;
  let slot = app.get_session(&id).await?;
  let mut session = slot.lock_idle_or_conflict().await?;
  blocking(move || {
    slot.require_live()?;
    session.set_metadata(metadata)?;
    Ok(slot.publish(&session)?)
  })
  .await
}
pub async fn run(app: Arc<App>, id: String) -> Result<Value, ApplicationError> {
  start(app, id, Operation::Run).await
}
pub async fn compact(app: Arc<App>, id: String) -> Result<Value, ApplicationError> {
  start(app, id, Operation::Compact).await
}
async fn start(app: Arc<App>, id: String, operation: Operation) -> Result<Value, ApplicationError> {
  let slot = app.get_session(&id).await?;
  slot.start(&app, operation).await?;
  Ok(json!({"accepted":true}))
}
pub async fn interrupt(app: Arc<App>, id: String) -> Result<Value, ApplicationError> {
  let slot = app.get_session(&id).await?;
  let requested = slot.interrupt_or_settle()?;
  // The scheduler waits for the cancelled run to release the session, then consumes pending
  // inputs. The interrupt invalidated older schedulers; only this fresh one resumes.
  slot.schedule(&app);
  Ok(json!({"requested": requested}))
}
pub async fn submit_input(
  app: Arc<App>,
  id: String,
  input: Input,
) -> Result<Value, ApplicationError> {
  app.lifecycle.require_open()?;
  let slot = app.get_session(&id).await?;
  let message = input::message(&app.data_dir.blobs(&id), input).await?;
  let entry = slot.enqueue(message).await?;
  slot.touch()?;
  slot.schedule(&app);
  let reader = slot.reader.clone();
  let first_user_message = crate::application::error::blocking(move || {
    Ok(super::naming::first_user(&reader)? == Some(entry))
  })
  .await?;
  Ok(json!({"entry":entry,"first_user_message":first_user_message}))
}

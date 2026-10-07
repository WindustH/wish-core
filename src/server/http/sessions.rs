//! Sessions over HTTP: the list, creating and reading one, its settings, running it, its live
//! events, and pages of its stored lists - entries, queue, generations and model calls.
use crate::server::{
  app::App,
  config::ShellSettings,
  error::{ApiError, blocking},
  management::SessionQuery,
  session::{CreateSession, Operation, ToolChanges},
};
use crate::{
  protocol::Message,
  session::SessionConfig,
  storage::{ReadList, StoredValue},
  tool::ask_user::{Delivery, late_answer_message},
};
use axum::{
  Json,
  extract::{Path, Query, State},
  http::StatusCode,
  response::{
    Sse,
    sse::{Event, KeepAlive},
  },
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{convert::Infallible, sync::Arc};
use tokio::sync::broadcast::error::RecvError;

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Page {
  pub start: u64,
  pub limit: usize,
}
impl Default for Page {
  fn default() -> Self {
    Self { start: 0, limit: 50 }
  }
}
async fn read_page<T: StoredValue + Serialize>(
  list: ReadList<T>,
  page: Page,
) -> Result<Json<Value>, ApiError> {
  blocking(move || {
    let page = list.read_page(page.start, page.limit)?;
    Ok(Json(json!({"start":page.start,"items":page.items,"next":page.next})))
  })
  .await
}
pub async fn list(
  State(app): State<Arc<App>>,
  Query(query): Query<SessionQuery>,
) -> Result<Json<Value>, ApiError> {
  let management = app.management.clone();
  blocking(move || Ok(Json(management.list(query)?))).await
}
/// What a user browses: sessions and groups, newest first.
pub async fn conversations(
  State(app): State<Arc<App>>,
  Query(query): Query<SessionQuery>,
) -> Result<Json<Value>, ApiError> {
  let management = app.management.clone();
  blocking(move || Ok(Json(management.conversations(query)?))).await
}
pub async fn create(
  State(app): State<Arc<App>>,
  Json(input): Json<CreateSession>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
  Ok((StatusCode::CREATED, Json(app.create_session(input).await?.describe())))
}
pub async fn get(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(app.get_session(&id).await?.describe()))
}
pub async fn enqueue(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Json(mut message): Json<Message>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
  app.lifecycle.require_open()?;
  message.normalize_new_input();
  let slot = app.get_session(&id).await?;
  let entry = slot.enqueue(message).await?;
  slot.touch()?;
  Ok((StatusCode::CREATED, Json(json!({"entry":entry}))))
}
pub async fn set_config(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Json(config): Json<SessionConfig>,
) -> Result<Json<Value>, ApiError> {
  app.lifecycle.require_open()?;
  let slot = app.get_session(&id).await?;
  let config = slot.configure_tools(config)?;
  let mut session = slot.lock_idle_or_conflict().await?;
  blocking(move || {
    slot.require_live()?;
    session.set_config(config)?;
    Ok(Json(slot.publish(&session)?))
  })
  .await
}
/// Body: `{"shell", "ask_user", "mcp", "web_search"}`, each optional. Turns the session's optional
/// tools on or off.
pub async fn set_tools(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Json(changes): Json<ToolChanges>,
) -> Result<Json<Value>, ApiError> {
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
    session.set_config(config)?;
    Ok(Json(slot.publish(&session)?))
  })
  .await
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
/// Body: `{"call_id", "answers"}`, or `{"call_id", "skip": true}`. Answers an `ask_user` form.
pub async fn answer(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Json(input): Json<Answer>,
) -> Result<Json<Value>, ApiError> {
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
  Ok(Json(json!({"delivered": delivered})))
}
/// Body: `{"program", "args"}` for the session's own shell, or null to follow the application's.
pub async fn set_shell(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Json(settings): Json<Option<ShellSettings>>,
) -> Result<Json<Value>, ApiError> {
  app.lifecycle.require_open()?;
  let slot = app.get_session(&id).await?;
  let global = app.shell.read().unwrap().clone();
  blocking(move || {
    slot.require_live()?;
    slot.set_shell(settings, &global)?;
    Ok(Json(slot.describe()))
  })
  .await
}
pub async fn set_metadata(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Json(metadata): Json<Value>,
) -> Result<Json<Value>, ApiError> {
  app.lifecycle.require_open()?;
  let slot = app.get_session(&id).await?;
  let mut session = slot.lock_idle_or_conflict().await?;
  blocking(move || {
    slot.require_live()?;
    session.set_metadata(metadata)?;
    Ok(Json(slot.publish(&session)?))
  })
  .await
}
pub async fn run(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
  start(app, id, Operation::Run).await
}
pub async fn compact(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
  start(app, id, Operation::Compact).await
}
async fn start(
  app: Arc<App>,
  id: String,
  operation: Operation,
) -> Result<(StatusCode, Json<Value>), ApiError> {
  let slot = app.get_session(&id).await?;
  slot.start(&app, operation).await?;
  Ok((StatusCode::ACCEPTED, Json(json!({"accepted":true}))))
}
pub async fn interrupt(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
  let slot = app.get_session(&id).await?;
  let requested = slot.interrupt_or_settle()?;
  // The scheduler waits for the cancelled run to release the session, then consumes pending
  // inputs. The interrupt invalidated older schedulers; only this fresh one resumes.
  slot.schedule(&app);
  Ok(Json(json!({"requested": requested})))
}
pub async fn entries(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Query(page): Query<Page>,
) -> Result<Json<Value>, ApiError> {
  read_page(app.get_session(&id).await?.reader.get_entries(), page).await
}
pub async fn queue(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Query(page): Query<Page>,
) -> Result<Json<Value>, ApiError> {
  read_page(app.get_session(&id).await?.reader.get_message_queue(), page).await
}
pub async fn generations(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Query(page): Query<Page>,
) -> Result<Json<Value>, ApiError> {
  read_page(app.get_session(&id).await?.reader.get_generations(), page).await
}
pub async fn generation_entries(
  State(app): State<Arc<App>>,
  Path((id, generation)): Path<(String, u64)>,
  Query(page): Query<Page>,
) -> Result<Json<Value>, ApiError> {
  let reader = app.get_session(&id).await?.reader.clone();
  let list = blocking(move || {
    let generation = reader.get_generations().get(generation)?.ok_or_else(ApiError::not_found)?;
    Ok(reader.get_generation_entry_ids(generation.id)?)
  })
  .await?;
  read_page(list, page).await
}
pub async fn calls(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Query(page): Query<Page>,
) -> Result<Json<Value>, ApiError> {
  read_page(app.get_session(&id).await?.reader.get_model_calls(), page).await
}
/// A `snapshot` of the session and its live preview first, then each change. A client that fell
/// behind gets a fresh snapshot; the stream ends after reporting the session's deletion.
pub async fn events(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
) -> Result<Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>>, ApiError> {
  let slot = app.get_session(&id).await?;
  let (receiver, snapshot) = slot.subscribe_live();
  let snapshot = Some(snapshot);
  // It ends there because it holds the slot, and nothing else would ever be sent on it.
  let stream = futures_util::stream::unfold(
    (receiver, app.lifecycle.stop.clone(), snapshot, slot, false),
    |(mut receiver, stop, mut snapshot, slot, deleted)| async move {
      if deleted {
        return None;
      }
      let value = if let Some(value) = snapshot.take() {
        value
      } else {
        tokio::select! {
          _ = stop.cancelled() => return None,
          result = receiver.recv() => match result {
            Ok(value) => value,
            Err(RecvError::Lagged(_)) => {
              let (fresh, snapshot) = slot.subscribe_live();
              receiver = fresh;
              snapshot
            }
            Err(RecvError::Closed) => return None,
          }
        }
      };
      let deleted = value["type"] == "deleted";
      Some((
        Ok(Event::default().event("wish").data(value.to_string())),
        (receiver, stop, snapshot, slot, deleted),
      ))
    },
  );
  Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

use crate::server::{
  app::App,
  error::{ApiError, blocking},
  session::{CreateSession, SessionSlot},
};
use crate::{
  executor::ExecutionControl,
  protocol::Message,
  session::{
    EntryId, SessionConfig,
    history::query::{HistoryFilter, HistoryPageRequest, HistorySearch},
  },
  storage::{ReadList, StoredValue},
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
pub async fn read_page<T: StoredValue + Serialize>(
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
  Query(query): Query<crate::server::management::SessionQuery>,
) -> Result<Json<Value>, ApiError> {
  let index = app.index.clone();
  blocking(move || Ok(Json(index.list(query)?))).await
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
  app.require_open()?;
  message.normalize_new_input();
  let slot = app.get_session(&id).await?;
  let id = blocking(move || Ok(slot.handle.enqueue_message(message)?)).await?;
  Ok((StatusCode::CREATED, Json(json!({"entry":id}))))
}
pub async fn set_config(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Json(config): Json<SessionConfig>,
) -> Result<Json<Value>, ApiError> {
  app.require_open()?;
  let slot = app.get_session(&id).await?;
  let config = slot.configure_tools(config)?;
  let mut session =
    slot.session.clone().try_lock_owned().map_err(|_| ApiError::conflict("session is running"))?;
  blocking(move || {
    slot.require_live()?;
    session.set_config(config)?;
    slot.update_snapshot(&session);
    slot.persist_index()?;
    Ok(Json(slot.describe()))
  })
  .await
}
pub async fn set_metadata(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Json(metadata): Json<Value>,
) -> Result<Json<Value>, ApiError> {
  app.require_open()?;
  let slot = app.get_session(&id).await?;
  let mut session =
    slot.session.clone().try_lock_owned().map_err(|_| ApiError::conflict("session is running"))?;
  blocking(move || {
    slot.require_live()?;
    session.set_metadata(metadata)?;
    slot.update_snapshot(&session);
    slot.persist_index()?;
    Ok(Json(slot.describe()))
  })
  .await
}
pub async fn run(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
  start(app, id, false).await
}
pub async fn compact(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
  start(app, id, true).await
}
async fn start(
  app: Arc<App>,
  id: String,
  compact: bool,
) -> Result<(StatusCode, Json<Value>), ApiError> {
  let slot = app.get_session(&id).await?;
  let (provider, provider_id) = slot.execution_provider(&app)?;
  let mut session =
    slot.session.clone().try_lock_owned().map_err(|_| ApiError::conflict("session is running"))?;
  slot.require_live()?;
  if !session.get_state().is_stable() {
    if slot.control.lock().unwrap().is_none() {
      session.settle_interrupted().map_err(ApiError::internal)?;
      slot.update_snapshot(&session);
    } else {
      return Err(ApiError::conflict("session has unfinished execution; inspect persisted state"));
    }
  }
  if compact && session.get_config().compaction.is_none() {
    return Err(ApiError::bad_request("session has no compaction config"));
  }
  let closing = app.closing.lock().unwrap();
  if *closing {
    return Err(ApiError::conflict("server is shutting down"));
  }
  if !compact {
    session.resume()?;
  }
  slot.update_snapshot(&session);
  slot.persist_index()?;
  let control = ExecutionControl::new();
  *slot.control.lock().unwrap() = Some(control.clone());
  {
    let mut status = slot.status.lock().unwrap();
    status["running"] = json!(true);
    status.as_object_mut().unwrap().remove("state");
  }
  app.tasks.spawn(slot.execute(provider, provider_id, session, control, compact));
  Ok((StatusCode::ACCEPTED, Json(json!({"accepted":true}))))
}
pub async fn interrupt(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
  let slot = app.get_session(&id).await?;
  let mut requested = slot.interrupt();
  if !requested {
    if let Ok(mut session) = slot.session.try_lock() {
      if !session.get_state().is_stable() {
        session.settle_interrupted().map_err(ApiError::internal)?;
        slot.update_snapshot(&session);
        let _ = slot.persist_index();
        requested = true;
      }
    }
  }
  // The scheduler waits for the cancelled run to release the session, then consumes
  // pending inputs. interrupt() invalidates older schedulers; only this fresh one resumes.
  super::content::schedule(app, slot);
  Ok(Json(json!({"requested": requested})))
}
pub async fn entries(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Query(page): Query<Page>,
) -> Result<Json<Value>, ApiError> {
  read_page(app.get_session(&id).await?.entries.clone(), page).await
}
pub async fn queue(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Query(page): Query<Page>,
) -> Result<Json<Value>, ApiError> {
  let slot = app.get_session(&id).await?;
  let list = slot.queue.lock().unwrap().clone();
  read_page(list, page).await
}
pub async fn generations(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Query(page): Query<Page>,
) -> Result<Json<Value>, ApiError> {
  read_page(app.get_session(&id).await?.generations.clone(), page).await
}
pub async fn generation_entries(
  State(app): State<Arc<App>>,
  Path((id, generation)): Path<(String, u64)>,
  Query(page): Query<Page>,
) -> Result<Json<Value>, ApiError> {
  let slot = app.get_session(&id).await?;
  let storage = app.storage.clone();
  let list = blocking(move || {
    let generation = slot.generations.get(generation)?.ok_or_else(ApiError::not_found)?;
    Ok(storage.open_list::<EntryId>(&generation.entries).read_only())
  })
  .await?;
  read_page(list, page).await
}
pub async fn calls(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Query(page): Query<Page>,
) -> Result<Json<Value>, ApiError> {
  read_page(app.get_session(&id).await?.calls.clone(), page).await
}
#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct HistoryQuery {
  filter: HistoryFilter,
  page: HistoryPageRequest,
}
pub async fn query_history(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Json(query): Json<HistoryQuery>,
) -> Result<Json<Value>, ApiError> {
  let reader = app.get_session(&id).await?.history.clone();
  blocking(move || Ok(Json(json!(reader.query_history(query.filter, query.page)?)))).await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Search {
  #[serde(default)]
  filter: HistoryFilter,
  query: HistorySearch,
}
pub async fn search_history(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Json(query): Json<Search>,
) -> Result<Json<Value>, ApiError> {
  let reader = app.get_session(&id).await?.history.clone();
  blocking(move || Ok(Json(json!(reader.search_history(query.query, query.filter)?)))).await
}
#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Around {
  before: usize,
  after: usize,
}
pub async fn read_history(
  State(app): State<Arc<App>>,
  Path((id, sequence)): Path<(String, u64)>,
  Query(around): Query<Around>,
) -> Result<Json<Value>, ApiError> {
  let reader = app.get_session(&id).await?.history.clone();
  blocking(move || {
    if reader.read_history_item(sequence)?.is_none() {
      return Err(ApiError::not_found());
    }
    Ok(Json(json!(reader.read_history_around(sequence, around.before, around.after)?)))
  })
  .await
}
pub async fn events(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
) -> Result<Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>>, ApiError> {
  let slot: Arc<SessionSlot> = app.get_session(&id).await?;
  let (receiver, snapshot) = slot.subscribe_live();
  let snapshot = Some(snapshot);
  let stream = futures_util::stream::unfold(
    (receiver, app.stop.clone(), snapshot, slot),
    |(mut receiver, stop, mut snapshot, slot)| async move {
      let value = if let Some(value) = snapshot.take() {
        value
      } else {
        tokio::select! {
          _=stop.cancelled()=>return None,
          result=receiver.recv()=>match result {
            Ok(value)=>value,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_))=>{
              let (fresh, snapshot) = slot.subscribe_live();
              receiver = fresh;
              snapshot
            },
            Err(tokio::sync::broadcast::error::RecvError::Closed)=>return None,
          }
        }
      };
      Some((
        Ok(Event::default().event("wish").data(value.to_string())),
        (receiver, stop, snapshot, slot),
      ))
    },
  );
  Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

//! HTTP adapters; domain operations live in the application layer.
use crate::application::session_commands::Answer;
use crate::application::session_queries::SessionQuery;
use crate::application::session_queries::{Page, StructurePage};
use crate::application::{
  app::App,
  config::ShellSettings,
  session::{CreateSession, ToolChanges},
};
use crate::application::{session_commands, session_queries};
use crate::server::error::ApiError;
use crate::{protocol::Message, session::SessionConfig};
use axum::{
  Json,
  extract::{Path, Query, State},
  http::StatusCode,
  response::{
    Sse,
    sse::{Event, KeepAlive},
  },
};
use futures_util::StreamExt;
use serde_json::Value;
use std::{convert::Infallible, sync::Arc};

pub async fn list(
  State(app): State<Arc<App>>,
  Query(query): Query<SessionQuery>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(session_queries::list(app, query).await?))
}
pub async fn conversations(
  State(app): State<Arc<App>>,
  Query(query): Query<SessionQuery>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(session_queries::conversations(app, query).await?))
}
pub async fn create(
  State(app): State<Arc<App>>,
  Json(input): Json<CreateSession>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
  Ok((StatusCode::CREATED, Json(session_commands::create(app, input).await?)))
}
pub async fn get(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(session_queries::get(app, id).await?))
}
pub async fn enqueue(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Json(message): Json<Message>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
  Ok((StatusCode::CREATED, Json(session_commands::enqueue(app, id, message).await?)))
}
pub async fn set_config(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Json(config): Json<SessionConfig>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(session_commands::set_config(app, id, config).await?))
}
pub async fn set_tools(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Json(changes): Json<ToolChanges>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(session_commands::set_tools(app, id, changes).await?))
}
pub async fn answer(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Json(input): Json<Answer>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(session_commands::answer(app, id, input).await?))
}
pub async fn set_shell(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Json(settings): Json<Option<ShellSettings>>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(session_commands::set_shell(app, id, settings).await?))
}
pub async fn set_metadata(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Json(metadata): Json<Value>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(session_commands::set_metadata(app, id, metadata).await?))
}
pub async fn run(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
  Ok((StatusCode::ACCEPTED, Json(session_commands::run(app, id).await?)))
}
pub async fn compact(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
  Ok((StatusCode::ACCEPTED, Json(session_commands::compact(app, id).await?)))
}
pub async fn interrupt(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(session_commands::interrupt(app, id).await?))
}
pub async fn entries(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Query(page): Query<Page>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(session_queries::entries(app, id, page).await?))
}
pub async fn queue(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Query(page): Query<Page>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(session_queries::queue(app, id, page).await?))
}
pub async fn generations(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Query(page): Query<Page>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(session_queries::generations(app, id, page).await?))
}
pub async fn generation_entries(
  State(app): State<Arc<App>>,
  Path((id, generation)): Path<(String, u64)>,
  Query(page): Query<Page>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(session_queries::generation_entries(app, (id, generation), page).await?))
}
pub async fn calls(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Query(page): Query<Page>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(session_queries::calls(app, id, page).await?))
}
pub async fn events(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
) -> Result<Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>>, ApiError> {
  let stream = crate::application::events::session_events(app, id)
    .await?
    .map(|value| Ok(Event::default().event("wish").data(value.to_string())));
  Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}
pub async fn inspect_context(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(session_queries::inspect_context(app, id).await?))
}
pub async fn generation_structure(
  State(app): State<Arc<App>>,
  Path((id, generation)): Path<(String, usize)>,
  Query(page): Query<StructurePage>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(session_queries::generation_structure(app, (id, generation), page).await?))
}

//! HTTP adapters; domain operations live in the application layer.
use crate::application::app::App;
use crate::application::runtime_status;
use crate::server::error::ApiError;
use axum::{
  Json,
  extract::State,
  response::{
    Sse,
    sse::{Event, KeepAlive},
  },
};
use futures_util::StreamExt;
use serde_json::Value;
use std::{convert::Infallible, sync::Arc};

pub async fn status(State(app): State<Arc<App>>) -> Result<Json<Value>, ApiError> {
  Ok(Json(runtime_status::status(app).await?))
}
pub async fn events(
  State(app): State<Arc<App>>,
) -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
  let stream = crate::application::events::application_events(app)
    .await
    .map(|value| Ok(Event::default().event("wish").data(value.to_string())));
  Sse::new(stream).keep_alive(KeepAlive::default())
}

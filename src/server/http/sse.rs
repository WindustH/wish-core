//! A model call's answer over HTTP: JSON when it came whole, else its stream relayed as server-sent
//! events - `model_event` for each stream event, then `done`, or `error` when the stream fails or
//! shutdown begins.
use crate::executor::model::{CallResponse, ModelStream};
use crate::server::error::ApiError;
use axum::{
  Json,
  response::{
    IntoResponse, Response, Sse,
    sse::{Event, KeepAlive},
  },
};
use serde_json::json;
use std::convert::Infallible;
use tokio_util::sync::CancellationToken;

pub fn model_response<S: ModelStream + 'static>(
  response: CallResponse<S>,
  stop: CancellationToken,
) -> Response {
  let stream = match response {
    CallResponse::Complete(response) => return Json(response).into_response(),
    CallResponse::Stream(stream) => stream,
  };
  let events =
    futures_util::stream::unfold((stream, stop, false), |(mut stream, stop, done)| async move {
      if done {
        return None;
      }
      let result = tokio::select! {
        _ = stop.cancelled() => Err(ApiError::shutting_down().message),
        result = stream.next() => result.map_err(|e| e.to_string()),
      };
      let (event, done) = match result {
        Ok(Some(event)) => (
          Event::default().event("model_event").data(serde_json::to_string(&event).unwrap()),
          false,
        ),
        Ok(None) => (Event::default().event("done").data("{}"), true),
        Err(error) => {
          (Event::default().event("error").data(json!({"message":error}).to_string()), true)
        }
      };
      Some((Ok::<_, Infallible>(event), (stream, stop, done)))
    });
  Sse::new(events).keep_alive(KeepAlive::default()).into_response()
}

//! One read-only question against a committed session snapshot; disconnect cancels the call.
use crate::server::{
  app::App,
  error::{ApiError, blocking},
  media::SessionModel,
};
use crate::{
  executor::model::{CallResponse, ModelCaller},
  protocol::{ContentBlock, Message},
};
use axum::{
  Json,
  extract::{Path, State},
  response::{
    IntoResponse, Response, Sse,
    sse::{Event, KeepAlive},
  },
};
use serde::Deserialize;
use serde_json::json;
use std::{convert::Infallible, sync::Arc};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ask {
  pub text: String,
  #[serde(default)]
  pub stream: bool,
  #[serde(default)]
  pub history: Vec<AskExchange>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskExchange {
  pub question: String,
  pub answer: String,
}

fn append_ask_messages(conversation: &mut Vec<Message>, input: Ask) -> Result<(), ApiError> {
  if input.text.trim().is_empty() {
    return Err(ApiError::bad_request("question is empty"));
  }
  if input.history.len() > 32
    || input
      .history
      .iter()
      .any(|turn| turn.question.trim().is_empty() || turn.answer.trim().is_empty())
    || input.history.iter().map(|turn| turn.question.len() + turn.answer.len()).sum::<usize>()
      > 128_000
  {
    return Err(ApiError::bad_request("BTW history is too long or contains an empty turn"));
  }
  let prefix = conversation
    .iter()
    .take_while(|message| matches!(message, Message::System { .. } | Message::Developer { .. }))
    .count();
  conversation.insert(prefix, Message::System {
    metadata: Default::default(),
    content: vec![ContentBlock::Text { text: "Answer the final question using the supplied session context and the subsequent temporary BTW conversation. Do not use tools or continue the agent task. BTW turns are not part of the permanent session history.".into() }],
  });
  for turn in input.history {
    conversation.push(Message::User {
      metadata: Default::default(),
      content: vec![ContentBlock::Text { text: turn.question }],
    });
    conversation.push(Message::Assistant {
      metadata: Default::default(),
      content: vec![ContentBlock::Text { text: turn.answer }],
    });
  }
  conversation.push(Message::User {
    metadata: Default::default(),
    content: vec![ContentBlock::Text { text: input.text }],
  });
  Ok(())
}
pub async fn ask(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Json(input): Json<Ask>,
) -> Result<Response, ApiError> {
  app.require_open()?;
  let slot = app.get_session(&id).await?;
  let provider = app.get_provider(&slot.get_descriptor().provider)?;
  let handle = slot.handle.clone();
  let mut request = blocking(move || Ok(handle.build_context_snapshot()?)).await?;
  request.stream = input.stream;
  request.tools.clear();
  request.tool_choice = None;
  append_ask_messages(&mut request.conversation, input)?;
  let client = crate::server::sampling::observe_client(
    &provider.client,
    app.index.clone(),
    app.tasks.clone(),
    slot.get_descriptor().provider,
    Some(id.clone()),
  )
  .with_session_id(id);
  let model = SessionModel::new(provider, slot.get_descriptor().provider, slot.image_dir.clone())
    .with_client(client);
  let result = tokio::select! {
    _ = app.stop.cancelled() => return Err(ApiError::conflict("server is shutting down")),
    result = model.call(&request) => result?,
  };
  match result {
    CallResponse::Complete(response) => Ok(Json(response).into_response()),
    CallResponse::Stream(stream) => {
      let events = futures_util::stream::unfold(
        (stream, app.stop.clone(), false),
        |(mut stream, stop, done)| async move {
          if done {
            return None;
          }
          let result = tokio::select! {
            _ = stop.cancelled() => Err("server is shutting down".to_owned()),
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
        },
      );
      Ok(Sse::new(events).keep_alive(KeepAlive::default()).into_response())
    }
  }
}

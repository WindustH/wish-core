//! One read-only question against a committed session snapshot; disconnect cancels the call.
use super::sse;
use crate::server::{
  app::App,
  error::{ApiError, blocking},
};
use crate::{
  executor::model::ModelCaller,
  protocol::{ContentBlock, Message},
};
use axum::{
  Json,
  extract::{Path, State},
  response::Response,
};
use serde::Deserialize;
use std::sync::Arc;

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

const ASK_INSTRUCTION: &str = "This is a temporary BTW question about the session so far. Answer it from the session context above and the BTW conversation that follows. Do not call tools or continue the agent task. BTW turns are not part of the permanent session history.";

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
  // The session context stays exactly as the conversation calls send it, so the question reads
  // their prompt cache; the instruction rides on the first BTW question, so later questions in the
  // same BTW conversation repeat it unchanged as well.
  let mut instruction = Some(ContentBlock::Text { text: ASK_INSTRUCTION.into() });
  let mut question = |text: String| Message::User {
    metadata: Default::default(),
    content: instruction.take().into_iter().chain([ContentBlock::Text { text }]).collect(),
  };
  for turn in input.history {
    conversation.push(question(turn.question));
    conversation.push(Message::Assistant {
      metadata: Default::default(),
      content: vec![ContentBlock::Text { text: turn.answer }],
    });
  }
  conversation.push(question(input.text));
  Ok(())
}
pub async fn ask(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Json(input): Json<Ask>,
) -> Result<Response, ApiError> {
  app.lifecycle.require_open()?;
  let slot = app.get_session(&id).await?;
  let provider = app.get_provider(&slot.get_descriptor().provider)?;
  let sender = slot.sender.clone();
  let mut request = blocking(move || Ok(sender.build_context_snapshot()?)).await?;
  request.stream = input.stream;
  append_ask_messages(&mut request.conversation, input)?;
  let model = slot.make_model(provider);
  let response = app.lifecycle.until_shutdown(model.call(&request)).await??;
  Ok(sse::model_response(response, app.lifecycle.stop.clone()))
}

//! Explicit stateless naming requests. Only clients decide when to call this endpoint.
use crate::{
  application::{
    app::App,
    error::{ApplicationError, blocking},
    session::SessionSlot,
    session_model::SessionModel,
  },
  protocol::{ContentBlock, Message, model_use::request::ToolChoice},
  session::{EntryId, SessionReader},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

const PROMPT: &str = "Generate a concise, descriptive title for this conversation. Use the user's language. Return only the title on one line, without quotes, Markdown, explanation or tool calls. Prefer 4–12 words, or 6–20 Chinese characters; at most 60 characters. Summarize the main topic rather than answering the user's request.";

#[derive(Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum NamingInput {
  Context,
  FirstMessage { entry: EntryId },
}

/// The earliest user message, including imported history and queued inputs.
pub(crate) fn first_user(reader: &SessionReader) -> Result<Option<EntryId>, ApplicationError> {
  // Queued input is already an immutable entry, but has not entered timeline history yet.
  // Stop at the first user entry so long conversations do not need a full-history scan.
  let entries = reader.get_entries();
  for position in 0..entries.len()? {
    if let Some(entry) = entries.get(position)?
      && matches!(entry.message, Message::User { .. })
    {
      return Ok(Some(entry.id));
    }
  }
  Ok(None)
}

pub async fn generate(
  app: Arc<App>,
  id: String,
  input: NamingInput,
) -> Result<Value, ApplicationError> {
  app.lifecycle.require_open()?;
  let slot = app.get_session(&id).await?;
  let owner = slot.clone();
  // No execution lock: naming can run beside the conversation's normal reply.
  let (request, descriptor) = blocking(move || prepare(&owner, input)).await?;
  let provider = app.get_provider(&descriptor.provider)?;
  // No with_history or conversation retry observer: this request must not become a chat turn.
  let client = provider.client.clone().with_session_id(id.clone());
  let model = SessionModel::new(provider, app.data_dir.blobs(&id), client);
  let response = app
    .lifecycle
    .until_shutdown(crate::executor::model::stateless::call_once(&model, &request))
    .await??;
  let text = response
    .messages
    .iter()
    .filter_map(|message| match message {
      Message::Assistant { content, .. } => Some(content),
      _ => None,
    })
    .flatten()
    .filter_map(|block| match block {
      ContentBlock::Text { text } => Some(text.as_str()),
      _ => None,
    })
    .collect::<Vec<_>>()
    .join("");
  let name = text.trim().trim_matches(|c| matches!(c, '"' | '\'' | '“' | '”')).trim();
  if name.is_empty() || name.lines().count() != 1 || name.chars().count() > 60 {
    return Err(ApplicationError::invalid("model returned an invalid session title"));
  }
  // The frontend applies this suggestion with If-Match, so intervening edits win.
  Ok(json!({"name":name,"revision":descriptor.revision}))
}

fn prepare(
  slot: &SessionSlot,
  input: NamingInput,
) -> Result<(crate::protocol::Request, crate::application::session::Descriptor), ApplicationError> {
  slot.require_live()?;
  let mut descriptor = slot.get_descriptor();
  let mut request = match input {
    NamingInput::Context => {
      let mut request = slot.sender.build_context_snapshot()?;
      request.conversation.push(Message::User {
        metadata: json!({"source":"wish_session_naming"}),
        content: vec![ContentBlock::Text { text: PROMPT.into() }],
      });
      request.tool_choice = if request.tools.is_empty() { None } else { Some(ToolChoice::None) };
      request
    }
    NamingInput::FirstMessage { entry } => {
      if first_user(&slot.reader)? != Some(entry) {
        return Err(ApplicationError::invalid("entry is not the first user message"));
      }
      let message =
        slot.reader.get_entries().get(entry.0 as u64)?.ok_or_else(ApplicationError::not_found)?;
      let config = slot.get_status().status.config;
      if let Some(pending) = &descriptor.pending_selection {
        descriptor.provider = pending.provider.clone();
      }
      let mut request = config.build_request(vec![
        Message::System {
          metadata: json!({"source":"wish_session_naming"}),
          content: vec![ContentBlock::Text { text: PROMPT.into() }],
        },
        message.message.clone(),
      ]);
      request.tools.clear();
      request.tool_choice = None;
      request
    }
  };
  request.stream = true;
  // Keep model reasoning settings/budget compatible with the selected provider.
  Ok((request, descriptor))
}

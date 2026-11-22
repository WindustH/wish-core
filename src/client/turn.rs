//! Per-call upstream state. Stream attempts publish an id here; it becomes usable only
//! when the executor commits the matching local response.
use crate::protocol::{
  Request,
  error::Error,
  model_use::{ModelUseProtocol, incremental::IncrementalProtocol, request},
};
use serde_json::Value;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub(crate) struct TurnAttempt {
  pub protocol: IncrementalProtocol,
  pub previous: Option<(String, usize)>,
  pub conversation: Arc<tokio::sync::OnceCell<String>>,
  pub returned: Arc<Mutex<Option<String>>>,
  pub socket: Option<crate::transport::ResponsesSocket>,
}
impl TurnAttempt {
  pub fn capture(&self, value: &Value, stream: bool) {
    // Some compatible Responses endpoints accept the request but explicitly
    // report that they ignored store=true. Never use such a response as a parent.
    let response = if stream { &value["response"] } else { value };
    if matches!(self.protocol, IncrementalProtocol::Responses | IncrementalProtocol::ArkResponses)
      && response["store"] == false
    {
      self.returned.lock().unwrap().take();
    } else if let Some(id) = self.protocol.response_id(value, stream) {
      *self.returned.lock().unwrap() = Some(id.to_owned());
    }
  }
  pub fn render(
    &self,
    model: ModelUseProtocol,
    projected: &Request,
    body: &mut Value,
    path: &mut String,
  ) -> Result<(), Error> {
    if let Some((_, cut)) = &self.previous {
      let suffix = &projected.conversation[*cut..];
      let items = match model {
        ModelUseProtocol::OpenAiResponses(mode) => {
          request::openai_responses::render_items(suffix, mode)?
        }
        ModelUseProtocol::GoogleInteractions => request::google_interactions::render_steps(suffix)?,
        ModelUseProtocol::MistralConversations => {
          request::mistral_conversations::render_inputs(suffix)?
        }
        ModelUseProtocol::AnthropicMessages(_) => {
          request::anthropic_messages::render_delta(projected, *cut)?
        }
        _ => return Err(Error::Build("unsupported incremental wire".into())),
      };
      body[self.protocol.items()] = Value::Array(items);
    }
    self.protocol.apply(body, path, self.previous.as_ref().map(|(id, _)| id.as_str()));
    if self.socket.is_some() {
      let envelope =
        body.as_object_mut().ok_or_else(|| Error::Build("expected a Responses object".into()))?;
      envelope.remove("stream");
      envelope.insert("type".into(), "response.create".into());
    }
    Ok(())
  }
}

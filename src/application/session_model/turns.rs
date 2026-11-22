//! One volatile checkpoint per open session. Match the effective projected inputs,
//! including image admission, before materializing bytes for the new wire suffix.
use super::*;
use crate::client::turn::TurnAttempt;
use crate::protocol::{
  endpoint::Credentials, model_use::incremental::IncrementalProtocol as Protocol,
};
use sha2::{Digest, Sha256};
use std::sync::Weak;

#[derive(Clone, Default)]
pub(in crate::application) struct TurnHistory(Arc<Mutex<Option<Checkpoint>>>);
struct Checkpoint {
  pending: Pending,
  id: String,
}
pub(super) struct Pending {
  provider: Weak<Provider>,
  credentials: Credentials,
  generation: usize,
  controls: [u8; 32],
  messages: Vec<[u8; 32]>,
  model: String,
  attempt: TurnAttempt,
}
fn digest(value: &impl serde::Serialize) -> [u8; 32] {
  let mut hash = Sha256::new();
  serde_json::to_writer(&mut hash, value).expect("protocol values serialize");
  hash.finalize().into()
}
impl Pending {
  pub(super) fn commit(
    mut self,
    messages: &[Message],
    history: TurnHistory,
  ) -> Option<Box<dyn FnOnce() + Send>> {
    let id = self.attempt.returned.lock().unwrap().take()?;
    for message in messages {
      if let Message::Reasoning { .. } = message {
        let mut message = message.clone();
        if let Message::Reasoning { source_model, .. } = &mut message {
          *source_model = Some(self.model.clone());
        }
        self.messages.push(digest(&message));
      } else {
        self.messages.push(digest(message));
      }
    }
    self.attempt.previous = None;
    Some(Box::new(move || {
      *history.0.lock().unwrap() = Some(Checkpoint { pending: self, id });
    }))
  }
}
impl SessionModel {
  pub(super) async fn continue_turn(
    &self,
    original: &Request,
    generation: usize,
  ) -> Result<CallResponse<<Self as ModelCaller>::Stream>, Error> {
    self.pending.lock().unwrap().take();
    let mut client = self.client();
    let config = &self.provider.config;
    let protocol = config.incremental.or_else(|| {
      (config.protocol == "codex_responses"
        && config.base_url.trim_end_matches('/') == "https://chatgpt.com/backend-api/codex")
        .then_some(Protocol::ResponsesWebsocket)
    });
    let Some(protocol) = protocol.filter(|p| {
      *p != Protocol::Disabled && (*p != Protocol::ResponsesWebsocket || original.stream)
    }) else {
      client.turn = None;
      return self.call_full(original, &client).await;
    };
    let (mut logical, _, deliveries) =
      self.prepare_projection(original, ImageProjection::Inspect).await?;
    // A local omission notice is appended after the input for this turn only.
    if deliveries.iter().any(|d| d.reason.is_some()) {
      self.history.0.lock().unwrap().take();
      client.turn = None;
      return self.call_full(original, &client).await;
    }
    let messages = std::mem::take(&mut logical.conversation);
    let controls = digest(&logical);
    let hashes: Vec<_> = messages.iter().map(digest).collect();
    let previous = self.history.0.lock().unwrap().take().filter(|c| {
      let p = &c.pending;
      p.provider.ptr_eq(&Arc::downgrade(&self.provider))
        && &p.credentials == client.get_credentials()
        && p.generation == generation
        && p.controls == controls
        && p.attempt.protocol == protocol
        && p.messages.len() < hashes.len()
        && hashes.starts_with(&p.messages)
        && messages[p.messages.len()..].iter().all(|m| {
          matches!(
            m,
            Message::User { .. }
              | Message::Developer { .. }
              | Message::System { .. }
              | Message::ToolResult { .. }
          )
        })
    });
    drop(messages);
    let mut attempt = TurnAttempt {
      protocol,
      previous: previous.as_ref().map(|c| (c.id.clone(), c.pending.messages.len())),
      conversation: Default::default(),
      returned: Default::default(),
      socket: (protocol == Protocol::ResponsesWebsocket).then(|| {
        previous
          .as_ref()
          .and_then(|c| c.pending.attempt.socket.clone())
          .unwrap_or_else(|| client.responses_socket())
      }),
    };
    drop(previous);
    loop {
      let projection = attempt
        .previous
        .as_ref()
        .map_or(ImageProjection::Send, |(_, n)| ImageProjection::SendSuffix(*n));
      let (request, images, deliveries) = self.prepare_projection(original, projection).await?;
      if let Some(observer) = &self.instruction_observer {
        observer.record_images(deliveries).await?;
      }
      self.record_instructions(original, &request).await?;
      client.turn = Some(attempt.clone());
      match client.call(&request).await {
        Ok(response) => {
          *self.pending.lock().unwrap() = Some(Pending {
            provider: Arc::downgrade(&self.provider),
            credentials: client.get_credentials().clone(),
            generation,
            controls,
            messages: hashes,
            model: request.model,
            attempt,
          });
          return Ok(response);
        }
        Err(Error::Upstream { status: Some(400 | 422), ref code, .. })
          if protocol == Protocol::AnthropicThreads
            && code.as_deref() == Some("thread_unsupported_request") =>
        {
          client.turn = None;
          return self.call_full(original, &client).await;
        }
        Err(err) if attempt.previous.is_some() && protocol.stale(&err) => {
          attempt.previous = None;
          attempt.conversation = Default::default();
          attempt.returned = Default::default();
          if attempt.socket.is_some() {
            attempt.socket = Some(client.responses_socket());
          }
        }
        // A proxy can serve an ordinary HTTP page or forbid upgrades while
        // accepting POST. No model generation has started at the handshake.
        Err(Error::Upstream {
          status: None | Some(200..=400 | 403 | 404 | 405 | 426 | 429 | 500..=599),
          ref code,
          ..
        }) if code.as_deref() == Some("websocket_handshake") => {
          client.turn = None;
          return self.call_full(original, &client).await;
        }
        Err(error) if images && crate::protocol::http_error::rejects_image_input(&error) => {
          self.rejected.store(true, Ordering::Relaxed);
          client.turn = None;
          return self.call_full(original, &client).await;
        }
        Err(error) => return Err(self.with_model_context(error, &request.model)),
      }
    }
  }
}

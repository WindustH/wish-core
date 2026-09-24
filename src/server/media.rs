//! Request-time image projection. Stored conversation always retains the original images.
use crate::server::provider::{ModelClient, Provider};
use crate::{
  Error,
  executor::model::{CallResponse, ModelCaller},
  protocol::{
    ContentBlock, Message, Request, TokenCount, UpstreamCompaction, UpstreamCompactionRequest,
    model_use::ModelUseProtocol,
  },
};
use base64::Engine;
use sha2::{Digest, Sha256};
use std::{
  path::PathBuf,
  sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
  },
};

pub const AGENT_INSTRUCTIONS: &str =
  "You are the agent operating this session. Use the provided tools to complete the user's task.";
pub fn apply_agent_instructions(messages: &mut Vec<Message>) {
  let mut found = false;
  for message in messages.iter_mut() {
    if let Message::System { metadata, content } = message {
      if metadata["source"] == "wish_agent_instructions" {
        *content = vec![ContentBlock::Text { text: AGENT_INSTRUCTIONS.into() }];
        found = true;
      }
    }
  }
  if !found {
    messages.insert(
      0,
      Message::System {
        metadata: serde_json::json!({"source":"wish_agent_instructions"}),
        content: vec![ContentBlock::Text { text: AGENT_INSTRUCTIONS.into() }],
      },
    );
  }
}
pub struct SessionModel {
  provider: Arc<Provider>,
  provider_id: String,
  client: ModelClient,
  directory: PathBuf,
  rejected: AtomicBool,
}
impl SessionModel {
  pub fn new(provider: Arc<Provider>, provider_id: String, directory: PathBuf) -> Self {
    Self {
      client: provider.client.clone(),
      provider,
      provider_id,
      directory,
      rejected: AtomicBool::new(false),
    }
  }
  pub fn with_client(mut self, client: ModelClient) -> Self {
    self.client = client;
    self
  }
  fn project(&self, model: &str, messages: &mut [Message], save: bool) -> Result<bool, Error> {
    let unsupported = self
      .provider
      .config
      .models
      .get(model)
      .and_then(|value| value["input_modalities"].as_array())
      .is_some_and(|values| !values.iter().any(|value| value == "image"));
    let rejected = self.rejected.load(Ordering::Relaxed);
    let mut images = false;
    for message in messages {
      let content = match message {
        Message::User { content, .. }
        | Message::Assistant { content, .. }
        | Message::System { content, .. }
        | Message::Developer { content, .. } => content,
        _ => continue,
      };
      let mut projected = Vec::new();
      for block in std::mem::take(content) {
        if let ContentBlock::Image { ref data_base64, .. } = block {
          images = true;
          let bytes = base64::engine::general_purpose::STANDARD
            .decode(data_base64)
            .map_err(|error| Error::Build(format!("invalid stored image: {error}")))?;
          let image_id = format!("{:x}", Sha256::digest(&bytes));
          let path = self.directory.join(&image_id);
          if save {
            save_image(&self.directory, data_base64)?;
          }
          let notice = if unsupported {
            "The selected model's declared capabilities do not support image input. Only this file reference is sent; do not claim to have viewed the image. Use shell for file processing."
          } else if rejected {
            "The upstream rejected image input. Only this file reference is sent; do not claim to have viewed the image. Use shell for file processing."
          } else {
            "The image is included below. Do not call view_image again merely to confirm an image you have already viewed; use shell when further processing is needed."
          };
          projected.push(ContentBlock::Text {
            text: format!(
              "[Image sha256:{image_id}]\n{notice}\nSession-local blob path: {}",
              serde_json::json!(path)
            ),
          });
          if !unsupported && !rejected {
            projected.push(block);
          }
        } else {
          projected.push(block);
        }
      }
      *content = projected;
    }
    Ok(images && !unsupported && !rejected)
  }
  async fn prepare(&self, request: &Request) -> Result<(Request, bool), Error> {
    let mut request = request.clone();
    apply_agent_instructions(&mut request.conversation);
    // File writes run outside the async runtime workers; validation below performs no I/O.
    let (mut messages, model, provider, provider_id, directory, rejected) = (
      request.conversation,
      request.model.clone(),
      self.provider.clone(),
      self.provider_id.clone(),
      self.directory.clone(),
      self.rejected.load(Ordering::Relaxed),
    );
    let (conversation, images) = tokio::task::spawn_blocking(move || {
      let projection = Self {
        client: provider.client.clone(),
        provider,
        provider_id,
        directory,
        rejected: AtomicBool::new(rejected),
      };
      projection.project(&model, &mut messages, true).map(|images| (messages, images))
    })
    .await
    .map_err(|e| Error::Build(e.to_string()))??;
    request.conversation = conversation;
    Ok((request, images))
  }
  fn with_model_context(&self, error: Error, model: &str) -> Error {
    match error {
      Error::Upstream { status, code, message, retry_after_ms } => Error::Upstream {
        status,
        code,
        message: format!("provider `{}` model `{model}`: {message}", self.provider_id),
        retry_after_ms,
      },
      other => other,
    }
  }
}
pub fn save_image(directory: &std::path::Path, data: &str) -> Result<PathBuf, Error> {
  let bytes = base64::engine::general_purpose::STANDARD
    .decode(data)
    .map_err(|e| Error::Build(format!("invalid image: {e}")))?;
  let path = directory.join(format!("{:x}", Sha256::digest(&bytes)));
  let temporary = directory.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
  std::fs::create_dir_all(directory)
    .and_then(|_| std::fs::write(&temporary, &bytes))
    .and_then(|_| std::fs::rename(&temporary, &path))
    .map_err(|error| {
      let _ = std::fs::remove_file(&temporary);
      Error::Build(format!("save session image: {error}"))
    })?;
  Ok(path)
}
fn rejects_images(error: &Error) -> bool {
  let Error::Upstream { status, code, message, .. } = error else {
    return false;
  };
  if !matches!(status, None | Some(400) | Some(422)) {
    return false;
  }
  let message = message.to_ascii_lowercase();
  matches!(
    code.as_deref(),
    Some("image_input_not_supported" | "image_not_supported" | "vision_not_supported")
  ) || [
    "image input is not supported",
    "image inputs are not supported",
    "does not support image input",
    "does not support images",
    "image_url is only supported by certain models",
  ]
  .iter()
  .any(|phrase| message.contains(phrase))
    || (message.contains("messages.content.type")
      && (message.contains("['text']") || message.contains("[\"text\"]")))
}
impl ModelCaller for SessionModel {
  type Stream = <ModelClient as ModelCaller>::Stream;
  fn get_model_use_protocol(&self) -> Option<ModelUseProtocol> {
    Some(self.client.get_model_use_protocol())
  }
  fn supports_upstream_compaction(&self) -> bool {
    self.client.supports_upstream_compaction()
  }
  fn validate_request(&self, request: &Request) -> Result<(), Error> {
    let mut request = request.clone();
    apply_agent_instructions(&mut request.conversation);
    self.project(&request.model, &mut request.conversation, false)?;
    self.client.validate_request(&request)
  }
  async fn count_tokens(&self, request: &Request) -> Result<Option<TokenCount>, Error> {
    let (request, _) = self.prepare(request).await?;
    ModelCaller::count_tokens(&self.client, &request)
      .await
      .map_err(|error| self.with_model_context(error, &request.model))
  }
  async fn compact_upstream(
    &self,
    request: &UpstreamCompactionRequest,
  ) -> Result<UpstreamCompaction, Error> {
    let mut projected = Request {
      model: request.model.clone(),
      conversation: vec![],
      stream: false,
      tools: vec![],
      tool_choice: None,
      max_output_tokens: None,
      reasoning: None,
      cache: None,
    };
    projected.conversation = request.conversation.clone();
    let (projected, _) = self.prepare(&projected).await?;
    self
      .client
      .compact_upstream(&UpstreamCompactionRequest {
        model: request.model.clone(),
        conversation: projected.conversation,
      })
      .await
      .map_err(|error| self.with_model_context(error, &request.model))
  }
  async fn call(&self, original: &Request) -> Result<CallResponse<Self::Stream>, Error> {
    let (request, images) = self.prepare(original).await?;
    let result = match self.client.call(&request).await {
      // Client has not handed out any event; partial stream failures never enter this branch.
      Err(error) if images && rejects_images(&error) => {
        self.rejected.store(true, Ordering::Relaxed);
        let (request, _) = self.prepare(original).await?;
        self.client.call(&request).await
      }
      result => result,
    };
    result.map_err(|error| self.with_model_context(error, &request.model))
  }
}

//! The model a session calls: its provider's client, seen through the session. Each request gets
//! the agent instructions first, encrypted compaction items the provider cannot read replaced by
//! their handoff, and its images projected for what the model accepts - the stored conversation
//! keeps the originals. An upstream that refuses images turns them into file references for the
//! rest of the model's life.
use crate::server::{
  blobs, compaction_item,
  provider::{ModelClient, Provider},
};
use crate::{
  Error,
  executor::model::{CallResponse, ModelCaller},
  protocol::{
    ContentBlock, Message, Request, TokenCount, Tool, UpstreamCompaction,
    UpstreamCompactionRequest, model_use::ModelUseProtocol,
  },
};
use base64::Engine;
use serde_json::Value;
use std::{
  collections::BTreeMap,
  path::{Path, PathBuf},
  sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
  },
};

const AGENT_INSTRUCTIONS: &str =
  "You are the agent operating this session. Use the provided tools to complete the user's task.";
/// The agent instructions a request with `tools` carries: the fixed text, then how to reach MCP
/// servers when there is a shell to reach them from. Neither depends on the servers or on the
/// session's MCP switch, so only switching the shell - which changes the tools anyway - changes it.
pub fn agent_instructions(tools: &[Tool]) -> String {
  let mut text = AGENT_INSTRUCTIONS.to_owned();
  if tools.iter().any(|tool| tool.name == "shell_start") {
    text.push_str("\n\n");
    text.push_str(crate::server::mcp::INSTRUCTIONS);
  }
  text
}
/// Puts the current agent instructions in the conversation: in place of the ones it carries, or
/// first when it carries none.
pub fn apply_agent_instructions(messages: &mut Vec<Message>, tools: &[Tool]) {
  let text = agent_instructions(tools);
  let mut found = false;
  for message in messages.iter_mut() {
    if let Message::System { metadata, content } = message
      && metadata["source"] == "wish_agent_instructions"
    {
      *content = vec![ContentBlock::Text { text: text.clone() }];
      found = true;
    }
  }
  if !found {
    messages.insert(
      0,
      Message::System {
        metadata: serde_json::json!({"source":"wish_agent_instructions"}),
        content: vec![ContentBlock::Text { text }],
      },
    );
  }
}
pub struct SessionModel {
  provider: Arc<Provider>,
  client: ModelClient,
  /// The session's blobs, where its images are saved for the model to be pointed at.
  image_dir: PathBuf,
  /// Set once the upstream refused an image.
  rejected: AtomicBool,
}
impl SessionModel {
  /// A model calling through `client`, the provider's own dressed for the session.
  pub fn new(provider: Arc<Provider>, image_dir: PathBuf, client: ModelClient) -> Self {
    Self { provider, client, image_dir, rejected: AtomicBool::new(false) }
  }
  async fn prepare(&self, request: &Request) -> Result<(Request, bool), Error> {
    let mut request = request.clone();
    apply_agent_instructions(&mut request.conversation, &request.tools);
    compaction_item::project(&mut request.conversation, &self.provider.id);
    // File writes run outside the async runtime workers; validation below performs no I/O.
    let (mut messages, model, provider, image_dir, rejected) = (
      request.conversation,
      request.model.clone(),
      self.provider.clone(),
      self.image_dir.clone(),
      self.rejected.load(Ordering::Relaxed),
    );
    let (conversation, images) = tokio::task::spawn_blocking(move || {
      let models = &provider.config.models;
      project_images(models, &model, &image_dir, rejected, &mut messages, true)
        .map(|images| (messages, images))
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
        message: format!("provider `{}` model `{model}`: {message}", self.provider.id),
        retry_after_ms,
      },
      other => other,
    }
  }
}
/// Replaces each image with a reference to its blob, followed by the image itself unless the
/// model's declared capabilities or the upstream refuse images. With `save`, the images are stored
/// as blobs. Returns whether an image is still sent.
fn project_images(
  models: &BTreeMap<String, Value>,
  model: &str,
  image_dir: &Path,
  rejected: bool,
  messages: &mut [Message],
  save: bool,
) -> Result<bool, Error> {
  let unsupported = models
    .get(model)
    .and_then(|value| value["input_modalities"].as_array())
    .is_some_and(|values| !values.iter().any(|value| value == "image"));
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
        let image_id = blobs::blob_id(&bytes);
        let path = image_dir.join(&image_id);
        if save {
          blobs::save_image(image_dir, data_base64)?;
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
    apply_agent_instructions(&mut request.conversation, &request.tools);
    compaction_item::project(&mut request.conversation, &self.provider.id);
    let rejected = self.rejected.load(Ordering::Relaxed);
    let models = &self.provider.config.models;
    project_images(
      models,
      &request.model,
      &self.image_dir,
      rejected,
      &mut request.conversation,
      false,
    )?;
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
    // The request's own tools, so its instructions read as in the conversation's requests.
    let mut projected = Request {
      model: request.model.clone(),
      conversation: vec![],
      stream: false,
      tools: request.tools.clone(),
      tool_choice: None,
      max_output_tokens: None,
      reasoning: None,
      cache: None,
    };
    projected.conversation = request.conversation.clone();
    let (projected, _) = self.prepare(&projected).await?;
    let mut compaction = self
      .client
      .compact_upstream(&UpstreamCompactionRequest {
        model: request.model.clone(),
        conversation: projected.conversation,
        tools: request.tools.clone(),
        tool_choice: request.tool_choice,
        reasoning: request.reasoning.clone(),
        cache: request.cache.clone(),
      })
      .await
      .map_err(|error| self.with_model_context(error, &request.model))?;
    for message in &mut compaction.conversation {
      if matches!(message, Message::UpstreamCompaction { .. }) {
        compaction_item::set_provider(message, &self.provider.id);
      }
    }
    Ok(compaction)
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

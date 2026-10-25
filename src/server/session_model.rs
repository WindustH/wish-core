//! The model a session calls: its provider's client, seen through the session. Each request gets
//! encrypted compaction items the provider cannot read replaced by
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
  collections::{BTreeMap, HashSet},
  path::{Path, PathBuf},
  sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
  },
};

const AGENT_INSTRUCTIONS: &str =
  "You are the agent operating this session. Use the provided tools to complete the user's task.";
/// The agent instructions persisted for a generation with `tools`: the fixed text, then how to reach MCP
/// servers, skills and other sessions when there is a shell to reach them from. None depends on
/// what there is or on the session's switches, so only switching the shell - which changes the
/// tools anyway - changes it.
pub fn agent_instructions(tools: &[Tool]) -> String {
  agent_instructions_for_shell(tools.iter().any(|tool| tool.name == "shell_start"))
}
pub fn agent_instructions_for_shell(shell: bool) -> String {
  let mut text = AGENT_INSTRUCTIONS.to_owned();
  if shell {
    for section in [
      crate::server::mcp::INSTRUCTIONS,
      crate::server::skills::INSTRUCTIONS,
      crate::server::peers::INSTRUCTIONS,
    ] {
      text.push_str("\n\n");
      text.push_str(section);
    }
  }
  text
}
/// Puts the current agent instructions in the conversation: in place of the ones it carries, or
/// first when it carries none.
/// Creation knows the switches before tool implementations are installed.
pub fn apply_agent_instructions_for_shell(messages: &mut Vec<Message>, shell: bool) {
  replace_agent_instructions(messages, agent_instructions_for_shell(shell));
}
fn replace_agent_instructions(messages: &mut Vec<Message>, text: String) {
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
pub fn persist_agent_instructions(
  session: &mut crate::session::Session,
  config: crate::session::SessionConfig,
) -> Result<(), crate::session::SessionError> {
  session.set_system_instruction_config(
    "wish_agent_instructions".into(),
    agent_instructions(&config.tools),
    config,
  )
}
pub struct SessionModel {
  provider: Arc<Provider>,
  client: ModelClient,
  /// The session's blobs, where its images are saved for the model to be pointed at.
  image_dir: PathBuf,
  /// Set once the upstream refused an image.
  rejected: AtomicBool,
  history: Option<(crate::session::SessionSender, tokio::sync::broadcast::Sender<Value>)>,
  recorded: Mutex<HashSet<String>>,
}
impl SessionModel {
  /// A model calling through `client`, the provider's own dressed for the session.
  pub fn new(provider: Arc<Provider>, image_dir: PathBuf, client: ModelClient) -> Self {
    Self {
      provider,
      client,
      image_dir,
      rejected: AtomicBool::new(false),
      history: None,
      recorded: Mutex::new(HashSet::new()),
    }
  }
  pub fn with_history(
    mut self,
    sender: crate::session::SessionSender,
    events: tokio::sync::broadcast::Sender<Value>,
  ) -> Self {
    self.history = Some((sender, events));
    self
  }
  async fn record_instructions(&self, original: &Request, request: &Request) -> Result<(), Error> {
    let Some((sender, events)) = &self.history else { return Ok(()) };
    let mut notes = Vec::new();
    for (index, message) in request.conversation.iter().enumerate() {
      match message {
        Message::System { content, .. }
          if original.conversation.get(index).is_none_or(|before| {
            serde_json::to_value(before).ok() != serde_json::to_value(message).ok()
          }) =>
        {
          notes.push(serde_json::json!({"title":"系统指令（请求时补入）","content":content}));
        }
        Message::User { metadata, content }
          if metadata["source"]
            .as_str()
            .is_some_and(|source| source.starts_with("wish_request_")) =>
        {
          notes.push(serde_json::json!({"title":metadata["instruction_title"],"content":content,"position":index}));
        }
        _ => {}
      }
      if let Message::User { content, .. } | Message::Assistant { content, .. } = message {
        for block in content {
          if let ContentBlock::Text { text } = block
            && text.starts_with("[Image sha256:")
          {
            notes.push(serde_json::json!({"title":"图片输入说明","content":[block]}));
          }
        }
      }
    }
    if !request.tools.is_empty() {
      notes.push(serde_json::json!({"title":"可用工具定义","content":[{"Text":{"text":serde_json::to_string_pretty(&request.tools).unwrap_or_default()}}]}));
    }
    let notes: Vec<_> = {
      let mut recorded = self.recorded.lock().unwrap();
      notes.into_iter().filter(|note| recorded.insert(note.to_string())).collect()
    };
    if notes.is_empty() {
      return Ok(());
    }
    let sender = sender.clone();
    tokio::task::spawn_blocking(move || {
      for note in notes {
        sender.record_application_event(
          serde_json::json!({"type":"model_instruction","instruction":note}),
        )?;
      }
      Ok::<_, crate::session::SessionError>(())
    })
    .await
    .map_err(|error| Error::Build(error.to_string()))?
    .map_err(|error| Error::Build(error.to_string()))?;
    let _ = events.send(serde_json::json!({"type":"history_changed"}));
    Ok(())
  }
  async fn prepare(&self, request: &Request) -> Result<(Request, bool), Error> {
    self.prepare_projection(request, true).await
  }
  async fn prepare_projection(
    &self,
    request: &Request,
    save: bool,
  ) -> Result<(Request, bool), Error> {
    let mut request = request.clone();
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
      project_images(models, &model, &image_dir, rejected, &mut messages, save)
        .map(|images| (messages, images))
    })
    .await
    .map_err(|e| Error::Build(e.to_string()))??;
    request.conversation = conversation;
    Ok((request, images))
  }
  /// Read-only projection for inspection. This is current context, not a past wire request.
  pub async fn inspect(&self, request: &Request) -> Result<Value, Error> {
    let (projected, _) = self.prepare_projection(request, false).await?;
    let mut value =
      serde_json::to_value(projected).map_err(|error| Error::Build(error.to_string()))?;
    image_references(&mut value);
    Ok(value)
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
    self.record_instructions(original, &request).await?;
    let result = match self.client.call(&request).await {
      // Client has not handed out any event; partial stream failures never enter this branch.
      Err(error) if images && rejects_images(&error) => {
        self.rejected.store(true, Ordering::Relaxed);
        let (request, _) = self.prepare(original).await?;
        self.record_instructions(original, &request).await?;
        self.client.call(&request).await
      }
      result => result,
    };
    result.map_err(|error| self.with_model_context(error, &request.model))
  }
}

/// Images remain inspectable by blob reference without duplicating base64 in the JSON response.
pub(crate) fn image_references(value: &mut Value) {
  let Some(messages) = value["conversation"].as_array_mut() else { return };
  for message in messages {
    for role in ["System", "Developer", "User", "Assistant"] {
      let Some(blocks) = message
        .get_mut(role)
        .and_then(|message| message.get_mut("content"))
        .and_then(Value::as_array_mut)
      else {
        continue;
      };
      for block in blocks {
        let Some(image) = block.get_mut("Image").and_then(Value::as_object_mut) else { continue };
        let Some(Value::String(encoded)) = image.remove("data_base64") else { continue };
        if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(&encoded) {
          image.insert("image_blob_id".into(), Value::String(blobs::blob_id(&bytes)));
          image.insert("byte_count".into(), serde_json::json!(bytes.len()));
        }
        image.insert(
          "data_base64".into(),
          Value::String("[image bytes represented by blob reference]".into()),
        );
      }
    }
  }
}

#[cfg(test)]
mod inspection_tests {
  use super::*;
  #[test]
  fn image_inspection_preserves_tool_arguments() {
    let argument = serde_json::json!({"Image":{"data_base64":"YWJj"}});
    let mut request = serde_json::json!({"conversation":[
      {"User":{"content":[{"Image":{"mime_type":"image/png","data_base64":"YWJj"}}]}},
      {"ToolUse":{"arguments":argument.clone()}}
    ]});
    image_references(&mut request);
    assert_eq!(request["conversation"][0]["User"]["content"][0]["Image"]["byte_count"], 3);
    assert_eq!(request["conversation"][1]["ToolUse"]["arguments"], argument);
  }
}

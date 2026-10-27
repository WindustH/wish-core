//! The model a session calls: its provider's client, seen through the session. Each request gets
//! encrypted compaction items the provider cannot read replaced by
//! their handoff, and its images projected for what the model accepts - the stored conversation
//! keeps the originals. An upstream that refuses images turns them into file references for the
//! rest of the model's life.
use super::model_images::project_images;
use crate::application::{
  compaction_item,
  provider::{ModelClient, Provider},
};
use crate::storage::blobs;
use crate::{
  executor::model::{CallResponse, ModelCaller},
  protocol::error::Error,
  protocol::{
    Message, Request, TokenCount, UpstreamCompaction, UpstreamCompactionRequest,
    model_use::ModelUseProtocol,
  },
};
use base64::Engine;
use serde_json::Value;
use std::{
  path::PathBuf,
  sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
  },
};

pub struct SessionModel {
  provider: Arc<Provider>,
  client: ModelClient,
  /// The session's blobs, where its images are saved for the model to be pointed at.
  image_dir: PathBuf,
  /// Set once the upstream refused an image.
  rejected: AtomicBool,
  instruction_observer: Option<super::model_observation::InstructionObserver>,
}
impl SessionModel {
  /// A model calling through `client`, the provider's own dressed for the session.
  pub fn new(provider: Arc<Provider>, image_dir: PathBuf, client: ModelClient) -> Self {
    Self {
      provider,
      client,
      image_dir,
      rejected: AtomicBool::new(false),
      instruction_observer: None,
    }
  }
  pub fn with_history(
    mut self,
    sender: crate::session::SessionSender,
    events: tokio::sync::broadcast::Sender<Value>,
  ) -> Self {
    self.instruction_observer =
      Some(super::model_observation::InstructionObserver::new(sender, events));
    self
  }
  async fn record_instructions(&self, original: &Request, request: &Request) -> Result<(), Error> {
    if let Some(observer) = &self.instruction_observer {
      observer.record(original, request).await?;
    }
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
    let (conversation, images) = crate::utils::blocking::io(move || {
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
      Err(error) if images && crate::protocol::http_error::rejects_image_input(&error) => {
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

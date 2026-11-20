//! The model a session calls: its provider's client, seen through the session. Each request gets
//! encrypted compaction items the provider cannot read replaced by
//! their handoff, and its images projected for what the model accepts - the stored conversation
//! keeps the originals. An upstream that refuses images turns them into file references for the
//! rest of the model's life.
use super::model_images::{ImageDelivery, ImageProjection, project_images};
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
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
  },
};

pub struct SessionModel {
  provider: Arc<Provider>,
  client: Mutex<ModelClient>,
  /// The session's blobs, where its images are saved for the model to be pointed at.
  blob_dir: PathBuf,
  /// Set once the upstream refused an image.
  rejected: AtomicBool,
  instruction_observer: Option<super::model_observation::InstructionObserver>,
}
impl SessionModel {
  /// A model calling through `client`, the provider's own dressed for the session.
  pub fn new(provider: Arc<Provider>, blob_dir: PathBuf, client: ModelClient) -> Self {
    Self {
      provider,
      client: Mutex::new(client),
      blob_dir,
      rejected: AtomicBool::new(false),
      instruction_observer: None,
    }
  }
  /// Snapshot credentials once at a logical call boundary. Physical retries and native
  /// stream recovery retain that snapshot, including the original account identity.
  fn client(&self) -> ModelClient {
    let credentials = self.provider.credentials.read().unwrap().clone();
    let mut client = self.client.lock().unwrap();
    if client.get_credentials() != &credentials {
      *client = client.clone().with_credentials(credentials);
    }
    client.clone()
  }
  pub(super) fn with_history(
    mut self,
    sender: crate::session::SessionSender,
    reader: crate::session::SessionReader,
    events: tokio::sync::broadcast::Sender<Value>,
    state: Arc<Mutex<super::model_observation::InstructionState>>,
  ) -> Self {
    self.instruction_observer =
      Some(super::model_observation::InstructionObserver::new(sender, reader, events, state));
    self
  }
  async fn record_instructions(&self, original: &Request, request: &Request) -> Result<(), Error> {
    if let Some(observer) = &self.instruction_observer {
      observer.record(original, request).await?;
    }
    Ok(())
  }
  async fn prepare(&self, request: &Request) -> Result<(Request, bool), Error> {
    let (mut request, images, deliveries) =
      self.prepare_projection(request, ImageProjection::Send).await?;
    // Sending is a durable lifecycle action. The pure projection below never invents
    // prompt text; only committed admission events may supply an omission explanation.
    if let Some(observer) = &self.instruction_observer {
      request.conversation.extend(observer.record_images(deliveries).await?);
    }
    Ok((request, images))
  }
  async fn prepare_projection(
    &self,
    request: &Request,
    projection: ImageProjection,
  ) -> Result<(Request, bool, Vec<ImageDelivery>), Error> {
    let mut request = request.clone();
    compaction_item::project(&mut request.conversation, &self.provider.id);
    let (images, deliveries) = project_images(
      &self.provider.config.models,
      &request.model,
      &self.blob_dir,
      self.rejected.load(Ordering::Relaxed),
      &mut request.conversation,
      projection,
    )
    .await?;
    Ok((request, images, deliveries))
  }
  /// Read-only projection for inspection. This is current context, not a past wire request.
  pub async fn inspect(&self, request: &Request) -> Result<Value, Error> {
    let (projected, _, _) = self.prepare_projection(request, ImageProjection::Inspect).await?;
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
    Some(self.client().get_model_use_protocol())
  }
  fn supports_upstream_compaction(&self) -> bool {
    self.client().supports_upstream_compaction()
  }
  async fn validate_request(&self, request: &Request) -> Result<(), Error> {
    let needs_projection = request.conversation.iter().any(|message| match message {
      Message::UpstreamCompaction { .. } => true,
      Message::User { content, .. }
      | Message::Assistant { content, .. }
      | Message::System { content, .. }
      | Message::Developer { content, .. } => content.iter().any(|block| {
        matches!(
          block,
          crate::protocol::ContentBlock::Image { .. }
            | crate::protocol::ContentBlock::ImageRef { .. }
        )
      }),
      _ => false,
    });
    if !needs_projection {
      return self.client().validate_request(request).await;
    }
    let (mut request, _, _) = self.prepare_projection(request, ImageProjection::Inspect).await?;
    // Compaction validates message structure, tool pairing and protocol capabilities.
    // Blob integrity is checked when sending; no file reads/Base64 are needed to probe a cut.
    for message in &mut request.conversation {
      let content = match message {
        Message::User { content, .. }
        | Message::Assistant { content, .. }
        | Message::System { content, .. }
        | Message::Developer { content, .. } => content,
        _ => continue,
      };
      for block in content {
        if let crate::protocol::ContentBlock::ImageRef { mime_type, .. } = block {
          *block = crate::protocol::ContentBlock::Image {
            mime_type: mime_type.clone(),
            data_base64: String::new(),
          };
        }
      }
    }
    self.client().validate_request(&request).await
  }
  async fn count_tokens(&self, request: &Request) -> Result<Option<TokenCount>, Error> {
    let client = self.client();
    if client.get_token_count_protocol().is_none() {
      return Ok(None);
    }
    // A real provider count still receives actual image bytes; placeholders would change its meaning.
    let (request, _, _) = self.prepare_projection(request, ImageProjection::CountTokens).await?;
    ModelCaller::count_tokens(&client, &request)
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
      .client()
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
    let client = self.client();
    let (request, images) = self.prepare(original).await?;
    self.record_instructions(original, &request).await?;
    let result = match client.call(&request).await {
      // Client has not handed out any event; partial stream failures never enter this branch.
      Err(error) if images && crate::protocol::http_error::rejects_image_input(&error) => {
        self.rejected.store(true, Ordering::Relaxed);
        let (request, _) = self.prepare(original).await?;
        self.record_instructions(original, &request).await?;
        client.call(&request).await
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
        if let Some(image) = block.get("ImageRef") {
          *block = serde_json::json!({"Image": {
            "mime_type": image["mime_type"], "image_blob_id": image["blob_id"],
            "byte_count": image["byte_count"], "data_base64": "[image bytes represented by blob reference]"
          }});
          continue;
        }
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

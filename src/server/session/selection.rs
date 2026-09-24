use super::*;
use crate::server::media::SessionModel;
use crate::{
  Error,
  executor::model::{CallResponse, ModelCaller},
  protocol::{
    Request, TokenCount, UpstreamCompaction, UpstreamCompactionRequest, model_use::ModelUseProtocol,
  },
  session::SessionError,
};

#[derive(Clone, Serialize, Deserialize)]
pub struct PendingSelection {
  pub provider: String,
  pub config: SessionConfig,
}

// Only the executor's boundary callback replaces this pointer. An active request retains
// its own model/client; HTTP edits merely update the pending descriptor.
pub struct SwitchingModel(RwLock<Arc<SessionModel>>);
impl SwitchingModel {
  pub fn new(model: SessionModel) -> Self {
    Self(RwLock::new(Arc::new(model)))
  }
  fn current(&self) -> Arc<SessionModel> {
    self.0.read().unwrap().clone()
  }
}
impl ModelCaller for SwitchingModel {
  type Stream = <SessionModel as ModelCaller>::Stream;
  fn get_model_use_protocol(&self) -> Option<ModelUseProtocol> {
    self.current().get_model_use_protocol()
  }
  fn supports_upstream_compaction(&self) -> bool {
    self.current().supports_upstream_compaction()
  }
  fn validate_request(&self, request: &Request) -> Result<(), Error> {
    self.current().validate_request(request)
  }
  async fn count_tokens(&self, request: &Request) -> Result<Option<TokenCount>, Error> {
    self.current().count_tokens(request).await
  }
  async fn compact_upstream(
    &self,
    request: &UpstreamCompactionRequest,
  ) -> Result<UpstreamCompaction, Error> {
    self.current().compact_upstream(request).await
  }
  async fn call(&self, request: &Request) -> Result<CallResponse<Self::Stream>, Error> {
    self.current().call(request).await
  }
}
impl SessionSlot {
  pub fn make_model(&self, provider: Arc<Provider>, provider_id: String) -> SessionModel {
    let session_id = self.get_descriptor().id;
    let client = crate::server::sampling::observe_client(
      &provider.client,
      self.index.clone(),
      self.tasks.clone(),
      provider_id.clone(),
      Some(session_id.clone()),
    )
    .with_session_id(session_id);
    SessionModel::new(provider, provider_id, self.image_dir.clone()).with_client(client)
  }
  pub fn apply_selection(
    &self,
    session: &mut Session,
    model: &SwitchingModel,
  ) -> Result<bool, SessionError> {
    let mut descriptor = self.descriptor.write().unwrap();
    let Some(pending) = descriptor.pending_selection.clone() else {
      return Ok(false);
    };
    let app = self
      .app
      .upgrade()
      .ok_or_else(|| SessionError::InvalidCompaction("application closed".into()))?;
    let provider = app
      .get_provider(&pending.provider)
      .map_err(|e| SessionError::InvalidCompaction(e.to_string()))?;
    let client = crate::server::sampling::observe_client(
      &provider.client,
      self.index.clone(),
      self.tasks.clone(),
      pending.provider.clone(),
      Some(descriptor.id.clone()),
    )
    .with_session_id(descriptor.id.clone());
    let next_model = SessionModel::new(provider, pending.provider.clone(), self.image_dir.clone())
      .with_client(client);
    // Save previous calls before changing provider attribution.
    self
      .index
      .save_calls(&descriptor, &self.calls)
      .map_err(|e| SessionError::InvalidCompaction(e.to_string()))?;
    session.set_config(pending.config)?;
    descriptor.provider = pending.provider;
    descriptor.pending_selection = None;
    *model.0.write().unwrap() = Arc::new(next_model);
    drop(descriptor);
    let mut status = self.status.lock().unwrap();
    status["config"] = json!(session.get_config());
    status["standby_preparing"] = json!(false);
    drop(status);
    self.persist_index().map_err(|e| SessionError::InvalidCompaction(e.to_string()))?;
    Ok(true)
  }
}

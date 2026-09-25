use super::*;
use crate::server::media::SessionModel;
use crate::{
  Error,
  executor::{BoundaryResult, ExecutionControl, RunBoundary, compaction::translation::{self, Handoff}, model::{CallResponse, ModelCaller}, notify_observers},
  protocol::{
    ContentBlock, Message, Request, TokenCount, UpstreamCompaction, UpstreamCompactionRequest, model_use::ModelUseProtocol,
  },
  session::{RunOutcome, SessionError, SessionEvent},
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

pub struct SelectionBoundary<'a> {
  pub slot: &'a SessionSlot,
  pub model: &'a SwitchingModel,
}
impl RunBoundary for SelectionBoundary<'_> {
  async fn apply(
    &mut self,
    session: &mut Session,
    control: &ExecutionControl,
    cursor: &mut u64,
    observe: &mut (impl FnMut(&SessionEvent) + Send),
  ) -> Result<BoundaryResult, SessionError> {
    self.slot.apply_selection(session, self.model, control, cursor, observe).await
  }
}

fn missing_context_placeholder() -> Message {
  Message::Developer {
    metadata: json!({"source":"upstream_compaction_handoff_unavailable"}),
    fixed: Some(false),
    content: vec![ContentBlock::Text {
      text: "[Earlier conversation was compressed into an encrypted item by the previous provider. It could not be translated for this model, so part of the earlier context is missing. Ask for needed details rather than inventing them.]".into(),
    }],
  }
}

impl SessionSlot {
  pub fn execution_provider(
    &self,
    app: &crate::server::app::App,
  ) -> Result<(Arc<Provider>, String), ApiError> {
    let descriptor = self.get_descriptor();
    match app.get_provider(&descriptor.provider) {
      Ok(provider) => Ok((provider, descriptor.provider)),
      Err(error) => match descriptor.pending_selection {
        Some(pending) => app.get_provider(&pending.provider).map(|provider| (provider, pending.provider)),
        None => Err(error),
      },
    }
  }

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
  pub async fn apply_selection(
    &self,
    session: &mut Session,
    model: &SwitchingModel,
    control: &ExecutionControl,
    cursor: &mut u64,
    observe: &mut (impl FnMut(&SessionEvent) + Send),
  ) -> Result<BoundaryResult, SessionError> {
    loop {
      if control.is_cancelled() {
        return Ok(BoundaryResult::Interrupted);
      }
      let selected = self.get_descriptor();
      let Some(pending) = selected.pending_selection.clone() else {
        return Ok(BoundaryResult::Unchanged);
      };
      let app = self
        .app
        .upgrade()
        .ok_or_else(|| SessionError::InvalidCompaction("application closed".into()))?;
      let provider = app
        .get_provider(&pending.provider)
        .map_err(|e| SessionError::InvalidCompaction(e.to_string()))?;
      let next_model = self.make_model(provider, pending.provider.clone());
      let mut translated = None;
      if !next_model.supports_upstream_compaction() {
        let old_request = session.build_request()?;
        let positions: Vec<_> = old_request.conversation.iter().enumerate()
          .filter_map(|(index, message)| matches!(message, Message::UpstreamCompaction { .. }).then_some(index))
          .collect();
        if let Some(&first) = positions.first() {
          let active = session.get_active_generation()?;
          let list = session.get_generation_entries(active.id)?;
          let ids: Vec<_> = list.read_page(0, list.len()? as usize)?.items.iter().map(|id| **id).collect();
          if ids.len() != old_request.conversation.len() {
            return Err(SessionError::StaleGeneration);
          }
          let handoff = if positions.len() != 1 {
            Handoff::Failed(RunOutcome::Failed(Error::Build("multiple encrypted compaction items cannot be translated together".into())))
          } else if !model.current().supports_upstream_compaction()
            || app.get_provider(&selected.provider).is_err()
          {
            Handoff::Failed(RunOutcome::Failed(Error::Build(format!(
              "previous provider `{}` is unavailable for encrypted context translation",
              selected.provider,
            ))))
          } else {
            let mut request = old_request.clone();
            request.conversation.truncate(first + 1);
            translation::generate_handoff(
              model.current().as_ref(), session, control, cursor, observe, request, (first + 1) as u64,
            ).await?
          };
          if matches!(handoff, Handoff::Interrupted) || control.is_cancelled() {
            return Ok(BoundaryResult::Interrupted);
          }
          let (replacement, failure) = match handoff {
            Handoff::Translated(message) => (message, None),
            Handoff::Failed(outcome) => (missing_context_placeholder(), Some(outcome)),
            Handoff::Interrupted => unreachable!(),
          };
          let after: Vec<_> = (first + 1..ids.len())
            .filter(|index| !positions.contains(index))
            .collect();
          let conversation: Vec<_> = old_request.conversation[..first].iter()
            .cloned()
            .chain(std::iter::once(replacement.clone()))
            .chain(after.iter().map(|index| old_request.conversation[*index].clone()))
            .collect();
          next_model.validate_request(&pending.config.build_request(conversation))
            .map_err(|error| SessionError::InvalidCompaction(format!("translated context is invalid for the selected provider: {error}")))?;
          translated = Some((active.id, ids[..first].to_vec(), replacement,
            after.into_iter().map(|index| ids[index]).collect(), failure));
        }
      }

      let mut descriptor = self.descriptor.write().unwrap();
      if descriptor.revision != selected.revision {
        continue;
      }
      if control.is_cancelled() {
        return Ok(BoundaryResult::Interrupted);
      }
      // Keep the handoff call attributed to its old provider before changing the descriptor.
      self.index.save_calls(&descriptor, &self.calls)
        .map_err(|error| SessionError::InvalidCompaction(error.to_string()))?;
      let translated_now = translated.is_some();
      if let Some((generation, prefix, replacement, suffix, failure)) = translated {
        session.commit_compaction_translation(generation, prefix, replacement, suffix, failure, pending.config.clone())?;
      } else {
        session.set_config(pending.config)?;
      }
      descriptor.provider = pending.provider;
      descriptor.pending_selection = None;
      *model.0.write().unwrap() = Arc::new(next_model);
      drop(descriptor);
      let mut status = self.status.lock().unwrap();
      status["config"] = json!(session.get_config());
      status["standby_preparing"] = json!(false);
      drop(status);
      if translated_now {
        notify_observers(session, cursor, observe)?;
      }
      self.persist_index().map_err(|error| SessionError::InvalidCompaction(error.to_string()))?;
      return Ok(BoundaryResult::Changed);
    }
  }
}

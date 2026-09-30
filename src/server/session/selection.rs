//! Changing a session's provider and model. While no operation runs, a change applies at once,
//! unless the context holds an encrypted compaction item the new provider cannot read and that has
//! no handoff yet. Then, and whenever an operation runs, the change is staged as the descriptor's
//! pending selection and applied at the next boundary of a run (see `SelectionBoundary`), after the
//! provider that made the item has written the handoff.
use super::{Descriptor, SessionSlot};
use crate::server::{
  app::App, compaction_item, error::ApiError, provider::Provider, sampling,
  session_model::SessionModel,
};
use crate::{
  Error,
  executor::{
    BoundaryResult, ExecutionControl, RunBoundary,
    compaction::handoff::{self, HandoffContext, PreviousProvider},
    deliver_new_events,
    model::{CallResponse, ModelCaller},
  },
  protocol::{
    Message, Request, TokenCount, UpstreamCompaction, UpstreamCompactionRequest,
    model_use::ModelUseProtocol,
  },
  session::{Session, SessionConfig, SessionError, SessionEvent},
};
use axum::http::HeaderValue;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::{Arc, RwLock};

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
  /// Switches to another model; a request already under way keeps the one it started with.
  fn replace(&self, model: SessionModel) {
    *self.0.write().unwrap() = Arc::new(model);
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
    delivered: &mut u64,
    observe: &mut (impl FnMut(&SessionEvent) + Send),
  ) -> Result<BoundaryResult, SessionError> {
    self.slot.apply_selection(session, self.model, control, delivered, observe).await
  }
}

/// A pending selection being applied at a boundary.
struct Switch {
  /// The descriptor it was read from: the switch applies only while it is still current.
  selected: Descriptor,
  pending: PendingSelection,
  model: SessionModel,
  /// Whether the provider switched from is still configured, to write a handoff for what only it
  /// can read.
  previous_available: bool,
}
impl Switch {
  /// Whether the provider switched from can read an encrypted item now.
  fn previous_reads(&self, item: &Message) -> bool {
    self.previous_available && compaction_item::can_read(item, &self.selected.provider)
  }
}
enum Prepared {
  /// The context change a switch needs, if any: the one encrypted item the next provider cannot
  /// read, replaced by a copy carrying its handoff, with later items of its kind left out.
  Ready(Option<Box<HandoffContext>>),
  Interrupted,
}
enum Committed {
  Done {
    translated: bool,
  },
  /// The descriptor changed since the switch was read.
  Stale,
  Interrupted,
}

/// The session engine's own errors are all a boundary may fail with.
fn boundary_error(error: impl std::fmt::Display) -> SessionError {
  SessionError::InvalidCompaction(error.to_string())
}

impl SessionSlot {
  /// The provider an operation starts with: the session's, or when that one is gone, the pending
  /// selection's, which applies at the operation's first boundary.
  pub fn execution_provider(&self, app: &App) -> Result<Arc<Provider>, ApiError> {
    let descriptor = self.get_descriptor();
    match app.get_provider(&descriptor.provider) {
      Ok(provider) => Ok(provider),
      Err(error) => match descriptor.pending_selection {
        Some(pending) => app.get_provider(&pending.provider),
        None => Err(error),
      },
    }
  }
  /// The model this session calls a provider through: its client sampled into the index and
  /// carrying the session's id.
  pub fn make_model(&self, provider: Arc<Provider>) -> SessionModel {
    let session_id = self.get_descriptor().id;
    let client = sampling::with_stream_sampling(
      &provider.client,
      self.management.clone(),
      self.tasks.clone(),
      provider.id.clone(),
      Some(session_id.clone()),
    )
    .with_session_id(session_id);
    SessionModel::new(provider, self.image_dir.clone(), client)
  }
  /// Stages a selection while an operation runs. Only the model, reasoning and output limit may
  /// differ from the config the session has, or has staged already.
  pub fn stage_selection(
    &self,
    app: &App,
    if_match: Option<&HeaderValue>,
    provider: Option<&Value>,
    config: SessionConfig,
  ) -> Result<(), ApiError> {
    self.edit_descriptor(if_match, |next| {
      let mut current = match &next.pending_selection {
        Some(pending) => json!(pending.config),
        None => json!(self.status.lock().unwrap().config),
      };
      let mut desired = json!(config);
      for key in ["model", "reasoning", "max_output_tokens"] {
        current.as_object_mut().unwrap().remove(key);
        desired.as_object_mut().unwrap().remove(key);
      }
      if current != desired {
        return Err(ApiError::conflict(
          "only model, reasoning and output limit can change while running",
        ));
      }
      let provider = provider
        .map(|v| v.as_str().ok_or_else(|| ApiError::bad_request("provider must be a string")))
        .transpose()?
        .unwrap_or_else(|| {
          next.pending_selection.as_ref().map(|p| p.provider.as_str()).unwrap_or(&next.provider)
        })
        .to_owned();
      app.get_provider(&provider)?;
      next.pending_selection = Some(PendingSelection { provider, config });
      Ok(())
    })
  }
  /// Applies the pending selection, if there is one, at a boundary of the operation that `model`
  /// serves: writes the handoff an encrypted item needs first, then switches the session's config,
  /// provider and model together.
  pub async fn apply_selection(
    &self,
    session: &mut Session,
    model: &SwitchingModel,
    control: &ExecutionControl,
    delivered: &mut u64,
    observe: &mut (impl FnMut(&SessionEvent) + Send),
  ) -> Result<BoundaryResult, SessionError> {
    loop {
      if control.is_cancelled() {
        return Ok(BoundaryResult::Interrupted);
      }
      let Some(switch) = self.read_switch()? else {
        return Ok(BoundaryResult::Unchanged);
      };
      let translation =
        match self.translate(&switch, session, model, control, delivered, observe).await? {
          Prepared::Ready(translation) => translation,
          Prepared::Interrupted => return Ok(BoundaryResult::Interrupted),
        };
      match self.commit_switch(switch, translation, session, model, control)? {
        Committed::Done { translated } => {
          if translated {
            deliver_new_events(session, delivered, observe)?;
          }
          self.persist_index().map_err(boundary_error)?;
          return Ok(BoundaryResult::Changed);
        }
        Committed::Stale => continue,
        Committed::Interrupted => return Ok(BoundaryResult::Interrupted),
      }
    }
  }
  fn read_switch(&self) -> Result<Option<Switch>, SessionError> {
    let selected = self.get_descriptor();
    let Some(pending) = selected.pending_selection.clone() else { return Ok(None) };
    let app = self.app.upgrade().ok_or_else(|| boundary_error("application closed"))?;
    let provider = app.get_provider(&pending.provider).map_err(boundary_error)?;
    let model = self.make_model(provider);
    let previous_available = app.get_provider(&selected.provider).is_ok();
    Ok(Some(Switch { selected, pending, model, previous_available }))
  }
  /// Writes the handoff the switch needs, when the context holds an encrypted item the next
  /// provider cannot read. It travels as that handoff: one needs writing when the item has none, or
  /// only a placeholder that the provider switched from, which can read the item, can replace.
  async fn translate(
    &self,
    switch: &Switch,
    session: &mut Session,
    model: &SwitchingModel,
    control: &ExecutionControl,
    delivered: &mut u64,
    observe: &mut (impl FnMut(&SessionEvent) + Send),
  ) -> Result<Prepared, SessionError> {
    let plan = handoff::plan_handoff(session, |message| {
      compaction_item::needs_handoff(message, &switch.pending.provider)
        && (compaction_item::get_handoff(message).is_none() || switch.previous_reads(message))
    })?;
    let Some(plan) = plan else { return Ok(Prepared::Ready(None)) };
    let current = model.current();
    let previous = PreviousProvider {
      caller: current.as_ref(),
      name: &switch.selected.provider,
      reads_item: switch.previous_reads(plan.item()),
    };
    let written =
      handoff::write_handoff(previous, session, control, delivered, observe, &plan).await?;
    let Some((content, failure)) = written.into_content().filter(|_| !control.is_cancelled())
    else {
      return Ok(Prepared::Interrupted);
    };
    // The item stays, encrypted content and all, so the provider that made it reads it again
    // after switching back; the handoff rides along for every other provider.
    let replacement = compaction_item::with_handoff(plan.item(), &content, failure.is_none());
    let (context, conversation) = plan.into_context(replacement, failure);
    switch.model.validate_request(&switch.pending.config.build_request(conversation)).map_err(
      |error| {
        boundary_error(format!("translated context is invalid for the selected provider: {error}"))
      },
    )?;
    Ok(Prepared::Ready(Some(Box::new(context))))
  }
  /// Switches the session's config (with the translation, if any), the descriptor's provider and
  /// the model together, unless the descriptor changed since the switch was read.
  fn commit_switch(
    &self,
    switch: Switch,
    translation: Option<Box<HandoffContext>>,
    session: &mut Session,
    model: &SwitchingModel,
    control: &ExecutionControl,
  ) -> Result<Committed, SessionError> {
    let mut descriptor = self.descriptor.write().unwrap();
    if descriptor.revision != switch.selected.revision {
      return Ok(Committed::Stale);
    }
    if control.is_cancelled() {
      return Ok(Committed::Interrupted);
    }
    // Keep the handoff call attributed to its old provider before changing the descriptor.
    let calls = self.reader.get_model_calls();
    self.management.save_calls(&descriptor, &calls).map_err(boundary_error)?;
    let translated = translation.is_some();
    let Switch { pending, model: next_model, .. } = switch;
    match translation.map(|translation| *translation) {
      Some(HandoffContext { generation, prefix, replacement, suffix, failure }) => {
        session.commit_compaction_translation(
          generation,
          prefix,
          replacement,
          suffix,
          failure,
          pending.config.clone(),
        )?;
      }
      None => session.set_config(pending.config)?,
    }
    descriptor.provider = pending.provider;
    descriptor.pending_selection = None;
    model.replace(next_model);
    drop(descriptor);
    self.selection_applied(session.get_config());
    Ok(Committed::Done { translated })
  }
}

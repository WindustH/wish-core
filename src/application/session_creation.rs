//! Validate and create a session, including idempotent first-input submission.
use super::{
  app::App,
  error::{ApplicationError, blocking},
  instructions::apply_agent_instructions,
  session::{CreateSession, Descriptor, SessionSlot, ToolSwitches},
};
use crate::{
  session::{EntryOrigin, Session},
  utils::time::Timestamp,
};
use serde_json::json;
use std::sync::Arc;
impl App {
  pub async fn create_session(
    self: &Arc<Self>,
    mut input: CreateSession,
  ) -> Result<Arc<SessionSlot>, ApplicationError> {
    self.lifecycle.require_open()?;
    // Serialize creation/replay so a retry after a lost response cannot enqueue the first input twice.
    let _creation = self.creation_lock.lock().await;
    let initial_input_digest = input
      .first_input
      .as_ref()
      .map(|first| {
        serde_json::to_vec(first)
          .map(|bytes| crate::storage::blobs::blob_id(&bytes))
          .map_err(ApplicationError::internal)
      })
      .transpose()?;
    let id = match input.first_input.as_ref() {
      Some(first) => uuid::Uuid::parse_str(&first.request_id)
        .map_err(|_| ApplicationError::invalid("first_input.request_id must be a UUID"))?
        .to_string(),
      None => uuid::Uuid::new_v4().to_string(),
    };
    if input.first_input.is_some() {
      match self.get_session(&id).await {
        Ok(slot) if slot.get_descriptor().initial_input_digest == initial_input_digest => {
          return Ok(slot);
        }
        Ok(_) => {
          return Err(ApplicationError::conflict(
            "request_id was already used for different input",
          ));
        }
        Err(error) if error.kind == super::error::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
      }
    }
    self.get_provider(&input.provider)?;
    if !input.cwd.is_absolute() {
      return Err(ApplicationError::invalid("cwd must be absolute"));
    }
    if !tokio::fs::metadata(&input.cwd)
      .await
      .map_err(|e| ApplicationError::invalid(e.to_string()))?
      .is_dir()
    {
      return Err(ApplicationError::invalid("cwd must be a directory"));
    }
    if !input.config.tools.is_empty() {
      return Err(ApplicationError::invalid(
        "tools are installed by the server; pass an empty tools array",
      ));
    }
    // Without a word from the request: no shell and no web search, and questions to the user and
    // MCP allowed. Web search stays off while no search provider could answer it.
    // Delivery defaults are copied once. Existing descriptors never follow global changes.
    let delivery_defaults = self.config_file.lock().await.config.defaults.tools;
    let mut tools = ToolSwitches {
      history_search: delivery_defaults.history_search,
      history_search_delivery: delivery_defaults.history_search_delivery,
      task_delivery: delivery_defaults.task_delivery,
      web_fetch_delivery: delivery_defaults.web_fetch_delivery,
      web_search_delivery: delivery_defaults.web_search_delivery,
      ask_user_delivery: delivery_defaults.ask_user_delivery,
      shell: false,
      ask_user: true,
      mcp: true,
      web_search: false,
      web_fetch: false,
      video_generate: delivery_defaults.video_generate,
      video_generate_delivery: delivery_defaults.video_generate_delivery,
      audio_transcribe: delivery_defaults.audio_transcribe,
      audio_transcribe_delivery: delivery_defaults.audio_transcribe_delivery,
      audio_generate: delivery_defaults.audio_generate,
      audio_generate_delivery: delivery_defaults.audio_generate_delivery,
      image_generate: delivery_defaults.image_generate,
      image_generate_delivery: delivery_defaults.image_generate_delivery,
      skills: true,
      sessions: delivery_defaults.sessions,
      sessions_delivery: delivery_defaults.sessions_delivery,
    }
    .apply_changes(input.tools);
    tools.web_search &= self.search.is_available(&self.get_service_accounts());
    tools.web_fetch &= self.fetch.available();
    let folder = input.folder.take();
    if let Some(folder) = folder.clone() {
      let management = self.management.clone();
      blocking(move || {
        if management.folder(&folder)?.is_none() {
          return Err(ApplicationError::invalid(format!("no folder `{folder}`")));
        }
        Ok(())
      })
      .await?;
    }
    let descriptor = Descriptor {
      initial_input_digest,
      pending_selection: None,
      created_by: input.created_by,
      id,
      provider: input.provider,
      cwd: input.cwd,
      tools,
      shell_override: None,
      created_at: Timestamp::now().0,
      updated_at: Timestamp::now().0,
      name: input.name,
      revision: 0,
    };
    let first_message = if let Some(first_input) = input.first_input {
      let directory = self.data_dir.blobs(&descriptor.id);
      match first_input.prepare(&directory).await {
        Ok(message) => Some(message),
        Err(error) => {
          let _ = tokio::fs::remove_dir_all(&directory).await;
          return Err(error);
        }
      }
    } else {
      None
    };
    let start_run = first_message.is_some();
    for message in &mut input.initial_messages {
      message.normalize_new_input();
    }
    // Agent instructions may be inserted at the front; origins line up from the back.
    apply_agent_instructions(&mut input.initial_messages, tools);
    let environment = crate::protocol::Message::System {
      metadata: json!({"source":"wish_environment"}),
      content: vec![crate::protocol::ContentBlock::Text {
        text: super::instructions::environment_instruction(
          &self.shell.read().unwrap(),
          &descriptor.cwd,
        ),
      }],
    };
    if let Some(existing) = input.initial_messages.iter_mut().find(|message| matches!(message,crate::protocol::Message::System{metadata,..} if metadata["source"]=="wish_environment")) {
      *existing=environment;
    } else { input.initial_messages.insert(1,environment); }
    let inserted = input.initial_messages.len().saturating_sub(input.initial_origins.len());
    let origins = std::iter::repeat_n(EntryOrigin::Imported, inserted).chain(input.initial_origins);
    let initial: Vec<_> = input.initial_messages.into_iter().zip(origins).collect();
    let storage = self.storage.clone();
    let id = descriptor.id.clone();
    let session = blocking(move || {
      Ok(Session::create_initialized(
        storage,
        &id,
        input.config,
        Some(input.metadata),
        initial,
        first_message,
      )?)
    })
    .await?;
    let id = descriptor.id.clone();
    let slot = match self.open_slot(descriptor, session).await {
      Ok(slot) => slot,
      Err(error) => {
        let (storage, management) = (self.storage.clone(), self.management.clone());
        if let Err(cleanup) = blocking(move || {
          let mut session = Session::load(storage, &id)?;
          session.delete()?;
          management.delete(&id)?;
          Ok(())
        })
        .await
        {
          eprintln!("rolling back session initialization: {cleanup}");
        }
        return Err(error);
      }
    };
    if let Some(folder) = folder {
      let management = self.management.clone();
      let id = slot.get_descriptor().id;
      if let Err(error) = blocking(move || Ok(management.place(&[id], Some(&folder))?)).await {
        let mut session = slot.session.clone().lock_owned().await;
        let owner = slot.clone();
        let management = self.management.clone();
        if let Err(cleanup) = blocking(move || {
          session.delete()?;
          owner.mark_deleted();
          management.delete(&owner.get_descriptor().id)?;
          Ok(())
        })
        .await
        {
          eprintln!("rolling back session placement: {cleanup}");
        }
        return Err(error);
      }
    }
    self.sessions.lock().await.insert(slot.get_descriptor().id.clone(), slot.clone());
    if start_run {
      slot.schedule(self);
    }
    Ok(slot)
  }
}

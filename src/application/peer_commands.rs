//! Session-authorized peer operations and ownership checks.
use crate::application::error::{ApplicationError, blocking};
use crate::application::{
  app::App,
  config::Defaults,
  session::{CreateSession, Descriptor, SessionSlot, ToolChanges},
};
use crate::protocol::{ContentBlock, Message};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;

/// Both direct tools and shell commands use the same live permission check.
fn authorized(slot: &Arc<SessionSlot>) -> Result<Arc<SessionSlot>, ApplicationError> {
  slot.require_live()?;
  if !slot.get_descriptor().tools.sessions {
    return Err(ApplicationError::conflict(
      "Session management is disabled for this session. The user can enable it in the session's settings.",
    ));
  }
  Ok(slot.clone())
}
fn forbidden(message: &str) -> ApplicationError {
  ApplicationError {
    kind: crate::application::error::ErrorKind::Forbidden,
    message: message.into(),
    details: None,
  }
}
/// Whether `me` may configure or delete `target`: itself, or a session it made.
fn may_manage(me: &Descriptor, target: &Descriptor) -> bool {
  target.id == me.id || target.created_by.as_deref() == Some(&me.id)
}

/// Resolve a session by ID or an unambiguous name.
async fn resolve_session(
  app: &Arc<App>,
  target: &str,
) -> Result<Arc<SessionSlot>, ApplicationError> {
  if let Ok(slot) = app.get_session(target).await {
    return Ok(slot);
  }
  let owner = app.clone();
  let name = target.to_owned();
  let matches = blocking(move || Ok(owner.management.named_sessions(&name)?)).await?;
  match matches.as_slice() {
    [id] => app.get_session(id).await,
    [] => Err(ApplicationError::not_found()),
    _ => Err(ApplicationError::invalid(format!("several are named `{target}`; use an id"))),
  }
}
fn display_name(descriptor: &Descriptor) -> String {
  if descriptor.name.is_empty() {
    descriptor.id.chars().take(8).collect()
  } else {
    descriptor.name.clone()
  }
}

/// Every session's descriptor and status, newest first.
fn sessions(app: &App) -> Result<Vec<(Descriptor, Value)>, ApplicationError> {
  Ok(
    app
      .management
      .list_all()?
      .into_iter()
      .filter_map(|record| {
        let descriptor = serde_json::from_value(record["session"].clone()).ok()?;
        Some((descriptor, record["status"].clone()))
      })
      .collect(),
  )
}
fn named(descriptor: &Descriptor) -> Value {
  json!({"id": descriptor.id, "name": display_name(descriptor)})
}
/// A session as a peer sees it.
fn session_summary(me: &str, descriptor: &Descriptor, status: &Value) -> Value {
  json!({
    "me": me,
    "kind": "session",
    "id": descriptor.id,
    "name": display_name(descriptor),
    "provider": descriptor.provider,
    "model": status["config"]["model"],
    "cwd": descriptor.cwd,
    "phase": status["phase"],
    "running": status["running"],
    "queue": status["queue_count"].as_u64().unwrap_or(0),
    "created_by": descriptor.created_by,
    "instructions": status["metadata"]["agent_custom"],
  })
}
pub(super) async fn list_inner(
  app: Arc<App>,
  id: String,
  slot: Arc<SessionSlot>,
) -> Result<Value, ApplicationError> {
  authorized(&slot)?;
  blocking(move || {
    let summaries: Vec<_> = sessions(&app)?
      .iter()
      .map(|(descriptor, status)| session_summary(&id, descriptor, status))
      .collect();
    Ok(json!({"me": id, "sessions": summaries}))
  })
  .await
}
pub(super) async fn show_inner(
  app: Arc<App>,
  (id, target): (String, String),
  slot: Arc<SessionSlot>,
) -> Result<Value, ApplicationError> {
  authorized(&slot)?;
  let target = resolve_session(&app, &target).await?;
  let status = serde_json::to_value(target.get_status()).map_err(ApplicationError::internal)?;
  Ok(session_summary(&id, &target.get_descriptor(), &status))
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NewSession {
  #[serde(default)]
  name: String,
  #[serde(default)]
  instructions: Option<String>,
  #[serde(default)]
  model: Option<String>,
  #[serde(default)]
  provider: Option<String>,
  #[serde(default)]
  cwd: Option<String>,
  #[serde(default)]
  shell: Option<bool>,
}
/// Makes a session for the one asking: on the configured defaults, in the asking session's
/// directory unless another is given, with the instructions it gives.
pub(super) async fn create_inner(
  app: Arc<App>,
  _id: String,
  slot: Arc<SessionSlot>,
  input: NewSession,
) -> Result<Value, ApplicationError> {
  let me = authorized(&slot)?;
  let mine = me.get_descriptor();
  let defaults: Defaults = app.config_file.lock().await.config.defaults.clone();
  let provider = input.provider.unwrap_or_else(|| {
    if defaults.provider.is_empty() { mine.provider.clone() } else { defaults.provider.clone() }
  });
  let model = input.model.unwrap_or_else(|| {
    if defaults.model.is_empty() {
      me.get_status().status.config.model.clone()
    } else {
      defaults.model.clone()
    }
  });
  if model.is_empty() {
    return Err(ApplicationError::invalid("no default model is configured; give --model"));
  }
  let mut config = defaults.session_config();
  config.model = model;
  let mut tools = ToolChanges::from(defaults.tools);
  if let Some(shell) = input.shell {
    tools.shell = Some(shell);
  }
  let instructions = input.instructions.unwrap_or_default();
  let initial_messages = if instructions.is_empty() {
    Vec::new()
  } else {
    vec![Message::System {
      metadata: Value::Null,
      content: vec![ContentBlock::Text { text: instructions.clone() }],
    }]
  };
  let slot = app
    .create_session(CreateSession {
      first_input: None,
      created_by: Some(mine.id.clone()),
      // Beside the session that makes it, as a file is made in the directory one works in.
      folder: {
        let store = app.management.clone();
        let id = mine.id.clone();
        blocking(move || Ok(store.folder_of(&id)?)).await?
      },
      initial_messages,
      initial_origins: Vec::new(),
      name: input.name,
      provider,
      config,
      cwd: input.cwd.map(Into::into).unwrap_or_else(|| mine.cwd.clone()),
      tools,
      metadata: json!({"agent_custom": instructions}),
    })
    .await?;
  let status = serde_json::to_value(slot.get_status()).map_err(ApplicationError::internal)?;
  Ok(session_summary(&mine.id, &slot.get_descriptor(), &status))
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Changes {
  #[serde(default)]
  name: Option<String>,
  #[serde(default)]
  model: Option<String>,
  #[serde(default)]
  provider: Option<String>,
  #[serde(default)]
  instructions: Option<String>,
}
/// Changes a session the asking one made, or itself - its name, model, provider or instructions,
/// the last given to it as a message as well. A session's changes go
/// as the user's do: a name at any time, a model at the next boundary when it runs, and
/// instructions only while it is idle.
pub(super) async fn configure_inner(
  app: Arc<App>,
  (_id, target): (String, String),
  slot: Arc<SessionSlot>,
  changes: Changes,
) -> Result<Value, ApplicationError> {
  let me = authorized(&slot)?;
  let slot = resolve_session(&app, &target).await?;
  if !may_manage(&me.get_descriptor(), &slot.get_descriptor()) {
    return Err(forbidden("only a session you made, or yourself, can be configured"));
  }
  let status = slot.get_status().status;
  if changes.instructions.is_some() && status.running {
    return Err(ApplicationError::conflict(
      "instructions can change only while the session is idle, so not your own during your turn",
    ));
  }
  let mut input = serde_json::Map::new();
  if changes.model.is_some() || changes.provider.is_some() {
    let mut config = status.config.clone();
    if let Some(model) = changes.model {
      config.model = model;
    }
    config.tools.clear();
    input
      .insert("config".into(), serde_json::to_value(config).map_err(ApplicationError::internal)?);
    if let Some(provider) = changes.provider {
      input.insert("provider".into(), Value::String(provider));
    }
  }
  if let Some(instructions) = &changes.instructions {
    let mut metadata = status.metadata.clone();
    if !metadata.is_object() {
      metadata = json!({});
    }
    metadata["agent_custom"] = Value::String(instructions.clone());
    input.insert("metadata".into(), metadata);
  }
  let mut shown = None;
  // Idle edits share one revision and publication. Running model changes are staged;
  // their independently allowed rename keeps the existing runtime policy.
  if !status.running
    && let Some(name) = &changes.name
  {
    input.insert("name".into(), Value::String(name.clone()));
  }
  if !input.is_empty() {
    shown = Some(
      crate::application::session_edit::update(
        app.clone(),
        slot.clone(),
        None,
        Value::Object(input),
      )
      .await?,
    );
  }
  if status.running
    && let Some(name) = changes.name
  {
    shown = Some(
      crate::application::session_edit::update(
        app.clone(),
        slot.clone(),
        None,
        json!({"name": name}),
      )
      .await?,
    );
  }
  if let Some(instructions) = changes.instructions.filter(|text| !text.is_empty()) {
    let message = Message::System {
      metadata: json!({"source": "instructions_changed"}),
      content: vec![ContentBlock::Text { text: instructions }],
    };
    slot.enqueue(message).await?;
  }
  shown.ok_or_else(|| ApplicationError::invalid("nothing to change"))
}
pub(super) async fn delete_inner(
  app: Arc<App>,
  (id, target): (String, String),
  slot: Arc<SessionSlot>,
) -> Result<(), ApplicationError> {
  let me = authorized(&slot)?;
  let slot = resolve_session(&app, &target).await?;
  let descriptor = slot.get_descriptor();
  if descriptor.id == id {
    return Err(forbidden("a session cannot delete itself"));
  }
  if !may_manage(&me.get_descriptor(), &descriptor) {
    return Err(forbidden("only a session you made can be deleted"));
  }
  crate::application::session_edit::delete(&app, &descriptor.id).await?;
  Ok(())
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Send {
  text: String,
}
/// Deliver directly to the recipient's durable input queue; no conversation or transcript is created.
pub(super) async fn send_inner(
  app: Arc<App>,
  (_id, target): (String, String),
  slot: Arc<SessionSlot>,
  input: Send,
) -> Result<Value, ApplicationError> {
  let me = authorized(&slot)?;
  let mine = me.get_descriptor();
  if input.text.trim().is_empty() {
    return Err(ApplicationError::invalid("nothing to send"));
  }
  let recipient = resolve_session(&app, &target).await?;
  recipient.require_live()?;
  let to = recipient.get_descriptor();
  if to.id == mine.id {
    return Err(ApplicationError::invalid("that is you"));
  }
  let from = named(&mine);
  let aliases = recipient.aliases.clone();
  let sender_id = mine.id.clone();
  let source_aliases = slot.aliases.clone();
  let text = input.text;
  let publication = recipient.blob_publication.clone().read_owned().await;
  recipient.require_live()?;
  let (sender_alias, text, publication) = blocking(move || {
    Ok((aliases.session(&sender_id)?, aliases.receive(&source_aliases, &text)?, publication))
  })
  .await?;
  let heading =
    format!("[Message from session {} ({sender_alias})]\n", serde_json::json!(display_name(&mine)));
  let queued = recipient.enqueue_published(Message::User {
    metadata: json!({"source":"session_message", "from": {"kind":"session", "id":mine.id, "name":display_name(&mine)}}),
    content: vec![ContentBlock::Text { text: format!("{heading}{text}") }],
  }, publication).await?;
  // Schedule immediately after durable enqueue; optional index refresh must not lose delivery.
  recipient.schedule(&app);
  let touched = recipient.clone();
  if let Err(error) = blocking(move || touched.touch()).await {
    eprintln!("peer message index: {error}");
  }
  Ok(json!({"sent":true, "from":from, "to":named(&to), "queued":queued}))
}

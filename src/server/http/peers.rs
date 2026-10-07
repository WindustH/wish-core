//! Peers over HTTP: what `wish session` asks through the bridge, as a session. A session's
//! `sessions` switch is enforced here and nowhere else, and so is what a session may do to
//! another: see every session and group, message any session, and configure or delete only itself
//! and the sessions it made.
use super::{bridge, manage};
use crate::protocol::{ContentBlock, Message};
use crate::server::{
  app::App,
  config::Defaults,
  error::ApiError,
  groups::{self, Author, GroupRecord, display_name},
  session::{CreateSession, Descriptor, SessionSlot, ToolChanges},
};
use axum::{
  Json,
  extract::{Path, State},
  http::{HeaderMap, StatusCode},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc};

/// The session a bridge request speaks for, when its sessions switch is on.
async fn me(app: &Arc<App>, id: &str, headers: &HeaderMap) -> Result<Arc<SessionSlot>, ApiError> {
  let slot = bridge::session(app, id, headers).await?;
  if !slot.get_descriptor().tools.sessions {
    // Said to the model through the command's output, so it knows why and who can change it.
    return Err(ApiError::conflict(
      "Session management is disabled for this session. The user can enable it in the session's \
       settings.",
    ));
  }
  Ok(slot)
}
fn forbidden(message: &str) -> ApiError {
  ApiError { status: StatusCode::FORBIDDEN, message: message.into(), details: None }
}
/// Whether `me` may configure or delete `target`: itself, or a session it made.
fn may_manage(me: &Descriptor, target: &Descriptor) -> bool {
  target.id == me.id || target.created_by.as_deref() == Some(&me.id)
}

/// What a target names.
enum Target {
  Session(Arc<SessionSlot>),
  Group(GroupRecord),
}
/// The session or group `target` names: by id, or by a name only one of them bears.
async fn resolve(app: &Arc<App>, target: &str) -> Result<Target, ApiError> {
  if let Some(group) = app.management.group(target)? {
    return Ok(Target::Group(group));
  }
  if let Ok(slot) = app.get_session(target).await {
    return Ok(Target::Session(slot));
  }
  let mut named: Vec<Target> = Vec::new();
  for (descriptor, _) in sessions(app)? {
    if descriptor.name.eq_ignore_ascii_case(target) {
      named.push(Target::Session(app.get_session(&descriptor.id).await?));
    }
  }
  for group in app.management.groups()? {
    if group.name.eq_ignore_ascii_case(target) {
      named.push(Target::Group(group));
    }
  }
  match named.len() {
    0 => Err(ApiError::not_found()),
    1 => Ok(named.remove(0)),
    _ => Err(ApiError::bad_request(format!("several are named `{target}`; use an id"))),
  }
}
async fn resolve_session(app: &Arc<App>, target: &str) -> Result<Arc<SessionSlot>, ApiError> {
  match resolve(app, target).await? {
    Target::Session(slot) => Ok(slot),
    Target::Group(_) => Err(ApiError::bad_request(format!("`{target}` is a group, not a session"))),
  }
}
async fn resolve_group(app: &Arc<App>, target: &str) -> Result<GroupRecord, ApiError> {
  match resolve(app, target).await? {
    Target::Group(group) => Ok(group),
    Target::Session(_) => {
      Err(ApiError::bad_request(format!("`{target}` is a session, not a group")))
    }
  }
}

/// Every session's descriptor and status, newest first.
fn sessions(app: &App) -> Result<Vec<(Descriptor, Value)>, ApiError> {
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
/// The sessions `ids` names, each `{"id", "name"}`; one deleted meanwhile is left out.
fn named_ids(app: &App, ids: &[String]) -> Vec<Value> {
  ids
    .iter()
    .filter_map(|id| app.management.read(id).ok())
    .filter_map(|record| serde_json::from_value::<Descriptor>(record["session"].clone()).ok())
    .map(|descriptor| named(&descriptor))
    .collect()
}
/// A session as a peer sees it.
fn session_summary(
  me: &str,
  descriptor: &Descriptor,
  status: &Value,
  groups: &[&GroupRecord],
) -> Value {
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
    "groups": groups.iter().map(|group| json!({"id": group.id, "name": group.name})).collect::<Vec<_>>(),
  })
}
/// A group as a peer sees it, with its `members` named.
fn group_summary(group: &GroupRecord, members: Vec<Value>) -> Value {
  json!({"kind": "group", "id": group.id, "name": group.name, "members": members})
}

pub async fn list(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
  me(&app, &id, &headers).await?;
  let sessions = sessions(&app)?;
  let groups = app.management.groups()?;
  let summaries: Vec<Value> = sessions
    .iter()
    .map(|(descriptor, status)| {
      let own: Vec<&GroupRecord> =
        groups.iter().filter(|group| group.members.contains(&descriptor.id)).collect();
      session_summary(&id, descriptor, status, &own)
    })
    .collect();
  let by_id: BTreeMap<&str, &Descriptor> =
    sessions.iter().map(|(descriptor, _)| (descriptor.id.as_str(), descriptor)).collect();
  let groups: Vec<Value> = groups
    .iter()
    .map(|group| {
      let members = group.members.iter().filter_map(|id| by_id.get(id.as_str()));
      group_summary(group, members.map(|descriptor| named(descriptor)).collect())
    })
    .collect();
  Ok(Json(json!({"me": id, "sessions": summaries, "groups": groups})))
}

/// How many of a group's messages `show` gives.
const RECENT: usize = 20;
pub async fn show(
  State(app): State<Arc<App>>,
  Path((id, target)): Path<(String, String)>,
  headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
  me(&app, &id, &headers).await?;
  match resolve(&app, &target).await? {
    Target::Group(group) => {
      let mut summary = group_summary(&group, named_ids(&app, &group.members));
      let mut messages = app.management.group_messages(&group.id, None, RECENT, "")?;
      messages.truncate(RECENT);
      messages.reverse();
      summary["messages"] = json!(messages);
      Ok(Json(summary))
    }
    Target::Session(slot) => {
      let descriptor = slot.get_descriptor();
      let status = serde_json::to_value(slot.get_status()).map_err(ApiError::internal)?;
      let groups = app.management.groups_of(&descriptor.id)?;
      Ok(Json(session_summary(&id, &descriptor, &status, &groups.iter().collect::<Vec<_>>())))
    }
  }
}

#[derive(Deserialize)]
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
pub async fn create(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  headers: HeaderMap,
  Json(input): Json<NewSession>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
  let me = me(&app, &id, &headers).await?;
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
    return Err(ApiError::bad_request("no default model is configured; give --model"));
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
      created_by: Some(mine.id.clone()),
      // Beside the session that makes it, as a file is made in the directory one works in.
      folder: app.management.folder_of(&mine.id)?,
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
  let status = serde_json::to_value(slot.get_status()).map_err(ApiError::internal)?;
  Ok((StatusCode::CREATED, Json(session_summary(&mine.id, &slot.get_descriptor(), &status, &[]))))
}

#[derive(Deserialize)]
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
/// the last given to it as a message as well - or renames a group it is in. A session's changes go
/// as the user's do: a name at any time, a model at the next boundary when it runs, and
/// instructions only while it is idle.
pub async fn configure(
  State(app): State<Arc<App>>,
  Path((id, target)): Path<(String, String)>,
  headers: HeaderMap,
  Json(changes): Json<Changes>,
) -> Result<Json<Value>, ApiError> {
  let me = me(&app, &id, &headers).await?;
  let slot = match resolve(&app, &target).await? {
    Target::Group(group) => return rename_group(&app, &id, &group, changes),
    Target::Session(slot) => slot,
  };
  if !may_manage(&me.get_descriptor(), &slot.get_descriptor()) {
    return Err(forbidden("only a session you made, or yourself, can be configured"));
  }
  let status = slot.get_status().status;
  if changes.instructions.is_some() && status.running {
    return Err(ApiError::conflict(
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
    input.insert("config".into(), serde_json::to_value(config).map_err(ApiError::internal)?);
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
  if !input.is_empty() {
    shown = Some(manage::update(app.clone(), slot.clone(), None, Value::Object(input)).await?);
  }
  if let Some(name) = changes.name {
    shown = Some(manage::update(app.clone(), slot.clone(), None, json!({"name": name})).await?);
  }
  if let Some(instructions) = changes.instructions.filter(|text| !text.is_empty()) {
    let message = Message::System {
      metadata: json!({"source": "instructions_changed"}),
      content: vec![ContentBlock::Text { text: instructions }],
    };
    slot.enqueue(message).await?;
  }
  shown.ok_or_else(|| ApiError::bad_request("nothing to change"))
}
/// Renames a group the asking session is in; a group has nothing else to configure.
fn rename_group(
  app: &App,
  me: &str,
  group: &GroupRecord,
  changes: Changes,
) -> Result<Json<Value>, ApiError> {
  let Changes { name: Some(name), model: None, provider: None, instructions: None } = changes
  else {
    return Err(ApiError::bad_request("a group has only its name to change"));
  };
  if !group.members.iter().any(|member| member == me) {
    return Err(forbidden("not a member of this group"));
  }
  let group = app.update_group(&group.id, Some(name), None)?;
  Ok(Json(group_summary(&group, named_ids(app, &group.members))))
}

pub async fn delete(
  State(app): State<Arc<App>>,
  Path((id, target)): Path<(String, String)>,
  headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
  let me = me(&app, &id, &headers).await?;
  let slot = resolve_session(&app, &target).await?;
  let descriptor = slot.get_descriptor();
  if descriptor.id == id {
    return Err(forbidden("a session cannot delete itself"));
  }
  if !may_manage(&me.get_descriptor(), &descriptor) {
    return Err(forbidden("only a session you made can be deleted"));
  }
  manage::delete(&app, &descriptor.id).await?;
  Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Send {
  text: String,
}
/// Sends a message as the asking session: to a group it is in, or to a session, in the group of
/// the two of them and the user. It wakes the other members, as any post does.
pub async fn send(
  State(app): State<Arc<App>>,
  Path((id, target)): Path<(String, String)>,
  headers: HeaderMap,
  Json(input): Json<Send>,
) -> Result<Json<Value>, ApiError> {
  let me = me(&app, &id, &headers).await?;
  let mine = me.get_descriptor();
  if input.text.trim().is_empty() {
    return Err(ApiError::bad_request("nothing to send"));
  }
  let group = match resolve(&app, &target).await? {
    Target::Group(group) => group,
    Target::Session(slot) if slot.get_descriptor().id == mine.id => {
      return Err(ApiError::bad_request("that is you"));
    }
    Target::Session(slot) => app.group_of_two(&mine, &slot.get_descriptor()).await?,
  };
  if !group.members.contains(&mine.id) {
    return Err(forbidden("not a member of this group; `wish session group add` joins it"));
  }
  let author = Author::Session { id: mine.id.clone(), name: display_name(&mine) };
  let posted = app.post(&group, author, input.text.clone(), None).await?;
  groups::note_sent(&me, &group, &posted, &input.text).await?;
  Ok(Json(json!({
    "group": {"id": group.id, "name": group.name},
    "seq": posted.message.seq,
    "woken": named_ids(&app, &posted.woken),
  })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewGroup {
  #[serde(default)]
  name: String,
  /// Sessions by name or id; the asking one is a member too.
  members: Vec<String>,
}
pub async fn create_group(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  headers: HeaderMap,
  Json(input): Json<NewGroup>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
  me(&app, &id, &headers).await?;
  let mut members = vec![id.clone()];
  for member in &input.members {
    members.push(resolve_session(&app, member).await?.get_descriptor().id);
  }
  let folder = app.management.folder_of(&id)?;
  let group = app.create_group(input.name, members, Some(id), folder).await?;
  Ok((StatusCode::CREATED, Json(group_summary(&group, named_ids(&app, &group.members)))))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AddMembers {
  add: Vec<String>,
}
/// A member of a group brings more sessions into it.
pub async fn add_members(
  State(app): State<Arc<App>>,
  Path((id, target)): Path<(String, String)>,
  headers: HeaderMap,
  Json(input): Json<AddMembers>,
) -> Result<Json<Value>, ApiError> {
  me(&app, &id, &headers).await?;
  let group = resolve_group(&app, &target).await?;
  if !group.members.contains(&id) {
    return Err(forbidden("not a member of this group"));
  }
  let mut members = group.members.clone();
  for member in &input.add {
    members.push(resolve_session(&app, member).await?.get_descriptor().id);
  }
  let group = app.update_group(&group.id, None, Some(members))?;
  Ok(Json(group_summary(&group, named_ids(&app, &group.members))))
}
pub async fn leave(
  State(app): State<Arc<App>>,
  Path((id, target)): Path<(String, String)>,
  headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
  me(&app, &id, &headers).await?;
  let group = resolve_group(&app, &target).await?;
  let members = group.members.iter().filter(|member| **member != id).cloned().collect();
  app.update_group(&group.id, None, Some(members))?;
  Ok(StatusCode::NO_CONTENT)
}

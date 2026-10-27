//! Group membership, transcripts and user commands. Attachments are managed separately.
use super::input::Input;
use crate::application::error::ApplicationError;
use crate::application::{app::App, groups::Author};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewGroup {
  #[serde(default)]
  name: String,
  /// The sessions in it, besides the user.
  members: Vec<String>,
  /// The folder it goes in; none for the root of the list.
  #[serde(default)]
  folder: Option<String>,
}
pub async fn create(app: Arc<App>, input: NewGroup) -> Result<Value, ApplicationError> {
  let group = app.create_group(input.name, input.members, None, input.folder).await?;
  Ok(json!(group))
}
pub async fn get(app: Arc<App>, id: String) -> Result<Value, ApplicationError> {
  super::error::blocking(move || Ok(json!(app.get_group(&id)?))).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Changes {
  name: Option<String>,
  members: Option<Vec<String>>,
}
pub async fn update(
  app: Arc<App>,
  id: String,
  changes: Changes,
) -> Result<Value, ApplicationError> {
  super::error::blocking(move || Ok(json!(app.update_group(&id, changes.name, changes.members)?)))
    .await
}
pub async fn delete(app: Arc<App>, id: String) -> Result<(), ApplicationError> {
  app.delete_group(&id).await?;
  Ok(())
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Page {
  /// Messages before this one; the newest when absent.
  before: Option<u64>,
  limit: usize,
  /// Words a message's text must hold.
  query: String,
}
impl Default for Page {
  fn default() -> Self {
    Self { before: None, limit: 50, query: String::new() }
  }
}
/// A page of a group's transcript, newest first; `next` continues it.
pub async fn messages(app: Arc<App>, id: String, page: Page) -> Result<Value, ApplicationError> {
  super::error::blocking(move || {
    app.get_group(&id)?;
    super::pagination::validate_limit(page.limit, 1000)?;
    let mut items = app.management.group_messages(&id, page.before, page.limit, &page.query)?;
    let more = items.len() > page.limit;
    items.truncate(page.limit);
    let next = more.then(|| items.last().map(|message| message.seq)).flatten();
    Ok(json!({"items": items, "next": next}))
  })
  .await
}
/// The user's post: text, with images and files uploaded to the group first.
pub async fn post(app: Arc<App>, id: String, input: Input) -> Result<Value, ApplicationError> {
  let owner = app.clone();
  let group = super::error::blocking(move || owner.get_group(&id)).await?;
  let posted = app.post(&group, Author::User, input.text.clone(), Some(input)).await?;
  Ok(json!({"message": posted.message, "woken": posted.woken}))
}

/// The groups a session is in.
pub async fn of_session(app: Arc<App>, id: String) -> Result<Value, ApplicationError> {
  super::error::blocking(move || {
    if !app.management.exists(&id)? {
      return Err(ApplicationError::not_found());
    }
    Ok(json!(app.management.groups_of(&id)?))
  })
  .await
}

//! Groups: chats among the user and several sessions, like a chat app's group chats. A group is
//! kept in the management index (`management::groups`) - its record, its members and its own
//! transcript - and runs nothing: its members are sessions, each with its own memory.
//!
//! A session takes part as a person would. A post goes to the transcript and to every other
//! member's queue as a user message headed by the group and the author, which wakes it; what it
//! answers there stays in its own conversation, and it speaks in the group only by sending, with
//! `wish session send` in its shell (`http::peers::send`), as often as it has something to say. A
//! message to a session is a post in the group of the two and the user, made when needed
//! (`App::group_of_two`).
pub use crate::server::management::{Author, GroupMessage, GroupRecord};

use crate::server::{
  app::App,
  error::{ApiError, blocking},
  http::content::input::{self, Input},
  management::NewGroupMessage,
  session::{Descriptor, SessionSlot},
};
use crate::{
  protocol::{ContentBlock, Message},
  utils::time::Timestamp,
};
use serde_json::{Value, json};
use std::sync::Arc;

/// What a post came to: the message as the transcript keeps it, and the members it woke.
pub struct Posted {
  pub message: GroupMessage,
  pub woken: Vec<String>,
}

impl App {
  /// Makes a group of `members`, existing sessions, and the user, in `folder` (none for the root).
  pub async fn create_group(
    &self,
    name: String,
    members: Vec<String>,
    created_by: Option<String>,
    folder: Option<String>,
  ) -> Result<GroupRecord, ApiError> {
    self.lifecycle.require_open()?;
    let now = Timestamp::now().0;
    let group = GroupRecord {
      id: uuid::Uuid::new_v4().to_string(),
      name,
      members: self.check_members(members)?,
      created_by,
      created_at: now,
      updated_at: now,
    };
    self.management.create_group(&group, folder.as_deref())?;
    self.announce_group(&group.id);
    Ok(group)
  }
  pub fn get_group(&self, id: &str) -> Result<GroupRecord, ApiError> {
    self.management.group(id)?.ok_or_else(ApiError::not_found)
  }
  /// Renames a group or replaces its members.
  pub fn update_group(
    &self,
    id: &str,
    name: Option<String>,
    members: Option<Vec<String>>,
  ) -> Result<GroupRecord, ApiError> {
    self.get_group(id)?;
    let now = Timestamp::now().0;
    if let Some(name) = name {
      self.management.rename_group(id, &name, now)?;
    }
    if let Some(members) = members {
      self.management.set_members(id, &self.check_members(members)?, now)?;
    }
    self.announce_group(id);
    self.get_group(id)
  }
  /// Deletes a group: its transcript and the files posted to it. Its members stay.
  pub async fn delete_group(&self, id: &str) -> Result<(), ApiError> {
    self.get_group(id)?;
    self.management.delete_group(id)?;
    match tokio::fs::remove_dir_all(self.data_dir.blobs(id)).await {
      Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
        return Err(ApiError::internal(error));
      }
      _ => {}
    }
    let _ = self.events.send(json!({"type":"group_deleted","id":id}));
    Ok(())
  }
  /// Takes a deleted session out of every group it was in.
  pub fn leave_groups(&self, session: &str) -> Result<(), ApiError> {
    for group in self.management.leave_groups(session)? {
      self.announce_group(&group);
    }
    Ok(())
  }
  /// The group of two sessions and the user, for a message from one to the other: the one they
  /// have, or a new one named after them.
  pub async fn group_of_two(
    &self,
    from: &Descriptor,
    to: &Descriptor,
  ) -> Result<GroupRecord, ApiError> {
    if let Some(id) = self.management.group_of_two(&from.id, &to.id)? {
      return self.get_group(&id);
    }
    let name = format!("{} & {}", display_name(from), display_name(to));
    let folder = self.management.folder_of(&from.id)?;
    self
      .create_group(name, vec![from.id.clone(), to.id.clone()], Some(from.id.clone()), folder)
      .await
  }
  /// The members as they will be kept: each an existing session, once.
  fn check_members(&self, members: Vec<String>) -> Result<Vec<String>, ApiError> {
    let mut checked: Vec<String> = Vec::new();
    for member in members {
      if !self.management.exists(&member)? {
        return Err(ApiError::bad_request(format!("no session `{member}`")));
      }
      if !checked.contains(&member) {
        checked.push(member);
      }
    }
    Ok(checked)
  }
  fn announce_group(&self, id: &str) {
    let _ = self.events.send(json!({"type":"group_changed","id":id}));
  }

  /// Posts to a group: the transcript takes the message, and every other member is told it and
  /// runs. `attached` is the user's input when it carries images or files, uploaded to the group's
  /// files.
  pub async fn post(
    self: &Arc<Self>,
    group: &GroupRecord,
    author: Author,
    text: String,
    attached: Option<Input>,
  ) -> Result<Posted, ApiError> {
    self.lifecycle.require_open()?;
    let author_id = match &author {
      Author::Session { id, .. } => Some(id.clone()),
      _ => None,
    };
    if author_id.as_ref().is_some_and(|id| !group.members.contains(id)) {
      return Err(ApiError::bad_request("not a member of this group"));
    }
    let mut members = Vec::new();
    for id in group.members.iter().filter(|id| Some(*id) != author_id.as_ref()) {
      // A member deleted meanwhile is simply not there any more.
      if let Ok(slot) = self.get_session(id).await {
        members.push(slot);
      }
    }
    // What each member is told: a line saying where and from whom, then the message. Files stay
    // the group's: the members' copies name them where the group keeps them. It is made first, so
    // an input that cannot be read is turned away before the transcript takes it.
    let heading = format!("[Group \"{}\" · from {}]\n", group.name, describe(&author));
    let attached = attached.filter(|input| !input.attachments.is_empty());
    let (content, text, attachments) = match attached {
      Some(input) => {
        let (text, attachments) = (without_placeholders(&input), attachment_list(&input));
        let Message::User { content: posted, .. } =
          input::message(&self.data_dir.blobs(&group.id), input).await?
        else {
          unreachable!("an input makes a user message")
        };
        let content = std::iter::once(ContentBlock::Text { text: heading }).chain(posted).collect();
        (content, text, attachments)
      }
      None => (vec![ContentBlock::Text { text: format!("{heading}{text}") }], text, Value::Null),
    };
    let now = Timestamp::now().0;
    let message = NewGroupMessage { author: author.clone(), text, attachments };
    let message = self.management.append_group_message(&group.id, message, now)?;
    self.announce_group(&group.id);
    let told = json!({
      "source": "group",
      "group": {"id": group.id, "name": group.name},
      "from": author,
    });
    let mut woken = Vec::new();
    for member in &members {
      member.enqueue(Message::User { metadata: told.clone(), content: content.clone() }).await?;
      member.touch()?;
      member.schedule(self);
      woken.push(member.get_descriptor().id);
    }
    Ok(Posted { message, woken })
  }
}

/// The name a session is shown by: its name, or the start of its id.
pub fn display_name(descriptor: &Descriptor) -> String {
  if descriptor.name.is_empty() {
    descriptor.id.chars().take(8).collect()
  } else {
    descriptor.name.clone()
  }
}

/// How a member is told who wrote.
fn describe(author: &Author) -> String {
  match author {
    Author::User => "the user".into(),
    Author::Session { name, .. } => name.clone(),
  }
}

/// The input's words, without the editor's placeholders for its attachments.
fn without_placeholders(input: &Input) -> String {
  let mut text = input.text.clone();
  for placeholder in input.attachments.iter().filter_map(|a| a.placeholder.as_deref()) {
    text = text.replace(placeholder, "");
  }
  text.trim().to_owned()
}
/// What a group's transcript keeps of an input's attachments: `[{"id", "kind", "name"}]`.
fn attachment_list(input: &Input) -> Value {
  input
    .attachments
    .iter()
    .map(
      |attachment| json!({"id": attachment.id, "kind": attachment.kind, "name": attachment.name}),
    )
    .collect()
}

/// Records in a session's history that it sent `text` to a group, so its page shows what it said
/// where.
pub async fn note_sent(
  slot: &Arc<SessionSlot>,
  group: &GroupRecord,
  posted: &Posted,
  text: &str,
) -> Result<(), ApiError> {
  let note = json!({
    "type": "group_message_sent",
    "group": {"id": group.id, "name": group.name},
    "text": text,
    "woken": posted.woken,
  });
  let sender = slot.sender.clone();
  blocking(move || Ok(sender.record_application_event(note)?)).await?;
  let _ = slot.events.send(json!({"type":"history_changed"}));
  Ok(())
}

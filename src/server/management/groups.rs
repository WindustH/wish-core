//! Groups: their records, members and messages. A group's messages are its own transcript, kept
//! here once; each member's copy is a message in that session's own history.
use super::ManagementStore;
use crate::server::error::ApiError;
use rusqlite::{OptionalExtension, Params, Row, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// A group as the API shows it.
#[derive(Clone, Debug, Serialize)]
pub struct GroupRecord {
  pub id: String,
  pub name: String,
  /// Its sessions, in the order they joined; the user is in every group.
  pub members: Vec<String>,
  /// The session that made it with `wish session`; absent for one the user made.
  pub created_by: Option<String>,
  pub created_at: u64,
  pub updated_at: u64,
}

/// Who wrote a message.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Author {
  User,
  /// A member, with the name it had then.
  Session {
    id: String,
    name: String,
  },
}

/// A message of a group's transcript.
#[derive(Clone, Debug, Serialize)]
pub struct GroupMessage {
  pub seq: u64,
  pub at: u64,
  pub author: Author,
  pub text: String,
  /// Images and files the user attached: `[{"id", "kind", "name"}]`, in the group's files.
  #[serde(skip_serializing_if = "Value::is_null")]
  pub attachments: Value,
}

/// A message to append: what [`GroupMessage`] holds but its place and time.
pub struct NewGroupMessage {
  pub author: Author,
  pub text: String,
  pub attachments: Value,
}

impl ManagementStore {
  /// Keeps a new group, in `folder` (none for the root of the list).
  pub fn create_group(&self, group: &GroupRecord, folder: Option<&str>) -> Result<(), ApiError> {
    let mut db = self.db.lock().unwrap();
    let tx = db.transaction()?;
    if let Some(folder) = folder {
      super::folders::require_folder(&tx, folder)?;
    }
    tx.execute(
      "INSERT INTO groups(id,name,created_by,created_at,updated_at,folder) VALUES(?1,?2,?3,?4,?5,?6)",
      params![
        group.id,
        group.name,
        group.created_by,
        group.created_at as i64,
        group.updated_at as i64,
        folder
      ],
    )?;
    insert_members(&tx, &group.id, &group.members)?;
    tx.commit()?;
    Ok(())
  }
  pub fn group(&self, id: &str) -> Result<Option<GroupRecord>, ApiError> {
    Ok(self.groups_where("WHERE id=?1", [id])?.pop())
  }
  /// Every group, newest first.
  pub fn groups(&self) -> Result<Vec<GroupRecord>, ApiError> {
    self.groups_where("", [])
  }
  /// The groups `session` is in, newest first.
  pub fn groups_of(&self, session: &str) -> Result<Vec<GroupRecord>, ApiError> {
    self.groups_where(
      "WHERE id IN (SELECT group_id FROM group_members WHERE session_id=?1)",
      [session],
    )
  }
  fn groups_where(&self, filter: &str, params: impl Params) -> Result<Vec<GroupRecord>, ApiError> {
    let db = self.db.lock().unwrap();
    let mut statement = db.prepare(&format!(
      "SELECT id,name,created_by,created_at,updated_at FROM groups {filter} ORDER BY updated_at DESC, id DESC"
    ))?;
    let mut groups: Vec<GroupRecord> =
      statement.query_map(params, group_row)?.collect::<Result<_, _>>()?;
    for group in &mut groups {
      group.members = members(&db, &group.id)?;
    }
    Ok(groups)
  }
  /// The group of exactly these two sessions, when there is one.
  pub fn group_of_two(&self, a: &str, b: &str) -> Result<Option<String>, ApiError> {
    Ok(
      self
        .db
        .lock()
        .unwrap()
        .query_row(
          "SELECT group_id FROM group_members WHERE session_id IN (?1,?2) GROUP BY group_id \
           HAVING count(*)=2 AND (SELECT count(*) FROM group_members m WHERE m.group_id=group_members.group_id)=2 LIMIT 1",
          [a, b],
          |row| row.get(0),
        )
        .optional()?,
    )
  }
  pub fn rename_group(&self, id: &str, name: &str, at: u64) -> Result<(), ApiError> {
    self.db.lock().unwrap().execute(
      "UPDATE groups SET name=?2,updated_at=?3 WHERE id=?1",
      params![id, name, at as i64],
    )?;
    Ok(())
  }
  pub fn set_members(&self, id: &str, members: &[String], at: u64) -> Result<(), ApiError> {
    let mut db = self.db.lock().unwrap();
    let tx = db.transaction()?;
    tx.execute("DELETE FROM group_members WHERE group_id=?1", [id])?;
    insert_members(&tx, id, members)?;
    tx.execute("UPDATE groups SET updated_at=?2 WHERE id=?1", params![id, at as i64])?;
    tx.commit()?;
    Ok(())
  }
  /// Takes a session out of every group; returns the groups it was in.
  pub fn leave_groups(&self, session: &str) -> Result<Vec<String>, ApiError> {
    let mut db = self.db.lock().unwrap();
    let tx = db.transaction()?;
    let groups: Vec<String> = {
      let mut statement = tx.prepare("SELECT group_id FROM group_members WHERE session_id=?1")?;
      statement.query_map([session], |row| row.get(0))?.collect::<Result<_, _>>()?
    };
    tx.execute("DELETE FROM group_members WHERE session_id=?1", [session])?;
    tx.commit()?;
    Ok(groups)
  }
  pub fn delete_group(&self, id: &str) -> Result<(), ApiError> {
    let mut db = self.db.lock().unwrap();
    let tx = db.transaction()?;
    for table in ["group_messages", "group_members"] {
      tx.execute(&format!("DELETE FROM {table} WHERE group_id=?1"), [id])?;
    }
    tx.execute("DELETE FROM groups WHERE id=?1", [id])?;
    tx.commit()?;
    super::shrink(&db)?;
    Ok(())
  }

  /// Appends a message to a group's transcript; returns the message as kept.
  pub fn append_group_message(
    &self,
    group: &str,
    message: NewGroupMessage,
    at: u64,
  ) -> Result<GroupMessage, ApiError> {
    let NewGroupMessage { author, text, attachments } = message;
    let mut db = self.db.lock().unwrap();
    let tx = db.transaction()?;
    let seq: u64 = tx.query_row(
      "SELECT coalesce(max(seq)+1,0) FROM group_messages WHERE group_id=?1",
      [group],
      |row| row.get::<_, i64>(0),
    )? as u64;
    let (kind, author_id, author_name) = match &author {
      Author::User => ("user", None, None),
      Author::Session { id, name } => ("session", Some(id.as_str()), Some(name.as_str())),
    };
    tx.execute(
      "INSERT INTO group_messages(group_id,seq,at,author_kind,author_id,author_name,text,attachments) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
      params![
        group,
        seq as i64,
        at as i64,
        kind,
        author_id,
        author_name,
        text,
        (!attachments.is_null()).then(|| attachments.to_string())
      ],
    )?;
    tx.execute("UPDATE groups SET updated_at=?2 WHERE id=?1", params![group, at as i64])?;
    tx.commit()?;
    Ok(GroupMessage { seq, at, author, text, attachments })
  }
  /// A page of a group's messages, newest first: those before `before`, with `query` in their
  /// text when given. The page holds one more than `limit` when there are more.
  /// Each group's transcript: its messages and the bytes their stored values take.
  pub fn transcript_sizes(&self) -> Result<HashMap<String, (u64, u64)>, ApiError> {
    let db = self.db.lock().unwrap();
    let mut statement = db.prepare(
      "SELECT group_id, count(*), sum(length(CAST(text AS BLOB)) + ifnull(length(author_id),0) + \
       ifnull(length(CAST(author_name AS BLOB)),0) + ifnull(length(attachments),0)) \
       FROM group_messages GROUP BY group_id",
    )?;
    let rows = statement.query_map([], |row| {
      Ok((row.get::<_, String>(0)?, (row.get::<_, i64>(1)? as u64, row.get::<_, i64>(2)? as u64)))
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
  }
  pub fn group_messages(
    &self,
    group: &str,
    before: Option<u64>,
    limit: usize,
    query: &str,
  ) -> Result<Vec<GroupMessage>, ApiError> {
    let db = self.db.lock().unwrap();
    let mut statement = db.prepare(
      "SELECT seq,at,author_kind,author_id,author_name,text,attachments FROM group_messages \
       WHERE group_id=?1 AND (?2 IS NULL OR seq<?2) AND (?3='' OR instr(lower(text),lower(?3))>0) \
       ORDER BY seq DESC LIMIT ?4",
    )?;
    let limit = i64::try_from(limit.saturating_add(1)).map_err(ApiError::internal)?;
    let rows = statement
      .query_map(params![group, before.map(|seq| seq as i64), query, limit], group_message)?;
    Ok(rows.collect::<Result<_, _>>()?)
  }
}

fn insert_members(tx: &Transaction, group: &str, members: &[String]) -> rusqlite::Result<()> {
  let mut insert =
    tx.prepare("INSERT INTO group_members(group_id,session_id,position) VALUES(?1,?2,?3)")?;
  for (position, member) in members.iter().enumerate() {
    insert.execute(params![group, member, position as i64])?;
  }
  Ok(())
}
fn members(db: &rusqlite::Connection, group: &str) -> rusqlite::Result<Vec<String>> {
  let mut statement =
    db.prepare("SELECT session_id FROM group_members WHERE group_id=?1 ORDER BY position")?;
  statement.query_map([group], |row| row.get(0))?.collect()
}
fn group_row(row: &Row) -> rusqlite::Result<GroupRecord> {
  Ok(GroupRecord {
    id: row.get(0)?,
    name: row.get(1)?,
    members: Vec::new(),
    created_by: row.get(2)?,
    created_at: row.get::<_, i64>(3)? as u64,
    updated_at: row.get::<_, i64>(4)? as u64,
  })
}
fn group_message(row: &Row) -> rusqlite::Result<GroupMessage> {
  let author = match row.get::<_, String>(2)?.as_str() {
    "session" => Author::Session { id: row.get(3)?, name: row.get(4)? },
    _ => Author::User,
  };
  let attachments: Option<String> = row.get(6)?;
  Ok(GroupMessage {
    seq: row.get::<_, i64>(0)? as u64,
    at: row.get::<_, i64>(1)? as u64,
    author,
    text: row.get(5)?,
    attachments: attachments.and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default(),
  })
}

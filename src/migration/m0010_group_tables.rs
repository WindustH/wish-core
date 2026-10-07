//! 9 to 10: a group is no longer a session that never runs but a record of its own. Groups, their
//! members and their transcripts get tables in `management.sqlite`, and each group kept as a
//! session moves there - its record from the session list, its messages from its engine session's
//! entries - and that session is deleted from `wish.sqlite`. Session records lose `session.kind`
//! and `session.members`. The files posted to a group stay where they are, `blobs/<group id>`.

use super::Data;
use rusqlite::{Connection, params};
use serde_json::{Value, json};

pub const SUMMARY: &str = "keep groups in tables of their own instead of sessions that never run";

const TABLES: &str = "
  CREATE TABLE IF NOT EXISTS groups(id TEXT PRIMARY KEY, name TEXT NOT NULL, created_by TEXT, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL);
  CREATE INDEX IF NOT EXISTS groups_updated ON groups(updated_at DESC,id);
  CREATE TABLE IF NOT EXISTS group_members(group_id TEXT NOT NULL, session_id TEXT NOT NULL, position INTEGER NOT NULL, PRIMARY KEY(group_id,session_id)) WITHOUT ROWID;
  CREATE INDEX IF NOT EXISTS group_members_session ON group_members(session_id);
  CREATE TABLE IF NOT EXISTS group_messages(group_id TEXT NOT NULL, seq INTEGER NOT NULL, at INTEGER NOT NULL, author_kind TEXT NOT NULL, author_id TEXT, author_name TEXT, text TEXT NOT NULL, attachments TEXT, PRIMARY KEY(group_id,seq)) WITHOUT ROWID;
";

pub fn apply(data: &mut Data) -> Result<(), String> {
  let Some(management) = data.management else { return Ok(()) };
  let failed = |error: rusqlite::Error| error.to_string();
  management.execute_batch(TABLES).map_err(failed)?;
  let records = management
    .prepare("SELECT id, record FROM sessions")
    .and_then(|mut statement| {
      statement
        .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?
        .collect::<Result<Vec<_>, _>>()
    })
    .map_err(failed)?;
  for (id, record) in records {
    let mut value: Value =
      serde_json::from_str(&record).map_err(|error| format!("session {id}: {error}"))?;
    let session = &mut value["session"];
    let kind = session.as_object_mut().and_then(|session| session.remove("kind"));
    let members = session.as_object_mut().and_then(|session| session.remove("members"));
    if kind.as_ref().and_then(Value::as_str) == Some("group") {
      move_group(management, data.wish, &id, &value["session"], members.unwrap_or_default())
        .map_err(|error| format!("group {id}: {error}"))?;
      continue;
    }
    if kind.is_none() && members.is_none() {
      continue;
    }
    management
      .execute("UPDATE sessions SET record = ?1 WHERE id = ?2", [&value.to_string(), &id])
      .map_err(|error| format!("session {id}: {error}"))?;
  }
  Ok(())
}

/// Moves a group kept as a session to the group tables, and deletes that session.
fn move_group(
  management: &Connection,
  wish: Option<&Connection>,
  id: &str,
  record: &Value,
  members: Value,
) -> Result<(), String> {
  let failed = |error: rusqlite::Error| error.to_string();
  management
    .execute(
      "INSERT OR IGNORE INTO groups(id,name,created_by,created_at,updated_at) VALUES(?1,?2,?3,?4,?5)",
      params![
        id,
        record["name"].as_str().unwrap_or_default(),
        record["created_by"].as_str(),
        record["created_at"].as_i64().unwrap_or(0),
        record["updated_at"].as_i64().unwrap_or(0)
      ],
    )
    .map_err(failed)?;
  for (position, member) in members.as_array().into_iter().flatten().enumerate() {
    management
      .execute(
        "INSERT OR IGNORE INTO group_members(group_id,session_id,position) VALUES(?1,?2,?3)",
        params![id, member.as_str(), position as i64],
      )
      .map_err(failed)?;
  }
  if let Some(wish) = wish {
    let key = format!("session/{}", hex::encode(id.as_bytes()));
    let entries = wish
      .prepare(
        "SELECT item.value FROM wish_items item JOIN wish_list_keys list ON list.id = item.list \
         WHERE list.name = ?1 ORDER BY item.position",
      )
      .and_then(|mut statement| {
        statement
          .query_map([format!("{key}/entries")], |row| row.get::<_, Vec<u8>>(0))?
          .collect::<Result<Vec<_>, _>>()
      })
      .map_err(failed)?;
    let mut seq = 0i64;
    for entry in entries {
      let entry: Value = serde_json::from_slice(&entry).map_err(|error| error.to_string())?;
      let Some(message) = entry["message"].get("User") else { continue };
      let metadata = &message["metadata"];
      let author = &metadata["author"];
      // Notes Wish wrote into a group stay behind, like the relay limit they told of (format 11).
      let Some(kind) = author["kind"].as_str().filter(|kind| *kind != "wish") else { continue };
      let text: Vec<&str> = message["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|block| block["Text"]["text"].as_str())
        .filter(|text| !text.starts_with("[File sha256:"))
        .collect();
      let attachments =
        metadata["attachments"].as_array().filter(|list| !list.is_empty()).map(|list| {
          Value::Array(
            list
              .iter()
              .map(|a| json!({"id": a["id"], "kind": a["kind"], "name": a["name"]}))
              .collect(),
          )
          .to_string()
        });
      management
        .execute(
          "INSERT OR IGNORE INTO group_messages(group_id,seq,at,author_kind,author_id,author_name,text,attachments) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
          params![
            id,
            seq,
            entry["recorded_at"].as_i64().unwrap_or(0),
            kind,
            author["id"].as_str(),
            author["name"].as_str(),
            text.join("").trim(),
            attachments
          ],
        )
        .map_err(failed)?;
      seq += 1;
    }
    // As deleting a session does: its history rows (the search index follows by trigger), its
    // lists' items, the lists, and its objects.
    let prefix = format!("{key}/");
    for sql in [
      "DELETE FROM wish_history_index WHERE list IN (SELECT id FROM wish_list_keys WHERE substr(name,1,length(?1))=?1)",
      "DELETE FROM wish_items WHERE list IN (SELECT id FROM wish_list_keys WHERE substr(name,1,length(?1))=?1)",
      "DELETE FROM wish_list_keys WHERE substr(name,1,length(?1))=?1",
      "DELETE FROM wish_lists WHERE substr(name,1,length(?1))=?1",
      "DELETE FROM wish_objects WHERE substr(name,1,length(?1))=?1",
    ] {
      wish.execute(sql, [&prefix]).map_err(failed)?;
    }
    wish.execute("DELETE FROM wish_objects WHERE name=?1", [&key]).map_err(failed)?;
  }
  management.execute("DELETE FROM sessions WHERE id = ?1", [id]).map_err(failed)?;
  Ok(())
}

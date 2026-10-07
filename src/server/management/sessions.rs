//! The session list's records: one per session, `{"session": descriptor, "status": status}`, as
//! `SessionSlot::persist_index` saves it; the list's filters read JSON paths out of it. The list a
//! user browses - folders, sessions and groups, arranged as `folders` tells - is `conversations`.
use super::{ManagementStore, folders::folder_size};
use crate::server::{error::ApiError, session::preview_pending_selection};
use rusqlite::{OptionalExtension, params};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SessionQuery {
  pub start: usize,
  pub limit: usize,
  pub query: String,
  pub phase: String,
  pub tag: String,
  pub order: String,
  /// The folder whose listing `conversations` gives; none for the root.
  pub folder: Option<String>,
  /// Every session and group, wherever it is, instead of a folder's listing.
  pub flat: bool,
}
impl Default for SessionQuery {
  fn default() -> Self {
    Self {
      start: 0,
      limit: 50,
      query: String::new(),
      phase: String::new(),
      tag: String::new(),
      order: "desc".into(),
      folder: None,
      flat: false,
    }
  }
}
impl SessionQuery {
  fn direction(&self) -> Result<&'static str, ApiError> {
    if self.limit == 0 {
      return Err(ApiError::bad_request("limit must be positive"));
    }
    match self.order.as_str() {
      "asc" => Ok("ASC"),
      "desc" => Ok("DESC"),
      _ => Err(ApiError::bad_request("order must be asc or desc")),
    }
  }
}
/// The session records a query selects, its parameters 1 to 3: words in the name or id, a phase
/// (or `running`) and a tag.
const SESSION_FILTER: &str = "(?1='' OR instr(lower(json_extract(record,'$.session.name')),lower(?1))>0 OR instr(lower(id),lower(?1))>0) AND (?2='' OR json_extract(record,'$.status.phase')=?2 OR (?2='running' AND json_extract(record,'$.status.running')=1)) AND (?3='' OR EXISTS(SELECT 1 FROM json_each(json_extract(record,'$.status.metadata.tags')) WHERE value=?3))";

impl ManagementStore {
  pub fn save(&self, record: &Value) -> Result<(), ApiError> {
    self.db.lock().unwrap().execute(
      "INSERT INTO sessions(id,updated_at,record) VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET updated_at=excluded.updated_at,record=excluded.record",
      params![
        record["session"]["id"].as_str(),
        record["session"]["updated_at"].as_i64(),
        record.to_string()
      ],
    )?;
    Ok(())
  }
  pub fn read(&self, id: &str) -> Result<Value, ApiError> {
    self.record(id)?.ok_or_else(ApiError::not_found)
  }
  fn record(&self, id: &str) -> Result<Option<Value>, ApiError> {
    let value: Option<String> = self
      .db
      .lock()
      .unwrap()
      .query_row("SELECT record FROM sessions WHERE id=?1", [id], |r| r.get(0))
      .optional()?;
    value.map(|value| serde_json::from_str(&value).map_err(ApiError::internal)).transpose()
  }
  pub fn exists(&self, id: &str) -> Result<bool, ApiError> {
    let found = self
      .db
      .lock()
      .unwrap()
      .query_row("SELECT 1 FROM sessions WHERE id=?1", [id], |_| Ok(()))
      .optional()?;
    Ok(found.is_some())
  }
  /// A page of the session list, each session as the API shows it.
  pub fn list(&self, q: SessionQuery) -> Result<Value, ApiError> {
    let direction = q.direction()?;
    let sql = format!(
      "SELECT record FROM sessions WHERE {SESSION_FILTER} ORDER BY updated_at {direction},id {direction} LIMIT ?4 OFFSET ?5"
    );
    let db = self.db.lock().unwrap();
    let mut statement = db.prepare(&sql)?;
    let rows = statement.query_map(
      params![
        q.query,
        q.phase,
        q.tag,
        i64::try_from(q.limit.saturating_add(1)).map_err(ApiError::internal)?,
        q.start as i64
      ],
      |r| r.get::<_, String>(0),
    )?;
    let mut items = Vec::new();
    for row in rows {
      let mut record: Value = serde_json::from_str(&row?).map_err(ApiError::internal)?;
      preview_pending_selection(&mut record);
      items.push(record);
    }
    let more = items.len() > q.limit;
    items.truncate(q.limit);
    Ok(json!({"items":items,"next":more.then_some(q.start+q.limit)}))
  }
  pub fn delete(&self, id: &str) -> Result<(), ApiError> {
    self.db.lock().unwrap().execute("DELETE FROM sessions WHERE id=?1", [id])?;
    Ok(())
  }
  /// Every session's record, newest first.
  pub fn list_all(&self) -> Result<Vec<Value>, ApiError> {
    let db = self.db.lock().unwrap();
    let mut statement =
      db.prepare("SELECT record FROM sessions ORDER BY updated_at DESC, id DESC")?;
    let rows = statement.query_map([], |r| r.get::<_, String>(0))?;
    let mut records = Vec::new();
    for row in rows {
      records.push(serde_json::from_str(&row?).map_err(ApiError::internal)?);
    }
    Ok(records)
  }
  pub fn count(&self) -> Result<i64, ApiError> {
    Ok(self.db.lock().unwrap().query_row("SELECT count(*) FROM sessions", [], |r| r.get(0))?)
  }

  /// A page of what a user browses. Without words, a phase or a tag to look for, it is one
  /// folder's listing - its folders by name, then its sessions and groups by time, pinned ones first
  /// in each - read along the folder indexes, so it costs what that folder holds. With them, or
  /// `flat`, it is every session and group that matches, wherever it is, newest first. The page is chosen by
  /// these columns alone, and only its own records are read. A phase or tag selects sessions only.
  pub fn conversations(&self, q: SessionQuery) -> Result<Value, ApiError> {
    let direction = q.direction()?;
    let limit = i64::try_from(q.limit.saturating_add(1)).map_err(ApiError::internal)?;
    let searching = q.flat || !(q.query.is_empty() && q.phase.is_empty() && q.tag.is_empty());
    let rows: Vec<(String, String, Option<String>, bool)> = {
      let db = self.db.lock().unwrap();
      let read = |row: &rusqlite::Row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?));
      if searching {
        let mut statement = db.prepare(&format!(
          "SELECT 'session',id,folder,pinned,updated_at FROM sessions WHERE {SESSION_FILTER} \
           UNION ALL SELECT 'group',id,folder,pinned,updated_at FROM groups WHERE ?2='' AND ?3='' \
           AND (?1='' OR instr(lower(name),lower(?1))>0 OR instr(lower(id),lower(?1))>0) \
           ORDER BY 5 {direction},2 {direction} LIMIT ?4 OFFSET ?5"
        ))?;
        statement
          .query_map(params![q.query, q.phase, q.tag, limit, q.start as i64], read)?
          .collect::<Result<_, _>>()?
      } else {
        let mut statement = db.prepare(&format!(
          "SELECT kind,id,parent,pinned FROM (\
           SELECT 0 AS rank,'folder' AS kind,id,parent,pinned,name,0 AS updated_at FROM folders WHERE parent IS ?1 \
           UNION ALL SELECT 1,'session',id,folder,pinned,'',updated_at FROM sessions WHERE folder IS ?1 \
           UNION ALL SELECT 1,'group',id,folder,pinned,'',updated_at FROM groups WHERE folder IS ?1) \
           ORDER BY rank,pinned DESC,name COLLATE NOCASE,updated_at {direction},id LIMIT ?2 OFFSET ?3"
        ))?;
        statement
          .query_map(params![q.folder, limit, q.start as i64], read)?
          .collect::<Result<_, _>>()?
      }
    };
    // One deleted since the page was chosen is left out.
    let mut items = Vec::new();
    for (kind, id, parent, pinned) in rows {
      let mut item = match kind.as_str() {
        "folder" => {
          let Some(folder) = self.folder(&id)? else { continue };
          let size = folder_size(&self.db.lock().unwrap(), &id)?;
          let mut folder = json!(folder);
          folder["items"] = json!(size);
          json!({"kind": "folder", "folder": folder})
        }
        "group" => {
          let Some(group) = self.group(&id)? else { continue };
          json!({"kind": "group", "group": group})
        }
        _ => {
          let Some(mut record) = self.record(&id)? else { continue };
          preview_pending_selection(&mut record);
          record["kind"] = json!("session");
          record
        }
      };
      item["parent"] = json!(parent);
      item["pinned"] = json!(pinned);
      items.push(item);
    }
    let more = items.len() > q.limit;
    items.truncate(q.limit);
    Ok(json!({"items":items,"next":more.then_some(q.start+q.limit)}))
  }
}

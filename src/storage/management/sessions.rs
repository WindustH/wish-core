//! The session list's records: one per session, `{"session": descriptor, "status": status}`, as
//! `SessionSlot::persist_index` saves it; the list's filters read JSON paths out of it. The list a
//! user browses - folders, sessions and groups, arranged as `folders` tells - is `conversations`.
use super::{ManagementStore, folders::folder_size};
use crate::storage::repository_error::RepositoryError;
use rusqlite::{OptionalExtension, params};
use serde_json::Value;

pub struct SessionFilter {
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
impl SessionFilter {
  fn direction(&self) -> Result<&'static str, RepositoryError> {
    if self.limit == 0 {
      return Err(RepositoryError::invalid("limit must be positive"));
    }
    match self.order.as_str() {
      "asc" => Ok("ASC"),
      "desc" => Ok("DESC"),
      _ => Err(RepositoryError::invalid("order must be asc or desc")),
    }
  }
}
/// The session records a query selects, its parameters 1 to 3: words in the name or id, a phase
/// (or `running`) and a tag.
const SESSION_FILTER: &str = "(?1='' OR instr(lower(json_extract(record,'$.session.name')),lower(?1))>0 OR instr(lower(id),lower(?1))>0) AND (?2='' OR json_extract(record,'$.status.phase')=?2 OR (?2='running' AND json_extract(record,'$.status.running')=1)) AND (?3='' OR EXISTS(SELECT 1 FROM json_each(json_extract(record,'$.status.metadata.tags')) WHERE value=?3))";

impl ManagementStore {
  pub fn save(&self, record: &Value) -> Result<(), RepositoryError> {
    self.lock()? .execute(
      "INSERT INTO sessions(id,updated_at,record) VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET updated_at=excluded.updated_at,record=excluded.record",
      params![
        record["session"]["id"].as_str(),
        record["session"]["updated_at"].as_i64(),
        record.to_string()
      ],
    )?;
    Ok(())
  }
  pub fn read(&self, id: &str) -> Result<Value, RepositoryError> {
    self.record(id)?.ok_or_else(RepositoryError::not_found)
  }
  fn record(&self, id: &str) -> Result<Option<Value>, RepositoryError> {
    let value: Option<String> = self
      .lock()?
      .query_row("SELECT record FROM sessions WHERE id=?1", [id], |r| r.get(0))
      .optional()?;
    value.map(|value| serde_json::from_str(&value).map_err(RepositoryError::internal)).transpose()
  }
  pub fn exists(&self, id: &str) -> Result<bool, RepositoryError> {
    let found =
      self.lock()?.query_row("SELECT 1 FROM sessions WHERE id=?1", [id], |_| Ok(())).optional()?;
    Ok(found.is_some())
  }
  /// A bounded page of persisted session index records.
  pub fn list(&self, q: SessionFilter) -> Result<RecordPage<Value>, RepositoryError> {
    let direction = q.direction()?;
    let sql = format!(
      "SELECT record FROM sessions WHERE {SESSION_FILTER} ORDER BY updated_at {direction},id {direction} LIMIT ?4 OFFSET ?5"
    );
    let db = self.lock()?;
    let mut statement = db.prepare(&sql)?;
    let rows = statement.query_map(
      params![
        q.query,
        q.phase,
        q.tag,
        i64::try_from(q.limit.saturating_add(1)).map_err(RepositoryError::internal)?,
        i64::try_from(q.start).map_err(RepositoryError::internal)?
      ],
      |r| r.get::<_, String>(0),
    )?;
    let mut items = Vec::new();
    for row in rows {
      let record: Value = serde_json::from_str(&row?).map_err(RepositoryError::internal)?;
      items.push(record);
    }
    let more = items.len() > q.limit;
    items.truncate(q.limit);
    Ok(RecordPage { items, next: more.then_some(q.start.saturating_add(q.limit)) })
  }
  pub fn delete(&self, id: &str) -> Result<(), RepositoryError> {
    self.lock()?.execute("DELETE FROM sessions WHERE id=?1", [id])?;
    Ok(())
  }
  /// Every session's record, newest first.
  pub fn list_all(&self) -> Result<Vec<Value>, RepositoryError> {
    let db = self.lock()?;
    let mut statement =
      db.prepare("SELECT record FROM sessions ORDER BY updated_at DESC, id DESC")?;
    let rows = statement.query_map([], |r| r.get::<_, String>(0))?;
    let mut records = Vec::new();
    for row in rows {
      records.push(serde_json::from_str(&row?).map_err(RepositoryError::internal)?);
    }
    Ok(records)
  }
  pub fn count(&self) -> Result<i64, RepositoryError> {
    Ok(self.lock()?.query_row("SELECT count(*) FROM sessions", [], |r| r.get(0))?)
  }

  /// A page of what a user browses. Without words, a phase or a tag to look for, it is one
  /// folder's listing - its folders by name, then its sessions and groups by time, pinned ones first
  /// in each - read along the folder indexes, so it costs what that folder holds. With them, or
  /// `flat`, it is every session and group that matches, wherever it is, newest first. The page is chosen by
  /// these columns alone, and only its own records are read. A phase or tag selects sessions only.
  pub fn conversations(
    &self,
    q: SessionFilter,
  ) -> Result<RecordPage<ConversationRow>, RepositoryError> {
    let direction = q.direction()?;
    let limit = i64::try_from(q.limit.saturating_add(1)).map_err(RepositoryError::internal)?;
    let searching = q.flat || !(q.query.is_empty() && q.phase.is_empty() && q.tag.is_empty());
    let rows: Vec<(String, String, Option<String>, bool)> = {
      let db = self.lock()?;
      let read = |row: &rusqlite::Row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?));
      if searching {
        let mut statement = db.prepare(&format!(
          "SELECT 'session',id,folder,pinned,updated_at FROM sessions WHERE {SESSION_FILTER} \
           UNION ALL SELECT 'group',id,folder,pinned,updated_at FROM groups WHERE ?2='' AND ?3='' \
           AND (?1='' OR instr(lower(name),lower(?1))>0 OR instr(lower(id),lower(?1))>0) \
           ORDER BY 5 {direction},2 {direction} LIMIT ?4 OFFSET ?5"
        ))?;
        statement
          .query_map(
            params![
              q.query,
              q.phase,
              q.tag,
              limit,
              i64::try_from(q.start).map_err(RepositoryError::internal)?
            ],
            read,
          )?
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
          .query_map(
            params![q.folder, limit, i64::try_from(q.start).map_err(RepositoryError::internal)?],
            read,
          )?
          .collect::<Result<_, _>>()?
      }
    };
    // One deleted since the page was chosen is left out.
    let mut items = Vec::new();
    for (kind, id, parent, pinned) in rows {
      let entry = match kind.as_str() {
        "folder" => {
          let Some(folder) = self.folder(&id)? else { continue };
          let size = folder_size(&*self.lock()?, &id)?;
          ConversationEntry::Folder { record: folder, size }
        }
        "group" => {
          let Some(group) = self.group(&id)? else { continue };
          ConversationEntry::Group(group)
        }
        _ => {
          let Some(record) = self.record(&id)? else { continue };
          ConversationEntry::Session(record)
        }
      };
      items.push(ConversationRow { entry, parent, pinned });
    }
    let more = items.len() > q.limit;
    items.truncate(q.limit);
    Ok(RecordPage { items, next: more.then_some(q.start.saturating_add(q.limit)) })
  }
}

/// Query results, with no HTTP serialization contract.
pub struct RecordPage<T> {
  pub items: Vec<T>,
  pub next: Option<usize>,
}
pub struct ConversationRow {
  pub entry: ConversationEntry,
  pub parent: Option<String>,
  pub pinned: bool,
}
pub enum ConversationEntry {
  Folder { record: super::FolderRecord, size: u64 },
  Group(super::GroupRecord),
  Session(Value),
}

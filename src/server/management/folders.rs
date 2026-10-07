//! Folders: how the user arranges the list, the way a file system arranges files. Every session,
//! group and folder names the folder it is in - `folder` for sessions and groups, `parent` for
//! folders, none for the root - and a folder's listing is read along the index on that pointer,
//! so it costs what the folder holds, never what the whole list does. Moving anything changes that
//! one pointer, as renaming a file moves it; deleting a folder hands what it held to its parent.
//! Pinned entries come first in their folder.
use super::ManagementStore;
use crate::server::error::ApiError;
use rusqlite::{OptionalExtension, Row, Transaction, params};
use serde::Serialize;

/// A folder as the API shows it.
#[derive(Clone, Debug, Serialize)]
pub struct FolderRecord {
  pub id: String,
  pub name: String,
  /// The folder it is in; none for the root.
  pub parent: Option<String>,
  pub pinned: bool,
  pub created_at: u64,
}

/// The tables that hold the list's entries, and the column naming the folder each is in.
const ENTRIES: [(&str, &str); 3] =
  [("sessions", "folder"), ("groups", "folder"), ("folders", "parent")];

impl ManagementStore {
  pub fn create_folder(&self, folder: &FolderRecord) -> Result<(), ApiError> {
    let db = self.db.lock().unwrap();
    if let Some(parent) = &folder.parent {
      require_folder(&db, parent)?;
    }
    db.execute(
      "INSERT INTO folders(id,name,parent,pinned,created_at) VALUES(?1,?2,?3,?4,?5)",
      params![folder.id, folder.name, folder.parent, folder.pinned, folder.created_at as i64],
    )?;
    Ok(())
  }
  pub fn folder(&self, id: &str) -> Result<Option<FolderRecord>, ApiError> {
    Ok(
      self
        .db
        .lock()
        .unwrap()
        .query_row(
          "SELECT id,name,parent,pinned,created_at FROM folders WHERE id=?1",
          [id],
          folder_row,
        )
        .optional()?,
    )
  }
  /// Every folder, by name: few enough to choose a place from.
  pub fn folders(&self) -> Result<Vec<FolderRecord>, ApiError> {
    let db = self.db.lock().unwrap();
    let mut statement = db.prepare(
      "SELECT id,name,parent,pinned,created_at FROM folders ORDER BY name COLLATE NOCASE, id",
    )?;
    Ok(statement.query_map([], folder_row)?.collect::<Result<_, _>>()?)
  }
  pub fn rename_folder(&self, id: &str, name: &str) -> Result<(), ApiError> {
    let changed =
      self.db.lock().unwrap().execute("UPDATE folders SET name=?2 WHERE id=?1", [id, name])?;
    if changed == 0 {
      return Err(ApiError::not_found());
    }
    Ok(())
  }
  /// Deletes a folder; what it held goes to the folder it was in.
  pub fn delete_folder(&self, id: &str) -> Result<(), ApiError> {
    let mut db = self.db.lock().unwrap();
    let tx = db.transaction()?;
    let parent: Option<String> = tx
      .query_row("SELECT parent FROM folders WHERE id=?1", [id], |row| row.get(0))
      .optional()?
      .ok_or_else(ApiError::not_found)?;
    for (table, column) in ENTRIES {
      tx.execute(
        &format!("UPDATE {table} SET {column}=?2 WHERE {column}=?1"),
        params![id, parent],
      )?;
    }
    tx.execute("DELETE FROM folders WHERE id=?1", [id])?;
    tx.commit()?;
    Ok(())
  }

  /// Moves sessions, groups and folders, by id, into `folder` (none for the root), together or
  /// not at all. A folder cannot go into itself or into one inside it.
  pub fn place(&self, ids: &[String], folder: Option<&str>) -> Result<(), ApiError> {
    let mut db = self.db.lock().unwrap();
    let tx = db.transaction()?;
    if let Some(folder) = folder {
      require_folder(&tx, folder)?;
    }
    for id in ids {
      if let Some(folder) = folder
        && is_folder(&tx, id)?
        && within(&tx, folder, id)?
      {
        return Err(ApiError::bad_request("a folder cannot go into itself"));
      }
      let moved = ENTRIES.iter().try_fold(0, |moved, (table, column)| {
        tx.execute(&format!("UPDATE {table} SET {column}=?2 WHERE id=?1"), params![id, folder])
          .map(|changed| moved + changed)
      })?;
      if moved == 0 {
        return Err(ApiError::bad_request(format!("nothing in the list is `{id}`")));
      }
    }
    tx.commit()?;
    Ok(())
  }
  /// Pins sessions, groups and folders, by id, to the top of their folders, or unpins them.
  pub fn pin(&self, ids: &[String], pinned: bool) -> Result<(), ApiError> {
    let mut db = self.db.lock().unwrap();
    let tx = db.transaction()?;
    for id in ids {
      let changed = ENTRIES.iter().try_fold(0, |changed, (table, _)| {
        tx.execute(&format!("UPDATE {table} SET pinned=?2 WHERE id=?1"), params![id, pinned])
          .map(|count| changed + count)
      })?;
      if changed == 0 {
        return Err(ApiError::bad_request(format!("nothing in the list is `{id}`")));
      }
    }
    tx.commit()?;
    Ok(())
  }
  /// The folder a session or group is in; none for the root, or for what is not there.
  pub fn folder_of(&self, id: &str) -> Result<Option<String>, ApiError> {
    let db = self.db.lock().unwrap();
    let found: Option<Option<String>> = db
      .query_row(
        "SELECT folder FROM sessions WHERE id=?1 UNION ALL SELECT folder FROM groups WHERE id=?1",
        [id],
        |row| row.get(0),
      )
      .optional()?;
    Ok(found.flatten())
  }
}

/// How many entries a folder holds directly, each counted along its table's index.
pub(super) fn folder_size(db: &rusqlite::Connection, id: &str) -> rusqlite::Result<u64> {
  let mut size = 0;
  for (table, column) in ENTRIES {
    let count: i64 =
      db.query_row(&format!("SELECT count(*) FROM {table} WHERE {column}=?1"), [id], |row| {
        row.get(0)
      })?;
    size += count as u64;
  }
  Ok(size)
}

fn folder_row(row: &Row) -> rusqlite::Result<FolderRecord> {
  Ok(FolderRecord {
    id: row.get(0)?,
    name: row.get(1)?,
    parent: row.get(2)?,
    pinned: row.get(3)?,
    created_at: row.get::<_, i64>(4)? as u64,
  })
}
fn is_folder(db: &rusqlite::Connection, id: &str) -> rusqlite::Result<bool> {
  Ok(db.query_row("SELECT 1 FROM folders WHERE id=?1", [id], |_| Ok(())).optional()?.is_some())
}
pub(super) fn require_folder(db: &rusqlite::Connection, id: &str) -> Result<(), ApiError> {
  if !is_folder(db, id)? {
    return Err(ApiError::bad_request(format!("no folder `{id}`")));
  }
  Ok(())
}
/// Whether `folder` is `ancestor` or inside it, found by climbing parents as a path is resolved.
fn within(tx: &Transaction, folder: &str, ancestor: &str) -> rusqlite::Result<bool> {
  tx.query_row(
    "WITH RECURSIVE up(id) AS (SELECT ?1 UNION SELECT parent FROM folders JOIN up ON folders.id=up.id WHERE parent IS NOT NULL) \
     SELECT 1 FROM up WHERE id=?2",
    [folder, ancestor],
    |_| Ok(()),
  )
  .optional()
  .map(|found| found.is_some())
}

//! Version 3 -> 4: intern long list names without changing public IDs or row contents.
use super::StorageError;
use rusqlite::Connection;

// Called inside the schema transaction. Any failure rolls back the entire upgrade.
pub(super) fn normalize_list_keys(db: &Connection, version: i64) -> Result<(), StorageError> {
  db.execute_batch(
    "CREATE TABLE IF NOT EXISTS wish_list_keys (
    id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE);",
  )?;
  if version != 3 {
    return Ok(());
  }
  db.execute_batch(
    "INSERT INTO wish_list_keys(name) SELECT name FROM wish_lists ORDER BY name;
    CREATE TABLE wish_items_v4 (
      list INTEGER NOT NULL REFERENCES wish_list_keys(id),
      position INTEGER NOT NULL CHECK(position>=0),value BLOB NOT NULL,
      PRIMARY KEY(list,position)) WITHOUT ROWID;
    INSERT INTO wish_items_v4 SELECT k.id,i.position,i.value
      FROM wish_items i JOIN wish_list_keys k ON k.name=i.list;
    CREATE TABLE wish_history_index_v4 (
      id INTEGER PRIMARY KEY, list INTEGER NOT NULL REFERENCES wish_list_keys(id),
      sequence INTEGER NOT NULL, recorded_at INTEGER NOT NULL, generation INTEGER NOT NULL,
      item_kind TEXT NOT NULL, message_type TEXT, event_type TEXT, origin TEXT,
      model_call INTEGER, tool_name TEXT, metadata TEXT NOT NULL, text TEXT NOT NULL,
      UNIQUE(list,sequence));
    INSERT INTO wish_history_index_v4
      SELECT h.id,k.id,h.sequence,h.recorded_at,h.generation,h.item_kind,h.message_type,
      h.event_type,h.origin,h.model_call,h.tool_name,h.metadata,h.text
      FROM wish_history_index h JOIN wish_list_keys k ON k.name=h.list;",
  )?;
  for (old, new) in
    [("wish_items", "wish_items_v4"), ("wish_history_index", "wish_history_index_v4")]
  {
    let old_count: i64 = db.query_row(&format!("SELECT count(*) FROM {old}"), [], |r| r.get(0))?;
    let new_count: i64 = db.query_row(&format!("SELECT count(*) FROM {new}"), [], |r| r.get(0))?;
    if old_count != new_count {
      return Err(StorageError::Corrupt("list key migration would lose rows".into()));
    }
  }
  // Preserve user-created JSON metadata indexes too. FTS row IDs and text stay unchanged:
  // DROP TABLE removes its triggers but does not fire per-row DELETE triggers.
  let indexes = db
    .prepare(
      "SELECT sql FROM sqlite_master
    WHERE type='index' AND tbl_name='wish_history_index' AND sql IS NOT NULL",
    )?
    .query_map([], |r| r.get::<_, String>(0))?
    .collect::<Result<Vec<_>, _>>()?;
  db.execute_batch(
    "DROP TABLE wish_items;
    ALTER TABLE wish_items_v4 RENAME TO wish_items;
    DROP TABLE wish_history_index;
    ALTER TABLE wish_history_index_v4 RENAME TO wish_history_index;",
  )?;
  for sql in indexes {
    db.execute_batch(&sql)?;
  }
  Ok(())
}

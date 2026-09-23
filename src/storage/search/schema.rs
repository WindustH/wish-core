use crate::storage::StorageError;
use rusqlite::OptionalExtension;
pub(crate) fn create_schema(connection: &rusqlite::Connection) -> Result<(), StorageError> {
  connection.execute_batch("CREATE TABLE IF NOT EXISTS wish_history_index (
    id INTEGER PRIMARY KEY, list INTEGER NOT NULL REFERENCES wish_list_keys(id), sequence INTEGER NOT NULL,
    recorded_at INTEGER NOT NULL, generation INTEGER NOT NULL, item_kind TEXT NOT NULL,
    message_type TEXT, event_type TEXT, origin TEXT, model_call INTEGER, tool_name TEXT,
    metadata TEXT NOT NULL, text TEXT NOT NULL, UNIQUE(list,sequence));
    CREATE INDEX IF NOT EXISTS wish_history_by_time ON wish_history_index(list,recorded_at,sequence);
    CREATE INDEX IF NOT EXISTS wish_history_by_kind ON wish_history_index(list,item_kind,sequence);
    CREATE INDEX IF NOT EXISTS wish_history_by_generation ON wish_history_index(list,generation,sequence);
    CREATE VIRTUAL TABLE IF NOT EXISTS wish_history_words USING fts5(text,content='wish_history_index',content_rowid='id');
    CREATE VIRTUAL TABLE IF NOT EXISTS wish_history_substrings USING fts5(text,content='wish_history_index',content_rowid='id',tokenize='trigram');
    CREATE TRIGGER IF NOT EXISTS wish_history_insert AFTER INSERT ON wish_history_index WHEN new.text != '' BEGIN
      INSERT INTO wish_history_words(rowid,text) VALUES(new.id,new.text);
      INSERT INTO wish_history_substrings(rowid,text) VALUES(new.id,new.text);
    END;
    CREATE TRIGGER IF NOT EXISTS wish_history_delete AFTER DELETE ON wish_history_index WHEN old.text != '' BEGIN
      INSERT INTO wish_history_words(wish_history_words,rowid,text) VALUES('delete',old.id,old.text);
      INSERT INTO wish_history_substrings(wish_history_substrings,rowid,text) VALUES('delete',old.id,old.text);
    END;")?;
  // Equality/IN filters never match NULL. Do not replicate every stream event into
  // message-only indexes. Keep names stable and upgrade old full indexes once.
  for (name, columns, present) in [
    ("message", "list,message_type,sequence", "message_type"),
    ("message_time", "list,message_type,recorded_at,sequence", "message_type"),
    ("origin", "list,origin,sequence", "origin"),
    ("event", "list,event_type,sequence", "event_type"),
    ("call", "list,model_call,sequence", "model_call"),
    ("tool", "list,tool_name,sequence", "tool_name"),
  ] {
    let name = format!("wish_history_by_{name}");
    let sql =
      format!("CREATE INDEX {name} ON wish_history_index({columns}) WHERE {present} IS NOT NULL");
    let existing: Option<String> = connection
      .query_row("SELECT sql FROM sqlite_master WHERE type='index' AND name=?1", [&name], |row| {
        row.get(0)
      })
      .optional()?;
    if existing.as_deref() != Some(sql.as_str()) {
      connection.execute_batch(&format!("DROP INDEX IF EXISTS {name}; {sql};"))?;
    }
  }
  Ok(())
}

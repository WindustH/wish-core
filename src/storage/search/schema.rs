use crate::storage::StorageError;
pub(crate) fn create_schema(connection: &rusqlite::Connection) -> Result<(), StorageError> {
  connection.execute_batch("CREATE TABLE IF NOT EXISTS wish_history_index (
    id INTEGER PRIMARY KEY, list TEXT NOT NULL, sequence INTEGER NOT NULL,
    recorded_at INTEGER NOT NULL, generation INTEGER NOT NULL, item_kind TEXT NOT NULL,
    message_type TEXT, event_type TEXT, origin TEXT, model_call INTEGER, tool_name TEXT,
    metadata TEXT NOT NULL, text TEXT NOT NULL, UNIQUE(list,sequence));
    CREATE INDEX IF NOT EXISTS wish_history_by_time ON wish_history_index(list,recorded_at,sequence);
    CREATE INDEX IF NOT EXISTS wish_history_by_kind ON wish_history_index(list,item_kind,sequence);
    CREATE INDEX IF NOT EXISTS wish_history_by_message ON wish_history_index(list,message_type,sequence);
    CREATE INDEX IF NOT EXISTS wish_history_by_message_time ON wish_history_index(list,message_type,recorded_at,sequence);
    CREATE INDEX IF NOT EXISTS wish_history_by_origin ON wish_history_index(list,origin,sequence);
    CREATE INDEX IF NOT EXISTS wish_history_by_event ON wish_history_index(list,event_type,sequence);
    CREATE INDEX IF NOT EXISTS wish_history_by_generation ON wish_history_index(list,generation,sequence);
    CREATE INDEX IF NOT EXISTS wish_history_by_call ON wish_history_index(list,model_call,sequence);
    CREATE INDEX IF NOT EXISTS wish_history_by_tool ON wish_history_index(list,tool_name,sequence);
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
  Ok(())
}

//! Secondary history indexes. The session layer extracts fields; SQL stays here.
mod query;
mod schema;
use super::{StorageError, Transaction};
pub(crate) use query::{IndexFilter, IndexHit, IndexPage, MetadataCondition, SearchText};
use rusqlite::params;

pub(crate) use schema::create_schema;

#[derive(Default)]
pub(crate) struct IndexRecord {
  pub sequence: u64,
  pub recorded_at: u64,
  pub generation: u64,
  pub item_kind: &'static str,
  pub message_type: Option<&'static str>,
  pub event_type: Option<&'static str>,
  pub origin: Option<String>,
  pub model_call: Option<u64>,
  pub tool_name: Option<String>,
  pub metadata: String,
  pub text: String,
}
impl Transaction<'_> {
  pub(crate) fn reset_history_index(&self, list: &str) -> Result<(), StorageError> {
    self.sql.execute(
      "DELETE FROM wish_history_index WHERE list=(SELECT id FROM wish_list_keys WHERE name=?1)",
      [list],
    )?;
    Ok(())
  }

  pub(crate) fn index_history_record(
    &mut self,
    list: &str,
    row: &IndexRecord,
  ) -> Result<(), StorageError> {
    let sequence = to_integer(row.sequence)?;
    let recorded_at = to_integer(row.recorded_at)?;
    let generation = to_integer(row.generation)?;
    let model_call = row.model_call.map(to_integer).transpose()?;
    self.sql.execute("INSERT INTO wish_history_index
      (list,sequence,recorded_at,generation,item_kind,message_type,event_type,origin,model_call,tool_name,metadata,text)
      VALUES ((SELECT id FROM wish_list_keys WHERE name=?1),?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12) ON CONFLICT(list,sequence) DO NOTHING",
      params![list,sequence,recorded_at,generation,row.item_kind,row.message_type,row.event_type,
        row.origin,model_call,row.tool_name,row.metadata,row.text])?;
    Ok(())
  }
  pub(crate) fn index_history_metadata(&self, path: &str) -> Result<(), StorageError> {
    // Validate SQLite's JSON path syntax before storing it in a schema expression.
    self.sql.query_row("SELECT json_extract('{}',?1)", [path], |_| Ok(()))?;
    use sha2::{Digest, Sha256};
    let name = hex::encode(Sha256::digest(path.as_bytes()));
    let path = query::quote_literal(path);
    self.sql.execute_batch(&format!("CREATE INDEX IF NOT EXISTS wish_history_metadata_{name}
      ON wish_history_index(list,json_extract(metadata,{path}),json_type(metadata,{path}),sequence)"))?;
    Ok(())
  }
}

fn to_integer(value: u64) -> Result<i64, StorageError> {
  value.try_into().map_err(|_| StorageError::InvalidRange)
}

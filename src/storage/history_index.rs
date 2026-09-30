//! The index of session history: a row per history record, with the fields it is filtered by and
//! the text it is searched by. The session layer extracts the fields; the SQL stays here, including
//! what deleting and measuring lists does to the index.
mod query;
mod schema;
use super::{ListId, NamespaceUsage, StorageError, Transaction};
pub(crate) use query::{IndexFilter, IndexHit, IndexPage, MetadataCondition, SearchText};
use rusqlite::params;
use std::collections::BTreeMap;

pub(crate) use schema::{compact_index, create_schema};

#[derive(Default)]
pub(crate) struct IndexRow {
  pub sequence: u64,
  pub recorded_at: u64,
  pub generation: u64,
  pub item_kind: &'static str,
  pub message_type: Option<&'static str>,
  pub event_type: Option<&'static str>,
  pub origin: Option<&'static str>,
  pub model_call: Option<u64>,
  pub tool_name: Option<String>,
  pub metadata: String,
  pub text: String,
}
impl Transaction<'_> {
  /// Drop the index rows of `sequences` and return the bytes of text and metadata they held.
  pub(crate) fn remove_history_records(
    &self,
    list: &ListId,
    sequences: &[u64],
  ) -> Result<u64, StorageError> {
    let mut statement = self.sql.prepare(concat!(
      "DELETE FROM wish_history_index WHERE list=",
      list_key!(),
      " AND sequence=?2 RETURNING length(text)+length(metadata)"
    ))?;
    let mut removed = 0;
    for sequence in sequences {
      let mut rows = statement.query(params![list.0, to_integer(*sequence)?])?;
      while let Some(row) = rows.next()? {
        removed += row.get::<_, i64>(0)?.max(0) as u64;
      }
    }
    Ok(removed)
  }

  pub(crate) fn index_history_record(
    &mut self,
    list: &ListId,
    row: &IndexRow,
  ) -> Result<(), StorageError> {
    let sequence = to_integer(row.sequence)?;
    let recorded_at = to_integer(row.recorded_at)?;
    let generation = to_integer(row.generation)?;
    let model_call = row.model_call.map(to_integer).transpose()?;
    self.sql.execute(concat!("INSERT INTO wish_history_index
      (list,sequence,recorded_at,generation,item_kind,message_type,event_type,origin,model_call,tool_name,metadata,text)
      VALUES (", list_key!(), ",?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12) ON CONFLICT(list,sequence) DO NOTHING"),
      params![list.0,sequence,recorded_at,generation,row.item_kind,row.message_type,row.event_type,
        row.origin,model_call,row.tool_name,row.metadata,row.text])?;
    Ok(())
  }

  /// The index rows of `list`, which is being deleted.
  pub(super) fn delete_history_rows(&self, list: &ListId) -> Result<(), StorageError> {
    self
      .sql
      .execute(concat!("DELETE FROM wish_history_index WHERE list=", list_key!()), [&list.0])?;
    Ok(())
  }
  /// The index rows of every list whose name starts with `prefix`, which are being deleted.
  pub(super) fn delete_namespace_history_rows(&self, prefix: &str) -> Result<(), StorageError> {
    self.sql.execute("DELETE FROM wish_history_index WHERE list IN (SELECT id FROM wish_list_keys WHERE substr(name,1,length(?1))=?1)", [prefix])?;
    Ok(())
  }
}

/// Adds to `usage` what the index holds for each child namespace: the bytes of its rows' text and
/// metadata, and its history records by message type. `child` is the SQL naming a list's child
/// namespace below `prefix`, parameter 1.
pub(super) fn measure_children(
  sql: &rusqlite::Connection,
  prefix: &str,
  child: &str,
  usage: &mut BTreeMap<String, NamespaceUsage>,
) -> Result<(), StorageError> {
  let mut statement = sql.prepare(&format!("SELECT {child}, sum(length(record.text) + length(record.metadata)) FROM wish_history_index record JOIN wish_list_keys list ON list.id = record.list WHERE substr(name, 1, length(?1)) = ?1 GROUP BY 1"))?;
  let rows = statement
    .query_map([prefix], |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?)))?;
  for row in rows {
    let (name, bytes) = row?;
    usage.entry(name).or_default().bytes += bytes.unwrap_or(0).max(0) as u64;
  }
  let mut statement = sql.prepare(&format!(
    "SELECT {child}, record.message_type, count(*) FROM wish_history_index record JOIN wish_list_keys list ON list.id = record.list WHERE substr(name, 1, length(?1)) = ?1 AND record.message_type IS NOT NULL GROUP BY 1, 2"
  ))?;
  let rows = statement.query_map([prefix], |row| {
    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, i64>(2)?))
  })?;
  for row in rows {
    let (name, kind, count) = row?;
    usage.entry(name).or_default().messages.insert(kind, count.max(0) as u64);
  }
  Ok(())
}

/// A position, time or id as SQLite's signed integer.
fn to_integer(value: u64) -> Result<i64, StorageError> {
  value.try_into().map_err(|_| StorageError::InvalidRange)
}

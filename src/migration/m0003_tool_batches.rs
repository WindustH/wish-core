//! 2 to 3: a tool batch - the calls of one model turn and how each went - only matters while its
//! tools run, and is now deleted when they finish. Batches earlier runs left behind are deleted
//! here; the entries and events already keep what the tools did. A session still executing tools
//! keeps the batch its state names.

use super::Data;
use serde_json::Value;
use std::collections::HashSet;

pub const SUMMARY: &str = "delete the tool batches finished runs left behind";

const SESSION_RECORD: &str = "wish_core::session::persistence::SessionRecord";
const TOOL_BATCH: &str = "wish_core::session::machine::state::ToolExecution";

pub fn apply(data: &mut Data) -> Result<(), String> {
  let Some(wish) = data.wish else { return Ok(()) };
  let failed = |error: rusqlite::Error| error.to_string();
  let mut in_use = HashSet::new();
  let mut records = wish.prepare("SELECT value FROM wish_objects WHERE kind=?1").map_err(failed)?;
  for record in
    records.query_map([SESSION_RECORD], |row| row.get::<_, Vec<u8>>(0)).map_err(failed)?
  {
    let record: Value =
      serde_json::from_slice(&record.map_err(failed)?).map_err(|error| error.to_string())?;
    collect_batches(&record["state"], &mut in_use);
  }
  let mut lists = wish.prepare("SELECT name FROM wish_lists WHERE kind=?1").map_err(failed)?;
  let names = lists
    .query_map([TOOL_BATCH], |row| row.get::<_, String>(0))
    .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
    .map_err(failed)?;
  for name in names.iter().filter(|name| !in_use.contains(*name)) {
    wish
      .execute(
        "DELETE FROM wish_items WHERE list=(SELECT id FROM wish_list_keys WHERE name=?1)",
        [name],
      )
      .and_then(|_| wish.execute("DELETE FROM wish_list_keys WHERE name=?1", [name]))
      .and_then(|_| wish.execute("DELETE FROM wish_lists WHERE name=?1", [name]))
      .map_err(failed)?;
  }
  Ok(())
}

/// The batch an `ExecutingTools` state names, wherever the state keeps it.
fn collect_batches(state: &Value, found: &mut HashSet<String>) {
  match state {
    Value::Object(fields) => {
      for (key, value) in fields {
        match (key.as_str(), value) {
          ("batch", Value::String(name)) => {
            found.insert(name.clone());
          }
          _ => collect_batches(value, found),
        }
      }
    }
    Value::Array(items) => items.iter().for_each(|item| collect_batches(item, found)),
    _ => {}
  }
}

//! 0 to 1: a session's MCP switch now only decides what `wish mcp` answers - the model's request is
//! the same either way - so it is switched on everywhere: in every session's record in
//! `management.sqlite` (`session.tools.mcp`) and for new sessions (`defaults.tools.mcp`).

use super::Data;
use serde_json::Value;

pub const SUMMARY: &str = "switch MCP on for every session and for new sessions";

pub fn apply(data: &mut Data) -> Result<(), String> {
  if let Some(tools) = data.config.pointer_mut("/defaults/tools").and_then(Value::as_object_mut) {
    tools.insert("mcp".into(), Value::Bool(true));
  }
  let Some(management) = data.management else { return Ok(()) };
  let records = management
    .prepare("SELECT id, record FROM sessions")
    .and_then(|mut statement| {
      statement
        .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?
        .collect::<Result<Vec<_>, _>>()
    })
    .map_err(|error| error.to_string())?;
  for (id, record) in records {
    let mut value: Value =
      serde_json::from_str(&record).map_err(|error| format!("session {id}: {error}"))?;
    let Some(tools) = value.pointer_mut("/session/tools").and_then(Value::as_object_mut) else {
      return Err(format!("session {id}: the record has no tool switches"));
    };
    tools.insert("mcp".into(), Value::Bool(true));
    let record = serde_json::to_string(&value).map_err(|error| error.to_string())?;
    management
      .execute("UPDATE sessions SET record = ?1 WHERE id = ?2", [&record, &id])
      .map_err(|error| format!("session {id}: {error}"))?;
  }
  Ok(())
}

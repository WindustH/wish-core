//! 8 to 9: sessions gain a sessions switch, deciding what `wish session` may do. Like MCP's and
//! the skills', it changes neither the tools nor the instructions a model is sent, so it starts
//! on: in every session's record in `management.sqlite` (`session.tools.sessions`) and for new
//! sessions (`defaults.tools.sessions`). A switch already there is left as it is.

use super::Data;
use serde_json::Value;

pub const SUMMARY: &str = "add the sessions switch, on for every session and for new sessions";

pub fn apply(data: &mut Data) -> Result<(), String> {
  if let Some(tools) = data.config.pointer_mut("/defaults/tools").and_then(Value::as_object_mut) {
    tools.entry("sessions").or_insert(Value::Bool(true));
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
    if tools.contains_key("sessions") {
      continue;
    }
    tools.insert("sessions".into(), Value::Bool(true));
    let record = serde_json::to_string(&value).map_err(|error| error.to_string())?;
    management
      .execute("UPDATE sessions SET record = ?1 WHERE id = ?2", [&record, &id])
      .map_err(|error| format!("session {id}: {error}"))?;
  }
  Ok(())
}

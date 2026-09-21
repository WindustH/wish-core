//! Indivisible replay units used when removing a context prefix.
use crate::protocol::{Message, error::Error};

/// Boundaries never split a model turn's reasoning/text/calls or its complete tool-result batch.
/// Wire rendering must additionally validate any replacement assembled from these units.
pub fn find_boundaries(messages: &[Message]) -> Result<Vec<usize>, Error> {
  let mut boundaries = vec![0];
  let mut pending: Vec<(&str, &str)> = Vec::new();
  for (index, message) in messages.iter().enumerate() {
    let starts_unit = matches!(
      message,
      Message::User { .. }
        | Message::System { .. }
        | Message::Developer { .. }
        | Message::UpstreamCompaction { .. }
    ) || (index > 0
      && matches!(messages[index - 1], Message::ToolResult { .. })
      && !matches!(message, Message::ToolResult { .. }));
    if starts_unit {
      if !pending.is_empty() {
        return Err(Error::Build("context boundary leaves unanswered tool calls".into()));
      }
      if index != 0 {
        boundaries.push(index);
      }
    }
    match message {
      Message::ToolUse { call_id, name, .. } => pending.push((call_id, name)),
      Message::ToolResult { call_id, name, .. } => {
        let Some(position) = pending.iter().position(|(id, tool)| *id == call_id && *tool == name)
        else {
          return Err(Error::Build("context contains an orphaned tool result".into()));
        };
        pending.remove(position);
      }
      _ => {}
    }
  }
  if !pending.is_empty() {
    return Err(Error::Build("context contains unanswered tool calls".into()));
  }
  if boundaries.last() != Some(&messages.len()) {
    boundaries.push(messages.len());
  }
  Ok(boundaries)
}

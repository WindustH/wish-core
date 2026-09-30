use crate::protocol::Message;
use crate::session::SessionError;
use serde_json::Value;

/// Whether the session takes `message` as input: a user, system or developer message.
pub(in crate::session) fn is_input(message: &Message) -> bool {
  matches!(message, Message::User { .. } | Message::System { .. } | Message::Developer { .. })
}

/// Whether a tool call names itself and its tool, and passes an object of arguments.
pub(in crate::session) fn is_valid_tool_use(call_id: &str, name: &str, arguments: &Value) -> bool {
  !call_id.is_empty() && !name.is_empty() && arguments.is_object()
}

pub(in crate::session) fn validate_tool_pairs<'a>(
  messages: impl Iterator<Item = &'a Message>,
) -> Result<(), SessionError> {
  let mut validator = ToolPairValidator::default();
  for message in messages {
    validator.accept(message)?;
  }
  validator.finish()
}
#[derive(Default)]
pub(super) struct ToolPairValidator {
  pending: std::collections::HashMap<String, String>,
}
impl ToolPairValidator {
  pub fn accept(&mut self, message: &Message) -> Result<(), SessionError> {
    match message {
      Message::ToolUse { call_id, name, arguments, .. } => {
        if !is_valid_tool_use(call_id, name, arguments)
          || self.pending.insert(call_id.clone(), name.clone()).is_some()
        {
          return Err(SessionError::UnpairedTools);
        }
      }
      Message::ToolResult { call_id, name, .. } => {
        if self.pending.remove(call_id).as_ref() != Some(name) {
          return Err(SessionError::UnpairedTools);
        }
      }
      message if is_input(message) && !self.pending.is_empty() => {
        return Err(SessionError::UnpairedTools);
      }
      _ => {}
    }
    Ok(())
  }
  pub fn finish(self) -> Result<(), SessionError> {
    if self.pending.is_empty() { Ok(()) } else { Err(SessionError::UnpairedTools) }
  }
}

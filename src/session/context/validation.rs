use crate::protocol::Message;
use crate::session::SessionError;

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
        if call_id.is_empty()
          || name.is_empty()
          || !arguments.is_object()
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
      Message::User { .. } | Message::System { .. } | Message::Developer { .. }
        if !self.pending.is_empty() =>
      {
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

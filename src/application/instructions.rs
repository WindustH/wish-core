//! Builds and persists the application agent instructions for each generation.
use crate::protocol::{ContentBlock, Message, Tool};
const AGENT_INSTRUCTIONS: &str =
  "You are the agent operating this session. Use the provided tools to complete the user's task.";
/// The agent instructions persisted for a generation with `tools`: the fixed text, then how to reach MCP
/// servers, skills and other sessions when there is a shell to reach them from. None depends on
/// what there is or on the session's switches, so only switching the shell - which changes the
/// tools anyway - changes it.
pub fn agent_instructions(tools: &[Tool]) -> String {
  agent_instructions_for_shell(tools.iter().any(|tool| tool.name == "shell_start"))
}
pub fn agent_instructions_for_shell(shell: bool) -> String {
  let mut text = AGENT_INSTRUCTIONS.to_owned();
  if shell {
    for section in [
      crate::application::mcp::INSTRUCTIONS,
      crate::application::skills::INSTRUCTIONS,
      crate::application::peers::INSTRUCTIONS,
    ] {
      text.push_str("\n\n");
      text.push_str(section);
    }
  }
  text
}
/// Puts the current agent instructions in the conversation: in place of the ones it carries, or
/// first when it carries none.
/// Creation knows the switches before tool implementations are installed.
pub fn apply_agent_instructions_for_shell(messages: &mut Vec<Message>, shell: bool) {
  replace_agent_instructions(messages, agent_instructions_for_shell(shell));
}
fn replace_agent_instructions(messages: &mut Vec<Message>, text: String) {
  let mut found = false;
  for message in messages.iter_mut() {
    if let Message::System { metadata, content } = message
      && metadata["source"] == "wish_agent_instructions"
    {
      *content = vec![ContentBlock::Text { text: text.clone() }];
      found = true;
    }
  }
  if !found {
    messages.insert(
      0,
      Message::System {
        metadata: serde_json::json!({"source":"wish_agent_instructions"}),
        content: vec![ContentBlock::Text { text }],
      },
    );
  }
}
pub fn persist_agent_instructions(
  session: &mut crate::session::Session,
  config: crate::session::SessionConfig,
) -> Result<(), crate::session::SessionError> {
  session.set_system_instruction_config(
    "wish_agent_instructions".into(),
    agent_instructions(&config.tools),
    config,
  )
}

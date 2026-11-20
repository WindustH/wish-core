//! One model operation outside the session state machine; no history or tool execution.
use super::{CallResponse, ModelCaller, ModelStream};
use crate::{
  protocol::error::Error,
  protocol::{
    ContentBlock, Message, Request, Response, ToolChoice,
    model_use::{ModelUseProtocol, mode::MessagesApiCompatMode, tool::tool_result_text},
  },
};
pub async fn call_once(caller: &impl ModelCaller, request: &Request) -> Result<Response, Error> {
  match caller.call(request).await? {
    CallResponse::Complete(response) => Ok(*response),
    CallResponse::Stream(mut stream) => {
      let mut accumulator = stream.create_accumulator();
      while let Some(event) = stream.next().await? {
        accumulator.feed(event)?;
      }
      accumulator.finish()
    }
  }
}

/// Prevent new calls on a text-only operation, with the original prefix wherever supported.
pub(crate) fn constrain_text_call(caller: &impl ModelCaller, request: &mut Request) {
  omit_unsupported_side_call_tools(caller, request);
  request.tool_choice = if request.tools.is_empty() { None } else { Some(ToolChoice::None) };
}

/// Converse and vendor Messages endpoints need not implement the official `none` choice.
/// On these wires send historical tool activity as ordinary text in the side-call copy.
pub(crate) fn omit_unsupported_side_call_tools(caller: &impl ModelCaller, request: &mut Request) {
  let omit_tools = match caller.get_model_use_protocol() {
    Some(ModelUseProtocol::BedrockConverse) => true,
    Some(ModelUseProtocol::AnthropicMessages(mode)) => mode != MessagesApiCompatMode::Official,
    _ => false,
  };
  if omit_tools {
    request.tools.clear();
    request.tool_choice = None;
    for message in &mut request.conversation {
      *message = match message {
        Message::ToolUse { metadata, call_id, name, arguments } => Message::Assistant {
          metadata: metadata.clone(),
          content: vec![ContentBlock::Text {
            text: format!("[Historical tool call {call_id}] {name} {arguments}"),
          }],
        },
        Message::ToolResult { metadata, call_id, name, content } => Message::User {
          metadata: metadata.clone(),
          content: vec![ContentBlock::Text {
            text: format!(
              "[Historical tool result {call_id}] {name} {}",
              tool_result_text(content)
            ),
          }],
        },
        _ => continue,
      };
    }
  }
}

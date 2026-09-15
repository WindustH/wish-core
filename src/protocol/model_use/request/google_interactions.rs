//! Google `interactions` request wire (`POST /v1beta/interactions`).
//!
//! Conversions:
//! - `model` travels in the body, since the URL carries no model segment.
//! - Leading `System` messages become the top-level `system_instruction` string.
//! - The conversation becomes a `steps[]` array of `user_input`, `model_output`, `function_call`,
//!   `function_result` and `thought` steps. Images are flat blocks
//!   (`{"type": "image", "mime_type", "data"}`).
//! - `Reasoning` rebuilds a `thought` step from its parts: the proof rides in `signature` and the
//!   text in a `summary[]` entry, which is the shape the wire sends and the shape it takes back.
//!
//! Constraints:
//! - Model-generated steps must be resent exactly as received: stripping or editing a `thought` step
//!   or its signature corrupts the reasoning context, and stateless mode has to resend the built-in
//!   tools' signatures too. A signature belongs only to the step that carried it - never to a
//!   `user_input`, a `model_output` or a custom `function_call`.
//! - Instructions go in the top-level `system_instruction`, which only takes text and has to lead
//!   the conversation: a `System` message past the leading run, or a non-text instruction block, is
//!   rejected. A `function_result` answers the `function_call` before it.
//!
//! Trade-offs:
//! - The wire has no developer role: a `Developer` message is sent as user input, so application
//!   instructions travel with the user's own words.
//! - `tool_choice`, `max_output_tokens` and `ReasoningConfig` share `generation_config`, which is
//!   omitted when all of them are absent; `stream` is always sent, since one body serves both calls.
//! - Of the reasoning axes only a tier word and a summary have a spelling (`thinking_level`, spelled
//!   lowercase, and `thinking_summaries`); an off switch is rejected.
//! - Requests are always stateless: `store: false` and no `previous_interaction_id`.
//! - A call goes out as `name` + `arguments`; the decoders also read `tool_name` + `args`, which
//!   older traffic spells.
//! - Not modeled: the eight built-in tools (and the signatures their steps carry), `agent` /
//!   `agent_config`, `background`, `previous_interaction_id`, `response_format`, `seed`,
//!   `stop_sequences`, `speech_config`, `transcription_config`, `video_config`, and the retrieve /
//!   cancel / delete endpoints.
//! - Tool results pass the payload through unchanged, and call/result pairing is not validated yet.
//! - Caching is implicit here, so `PromptCache` is not sent: explicit cached content is not
//!   modeled.

use crate::protocol::error::Error;
use crate::protocol::{
  ContentBlock, Message, ReasoningConfig, ReasoningSummary, Request, Tool, ToolChoice,
};
use serde_json::{Map, Value, json};

/// Headers every call carries, besides auth and `content-type`.
pub const HEADERS: &[(&str, &str)] = &[];

pub fn render(request: &Request, stream: bool) -> Result<Value, Error> {
  let mut body = Map::new();
  body.insert("model".into(), json!(request.model));
  body.insert("stream".into(), json!(stream));
  let (instruction, rest) = split_leading_instructions(&request.conversation)?;
  body.insert("input".into(), Value::Array(render_steps(rest)?));
  if let Some(instruction) = instruction {
    body.insert("system_instruction".into(), json!(instruction));
  }
  if !request.tools.is_empty() {
    let tools: Vec<Value> = request.tools.iter().map(render_tool).collect();
    body.insert("tools".into(), Value::Array(tools));
  }
  if let Some(config) = render_generation_config(request)? {
    body.insert("generation_config".into(), config);
  }
  body.insert("store".into(), json!(false));
  Ok(Value::Object(body))
}

/// The instruction the leading `System` / `Developer` run spells out, and the conversation that
/// remains after it: the wire takes the instruction at the top level, so the run is consumed here
/// and a `System` message past it is an error inside `render_steps`.
fn split_leading_instructions(
  conversation: &[Message],
) -> Result<(Option<String>, &[Message]), Error> {
  let mut text: Vec<&str> = Vec::new();
  let mut index = 0;
  while let Some(message) = conversation.get(index) {
    let content = match message {
      Message::System { content } | Message::Developer { content } => content,
      _ => break,
    };
    for block in content {
      match block {
        ContentBlock::Text { text: block_text } => text.push(block_text),
        _ => {
          return Err(Error::Build(
            "instruction can only carry text blocks on the interactions wire".to_owned(),
          ));
        }
      }
    }
    index += 1;
  }
  let instruction = (!text.is_empty()).then(|| text.join("\n"));
  Ok((instruction, &conversation[index..]))
}

fn render_steps(conversation: &[Message]) -> Result<Vec<Value>, Error> {
  let mut steps: Vec<Value> = Vec::new();
  for message in conversation {
    match message {
      Message::System { .. } => {
        return Err(Error::Build(
          "system instruction must lead the conversation on the interactions wire".to_owned(),
        ));
      }
      Message::UpstreamCompaction { .. } => {
        return Err(Error::Build(
          "the interactions wire cannot carry a compacted conversation".to_owned(),
        ));
      }
      // Both the developer and user instructions are the same `user_input` step on this wire.
      Message::Developer { content } | Message::User { content } => {
        steps.push(json!({"type": "user_input", "content": render_content_blocks(content)}))
      }
      Message::Reasoning { plaintext, signature, .. } => {
        if let Some(step) = render_thought(plaintext, signature) {
          steps.push(step);
        }
      }
      Message::Assistant { content } => {
        if !content.is_empty() {
          steps.push(render_model_output(content)?);
        }
      }
      Message::ToolUse { call_id, name, arguments } => {
        let mut step = json!({"type": "function_call", "name": name, "arguments": arguments});
        if !call_id.is_empty() {
          step["id"] = json!(call_id);
        }
        steps.push(step);
      }
      Message::ToolResult { call_id, name, content } => {
        let mut step = json!({"type": "function_result", "name": name, "result": content});
        if !call_id.is_empty() {
          step["call_id"] = json!(call_id);
        }
        steps.push(step);
      }
    }
  }
  Ok(steps)
}

fn render_model_output(content: &[ContentBlock]) -> Result<Value, Error> {
  if content.iter().any(|block| matches!(block, ContentBlock::Image { .. })) {
    return Err(Error::Build(
      "model_output cannot carry images on the interactions wire".to_owned(),
    ));
  }
  Ok(json!({"type": "model_output", "content": render_content_blocks(content)}))
}

fn render_content_blocks(content: &[ContentBlock]) -> Vec<Value> {
  let mut blocks: Vec<Value> = Vec::new();
  for block in content {
    match block {
      ContentBlock::Text { text } => blocks.push(json!({"type": "text", "text": text})),
      ContentBlock::Image { mime_type, data_base64 } => {
        blocks.push(json!({"type": "image", "mime_type": mime_type, "data": data_base64}))
      }
    }
  }
  blocks
}

fn render_thought(plaintext: &str, signature: &str) -> Option<Value> {
  if plaintext.is_empty() && signature.is_empty() {
    return None;
  }
  let mut step = json!({"type": "thought"});
  if !signature.is_empty() {
    step["signature"] = json!(signature);
  }
  if !plaintext.is_empty() {
    step["summary"] = json!([{"type": "text", "text": plaintext}]);
  }
  Some(step)
}

fn render_tool(tool: &Tool) -> Value {
  json!({
    "type": "function",
    "name": tool.name,
    "description": tool.description,
    "parameters": tool.input_schema,
  })
}

fn render_generation_config(request: &Request) -> Result<Option<Value>, Error> {
  let mut config = Map::new();
  if !request.tools.is_empty()
    && let Some(choice) = request.tool_choice
  {
    let choice = match choice {
      ToolChoice::Auto => "auto",
      ToolChoice::None => "none",
      ToolChoice::Required => "any",
    };
    config.insert("tool_choice".into(), json!(choice));
  }
  if let Some(max_output_tokens) = request.max_output_tokens {
    config.insert("max_output_tokens".into(), json!(max_output_tokens));
  }
  if let Some(reasoning) = &request.reasoning {
    render_reasoning(reasoning, &mut config)?;
  }
  if config.is_empty() {
    return Ok(None);
  }
  Ok(Some(Value::Object(config)))
}

/// Of the reasoning axes this wire spells a tier word and a summary only: `enabled: false` has no
/// spelling.
fn render_reasoning(
  config: &ReasoningConfig,
  output: &mut Map<String, Value>,
) -> Result<(), Error> {
  if config.enabled == Some(false) {
    return Err(Error::Build("the interactions wire has no reasoning off switch".to_owned()));
  }
  if let Some(effort) = &config.effort {
    output.insert("thinking_level".into(), json!(effort.to_lowercase()));
  }
  match config.summary {
    Some(ReasoningSummary::Auto) => {
      output.insert("thinking_summaries".into(), json!("auto"));
    }
    Some(ReasoningSummary::None) => {
      output.insert("thinking_summaries".into(), json!("none"));
    }
    None => {}
  }
  Ok(())
}

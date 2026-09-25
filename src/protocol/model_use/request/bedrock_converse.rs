//! Bedrock Converse request wire.
//!
//! Conversions:
//! - Leading `System` messages become the top-level `system[]` blocks. A `System` or `Developer`
//!   message past the leading run becomes a `user` turn in place.
//! - Content is always a block array: text -> `{"text": ...}`, images -> `{"image": {format,
//!   bytes}}`, and tool results -> `{"toolResult": {toolUseId, status, content[]}}` where an object
//!   payload travels as `json` and anything else as text.
//! - Adjacent same-role turns merge, `ToolUse` folds into the assistant turn that requests it, and
//!   `Reasoning` replays inside `reasoningContent`: `reasoningText` with `text` + `signature`, or
//!   `redactedContent` with a `ciphertext`.
//! - `ReasoningConfig` is bridged through `additionalModelRequestFields.thinking`, the escape hatch
//!   this provider-agnostic wire uses for Anthropic's thinking parameters, so the shared tier preset
//!   (a budget per word, or `adaptive`) applies here too.
//! - `PromptCache::breakpoints` marks the same three places as the Anthropic wire (the system
//!   blocks, the last tool definition, the tail of the last message), but this wire takes them as
//!   standalone entries appended after the cached blocks.
//!
//! Constraints:
//! - `messages` knows exactly two roles, `user` and `assistant`; instructions ride in the top-level
//!   `system[]` blocks instead, which only take text: a non-text block in the leading instruction
//!   run is rejected.
//! - A `toolResult` answers a `toolUse` of the assistant turn before it and repeats its `toolUseId`.
//! - A signed thinking block has to be replayed: dropping the block or its signature makes the
//!   service reject the next turn with a signature mismatch.
//! - A call is SigV4-signed rather than bearer-authenticated: an unsigned call is rejected.
//!
//! Trade-offs:
//! - `ToolChoice::None` is expressed by omitting `toolChoice` entirely, because Converse has no
//!   "none" member; the model then keeps its configured default.
//! - `reasoningText.text` carries `plaintext`: `display` has no slot on this wire.
//! - `inferenceConfig` is only sent when `max_output_tokens` is set, and stays optional even though
//!   `maxTokens` is mandatory inside it; a cap at or below the bridged thinking budget is raised to
//!   leave room for the answer.
//! - `toolUseId` pairing is validated in both directions, and a cache key has no spelling on this
//!   wire: `PromptCache::breakpoints` is dropped for models outside the Claude family (they reject
//!   `cachePoint`), and the response half of the `additionalModelRequestFields` bridge is not read
//!   yet.
//! - The signing is not implemented yet - it belongs to the transport layer - so a real Converse
//!   endpoint rejects a call until it is.
//! - A `Developer` message has no role of its own on this wire, so in the leading run its text joins
//!   the same `system[]` blocks, and past it the model reads it as user input.

use crate::protocol::error::Error;
use crate::protocol::ReasoningOpaqueKind;
use crate::protocol::model_use::request::{ANSWER_HEADROOM, TierBudget, resolve_tier_budget};
use crate::protocol::{ContentBlock, Message, ReasoningConfig, Request, Tool, ToolChoice};
use serde_json::{Map, Value, json};

/// Headers every call carries, besides auth and `content-type`.
pub const HEADERS: &[(&str, &str)] = &[];

pub fn render(request: &Request) -> Result<Value, Error> {
  // Only the Claude family takes explicit `cachePoint` entries; the other Bedrock families (Nova,
  // Llama, ...) reject them, and an id names its family in every shape it comes in (plain foundation
  // ids, inference profiles, ARNs), so a substring check is the reliable discriminator.
  let breakpoints = request.cache.as_ref().is_some_and(|cache| cache.breakpoints)
    && request.model.contains("anthropic");
  let mut body = Map::new();
  let (system, conversation) = split_leading_instructions(&request.conversation)?;
  if !system.is_empty() {
    body.insert("system".into(), Value::Array(system));
  }
  body.insert("messages".into(), Value::Array(render_messages(conversation)?));
  if !request.tools.is_empty() {
    let mut tool_config = Map::new();
    tool_config
      .insert("tools".into(), Value::Array(request.tools.iter().map(render_tool).collect()));
    match request.tool_choice {
      Some(ToolChoice::None) | None => {}
      Some(choice) => {
        tool_config.insert("toolChoice".into(), render_tool_choice(choice));
      }
    }
    body.insert("toolConfig".into(), Value::Object(tool_config));
  }
  if let Some(max_tokens) = request.max_output_tokens {
    body.insert("inferenceConfig".into(), json!({"maxTokens": max_tokens}));
  }
  if let Some(reasoning) = &request.reasoning
    && let Some(fields) = render_reasoning(reasoning)?
  {
    body.insert("additionalModelRequestFields".into(), fields);
  }
  if let Some(budget_tokens) = body
    .get("additionalModelRequestFields")
    .and_then(|fields| fields.pointer("/thinking/budget_tokens"))
    .and_then(Value::as_u64)
    && let Some(max_tokens) = request.max_output_tokens
  {
    // The model refuses to answer at or below the tokens it thinks with: a cap that tight is raised
    // instead of failing the call.
    body.insert(
      "inferenceConfig".into(),
      json!({
        "maxTokens": if max_tokens > budget_tokens { max_tokens } else { budget_tokens + ANSWER_HEADROOM }
      }),
    );
  }
  if breakpoints {
    mark_breakpoints(&mut body);
  }
  Ok(Value::Object(body))
}

/// End the cached prefix where this wire takes a marker: after the system blocks, after the tool
/// definitions, and after the content of the last wire message. The context is append-only within a
/// session, so each request's tail becomes the next request's cached prefix.
fn mark_breakpoints(body: &mut Map<String, Value>) {
  append_cache_point(body.get_mut("system"));
  append_cache_point(body.get_mut("toolConfig").and_then(|config| config.get_mut("tools")));
  append_cache_point(
    body
      .get_mut("messages")
      .and_then(|messages| messages.as_array_mut()?.last_mut())
      .and_then(|message| message.get_mut("content")),
  );
}

/// Appends the wire's cache marker to one wire array, when the field is there.
fn append_cache_point(array: Option<&mut Value>) {
  if let Some(array) = array.and_then(Value::as_array_mut) {
    array.push(json!({"cachePoint": {"type": "default"}}));
  }
}

/// The bridge this provider-agnostic wire uses for Anthropic thinking parameters: the tier preset
/// selects extended thinking, `enabled` alone selects adaptive thinking, and omission is the off
/// state. The wire has no summary knob.
fn render_reasoning(config: &ReasoningConfig) -> Result<Option<Value>, Error> {
  if config.summary.is_some() {
    return Err(Error::Build("the converse wire has no reasoning summary axis".to_owned()));
  }
  let thinking = match (config.enabled, config.effort.as_deref()) {
    (Some(false), Some(_)) => {
      return Err(Error::Build(
        "thinking cannot be both disabled and given a depth tier on the converse wire".to_owned(),
      ));
    }
    (Some(false) | None, None) => return Ok(None),
    (_, Some(effort)) => match resolve_tier_budget(effort)? {
      TierBudget::Tokens(budget_tokens) => {
        json!({"type": "enabled", "budget_tokens": budget_tokens})
      }
      TierBudget::Adaptive => json!({"type": "adaptive"}),
    },
    (Some(true), None) => json!({"type": "adaptive"}),
  };
  Ok(Some(json!({"thinking": thinking})))
}

fn split_leading_instructions(conversation: &[Message]) -> Result<(Vec<Value>, &[Message]), Error> {
  let mut blocks: Vec<Value> = Vec::new();
  let mut index = 0;
  while let Some(message) = conversation.get(index) {
    let content = match message {
      Message::System { content, .. } | Message::Developer { content, .. } => content,
      _ => break,
    };
    for block in content {
      match block {
        ContentBlock::Text { text } => blocks.push(json!({"text": text})),
        _ => {
          return Err(Error::Build(
            "a system block can only carry text on the converse wire".to_owned(),
          ));
        }
      }
    }
    index += 1;
  }
  Ok((blocks, &conversation[index..]))
}

fn render_messages(conversation: &[Message]) -> Result<Vec<Value>, Error> {
  let push = |turns: &mut Vec<Value>,
              current: &mut Option<&'static str>,
              blocks: &mut Vec<Value>,
              role: &'static str,
              block: Value| {
    if *current != Some(role) {
      if !blocks.is_empty() {
        turns.push(json!({"role": current.unwrap(), "content": std::mem::take(blocks)}));
      }
      *current = Some(role);
    }
    blocks.push(block);
  };
  let mut turns: Vec<Value> = Vec::new();
  let mut blocks: Vec<Value> = Vec::new();
  let mut current: Option<&'static str> = None;
  let mut pending: Vec<String> = Vec::new();
  for message in conversation {
    match message {
      Message::UpstreamCompaction { .. } => {
        return Err(Error::Build(
          "the converse wire cannot carry a compacted conversation".to_owned(),
        ));
      }
      // Past the leading run an instruction has no place in `system[]`: it rides as user input.
      Message::System { content, .. }
      | Message::Developer { content, .. }
      | Message::User { content, .. } => {
        for block in render_user_blocks(content)? {
          push(&mut turns, &mut current, &mut blocks, "user", block);
        }
      }
      Message::Assistant { content, .. } => {
        for block in render_assistant_blocks(content)? {
          push(&mut turns, &mut current, &mut blocks, "assistant", block);
        }
      }
      Message::Reasoning { plaintext, signature, ciphertext, opaque_kind, .. } => {
        let signature = ReasoningOpaqueKind::matching(*opaque_kind, ReasoningOpaqueKind::BedrockSignature, signature);
        let ciphertext = ReasoningOpaqueKind::matching(*opaque_kind, ReasoningOpaqueKind::BedrockRedacted, ciphertext);
        let content = if !ciphertext.is_empty() {
          json!({"redactedContent": ciphertext})
        } else if !plaintext.is_empty() || !signature.is_empty() {
          let mut reasoning_text = json!({"text": plaintext});
          if !signature.is_empty() {
            reasoning_text["signature"] = json!(signature);
          }
          json!({"reasoningText": reasoning_text})
        } else {
          continue;
        };
        push(
          &mut turns,
          &mut current,
          &mut blocks,
          "assistant",
          json!({"reasoningContent": content}),
        );
      }
      Message::ToolUse { call_id, name, arguments, .. } => {
        if call_id.is_empty() {
          return Err(Error::Build(
            "a tool use needs a `toolUseId` on the converse wire".to_owned(),
          ));
        }
        pending.push(call_id.clone());
        push(
          &mut turns,
          &mut current,
          &mut blocks,
          "assistant",
          json!({"toolUse": {"toolUseId": call_id, "name": name, "input": arguments}}),
        );
      }
      Message::ToolResult { call_id, content, .. } => {
        let Some(index) = pending.iter().position(|id| id == call_id) else {
          return Err(Error::Build(format!(
            "tool result references an unknown `toolUseId`: {call_id}"
          )));
        };
        pending.remove(index);
        push(
          &mut turns,
          &mut current,
          &mut blocks,
          "user",
          json!({
            "toolResult": {
              "toolUseId": call_id,
              "status": "success",
              "content": render_tool_result_blocks(content),
            }
          }),
        );
      }
    }
  }
  if !pending.is_empty() {
    return Err(Error::Build(format!(
      "tool uses without a matching tool result on the converse wire: {}",
      pending.join(", ")
    )));
  }
  if !blocks.is_empty() {
    turns.push(json!({"role": current.unwrap(), "content": blocks}));
  }
  Ok(turns)
}

fn render_user_blocks(content: &[ContentBlock]) -> Result<Vec<Value>, Error> {
  let mut blocks: Vec<Value> = Vec::new();
  for block in content {
    match block {
      ContentBlock::Text { text } => blocks.push(json!({"text": text})),
      ContentBlock::Image { mime_type, data_base64 } => {
        let Some(format) = resolve_image_format(mime_type) else {
          return Err(Error::Build(format!(
            "image media type cannot be expressed on the converse wire: {mime_type}"
          )));
        };
        blocks.push(json!({"image": {"format": format, "source": {"bytes": data_base64}}}));
      }
    }
  }
  Ok(blocks)
}

fn render_assistant_blocks(content: &[ContentBlock]) -> Result<Vec<Value>, Error> {
  let mut blocks: Vec<Value> = Vec::new();
  for block in content {
    match block {
      ContentBlock::Text { text } => blocks.push(json!({"text": text})),
      ContentBlock::Image { .. } => {
        return Err(Error::Build(
          "an assistant turn cannot carry images on the converse wire".to_owned(),
        ));
      }
    }
  }
  Ok(blocks)
}

fn render_tool(tool: &Tool) -> Value {
  json!({
    "toolSpec": {
      "name": tool.name,
      "description": tool.description,
      "inputSchema": {"json": tool.input_schema},
    }
  })
}

fn render_tool_choice(choice: ToolChoice) -> Value {
  match choice {
    ToolChoice::Auto => json!({"auto": {}}),
    ToolChoice::Required => json!({"any": {}}),
    ToolChoice::None => unreachable!("tool choice `none` is omitted on the converse wire"),
  }
}

fn render_tool_result_blocks(content: &Value) -> Value {
  let block = match content {
    Value::Object(_) => json!({"json": content}),
    Value::String(text) => json!({"text": text}),
    other => json!({"text": other.to_string()}),
  };
  Value::Array(vec![block])
}

fn resolve_image_format(mime_type: &str) -> Option<&'static str> {
  match mime_type {
    "image/png" => Some("png"),
    "image/jpeg" => Some("jpeg"),
    "image/gif" => Some("gif"),
    "image/webp" => Some("webp"),
    _ => None,
  }
}

/// The streaming endpoint for the same request body.
pub fn resolve_stream_path(path: &str) -> String {
  if path.ends_with("/converse") { format!("{path}-stream") } else { path.to_owned() }
}

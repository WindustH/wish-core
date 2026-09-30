//! Anthropic Messages request wire.
//!
//! Conversions:
//! - Leading `System` messages are hoisted into the top-level `system` field, joined with
//!   newlines. A `System` or `Developer` message past the leading run becomes a `user` turn in
//!   place.
//! - Adjacent same-role messages merge into one turn, `ToolUse` folds into the assistant turn that
//!   requests it, and `ToolResult` messages merge into the single user turn that follows (arriving
//!   in call order).
//! - `Reasoning` replays as `thinking` + `signature`, or as `redacted_thinking` when it carries a
//!   `ciphertext` instead of text, and it leads the assistant turn.
//! - Consecutive assistant messages are the segments of one response resumed by automatic output
//!   continuation: their texts join into one text block, and thinking stored after the turn's
//!   content is dropped, or moved to the front when the turn did not open with thinking.
//! - A tool result's `content` is a plain string: a string payload passes through, anything else
//!   is stringified.
//! - `ReasoningConfig` renders the control this endpoint documents: Claude's own `thinking` object
//!   in the default mode, where a depth tier becomes the preset budget, and a vendor's
//!   `thinking.type` switch or `output_config.effort` tier in the vendor modes [`MessagesApiCompatMode`]
//!   knows.
//! - `PromptCache::breakpoints` marks the three breakpoints this wire takes: the system block,
//!   the last tool definition, and the tail of the last wire message.
//! - Streaming is one body field: `stream: true`, and the reply arrives as SSE.
//!
//! Constraints:
//! - `messages` must open with a `user` turn and strictly alternate user / assistant; consecutive
//!   same-role turns are rejected by the wire.
//! - There is no system role inside `messages`: instructions belong in the top-level `system`
//!   field, which only takes text.
//! - Tool results are `tool_result` blocks inside a `user` turn, never a `tool` role, and each one
//!   answers a `tool_use` id from the assistant turn before it.
//! - A thinking block and its `signature` must go back unmodified: stripping or editing either is
//!   an immediate `400 INVALID_REQUEST` (`Thinking block signature mismatch`). Thinking blocks lead
//!   the assistant turn.
//! - `max_tokens` is required, and sampling temperature has to stay unset while thinking is active.
//!
//! Trade-offs:
//! - A `max_output_tokens` cap at or below the thinking budget is raised to leave room for the
//!   answer.
//! - `ToolChoice::None` has no wire spelling and is rejected rather than silently dropped.
//! - Call/result pairing is validated in both directions, so a dangling `ToolUse` or an orphan
//!   result is an error instead of a malformed request.
//! - A `Developer` message has no role of its own on this wire, so in the leading run its text joins
//!   the same `system` field, and past it the model reads it as user input.
//! - `PromptCache::key` has no spelling on this wire, so it is not sent.
//! - Foreign signed thinking is omitted; its proof cannot authenticate an Anthropic block.
//!   Unsigned plaintext thinking remains available for compatible endpoints.
//! - Vendors that reimplement the wire patched their own thinking control in, so each patch is its
//!   own mode and an axis a mode has no spelling for is rejected, never dropped. DeepSeek's switch
//!   and effort spelling follow the controls its chat wire takes, because its messages wire takes
//!   neither; Qwen's wire replaces the budget with `output_config.effort` entirely.
//! - The wire's `is_error` flag is never sent (the model keeps its `false` default), reasoning
//!   provider/model binding of otherwise matching signatures is not tracked, and
//!   `prompt_cache_retention` is not modeled.
//! - A breakpoint needs a block to live on, so a cached prompt head is sent as a one-block `system`
//!   array instead of the plain joined string.

use crate::protocol::error::Error;
use crate::protocol::model_use::request::{
  AlternatingTurns, lift_cap_above_budget, render_claude_thinking, render_thinking_switch,
  split_leading_instructions,
};
use crate::protocol::model_use::tool::tool_result_text;
use crate::protocol::{
  ContentBlock, Message, ReasoningConfig, ReasoningOpaqueKind, Request, Tool, ToolChoice,
};
use serde_json::{Map, Value, json};

pub use crate::protocol::model_use::mode::MessagesApiCompatMode;

/// Headers every call carries, besides auth and `content-type`.
pub const HEADERS: &[(&str, &str)] = &[("anthropic-version", "2023-06-01")];

pub fn render(request: &Request, mode: MessagesApiCompatMode) -> Result<Value, Error> {
  let max_tokens = request.max_output_tokens.ok_or_else(|| {
    Error::Build("max_tokens is required on the anthropic messages wire".to_owned())
  })?;
  render_body(request, mode, Some(max_tokens))
}

pub(crate) fn render_for_token_count(request: &Request) -> Result<Value, Error> {
  render_body(request, MessagesApiCompatMode::Official, None)
}

// Counting uses exactly the same inputs, without an output cap or streaming controls.
fn render_body(
  request: &Request,
  mode: MessagesApiCompatMode,
  max_tokens: Option<u64>,
) -> Result<Value, Error> {
  if request.tool_choice == Some(ToolChoice::None) {
    return Err(Error::Build(
      "tool_choice `none` cannot be expressed on the anthropic messages wire".to_owned(),
    ));
  }

  let (system, rest) = split_leading_instructions(
    &request.conversation,
    "a system instruction can only carry text blocks on the anthropic messages wire",
  )?;
  let messages = render_messages(rest)?;
  if messages.is_empty() {
    return Err(Error::Build("request has no messages".to_owned()));
  }

  let breakpoints = request.cache.as_ref().is_some_and(|cache| cache.breakpoints);
  let mut body = Map::new();
  body.insert("model".into(), json!(request.model));
  if let Some(max_tokens) = max_tokens {
    body.insert("max_tokens".into(), json!(max_tokens));
  }
  if !system.is_empty() {
    let system: Vec<String> = system.iter().map(|text| text.join("\n")).collect();
    let system = system.join("\n");
    if breakpoints {
      body.insert("system".into(), json!([{"type": "text", "text": system}]));
    } else {
      body.insert("system".into(), json!(system));
    }
  }
  body.insert("messages".into(), Value::Array(messages));
  if request.stream && max_tokens.is_some() {
    body.insert("stream".into(), json!(true));
  }
  if let Some(reasoning) = &request.reasoning {
    render_reasoning(mode, reasoning, &mut body)?;
  }
  if let Some(max_tokens) = max_tokens
    && let Some(budget_tokens) = body
      .get("thinking")
      .and_then(|thinking| thinking.get("budget_tokens"))
      .and_then(Value::as_u64)
  {
    body.insert("max_tokens".into(), json!(lift_cap_above_budget(max_tokens, budget_tokens)));
  }
  if !request.tools.is_empty() {
    let tools: Vec<Value> = request.tools.iter().map(render_tool).collect();
    body.insert("tools".into(), Value::Array(tools));
    if let Some(choice) = request.tool_choice {
      body.insert("tool_choice".into(), render_tool_choice(choice));
    }
  }
  if breakpoints {
    mark_breakpoints(&mut body);
  }
  Ok(Value::Object(body))
}

/// The thinking controls this endpoint takes: Claude's own `thinking` object, and for a vendor mode
/// the single control its wire documents. A depth tier travels as a tier word where the wire names
/// one and as the preset budget where it only budgets, and an axis with no spelling is rejected
/// rather than dropped.
///
/// Claude's off state is plain omission (it has no `disabled` member), a vendor switch spells it,
/// and the vendors whose thinking is always on reject it instead.
fn render_reasoning(
  mode: MessagesApiCompatMode,
  config: &ReasoningConfig,
  body: &mut Map<String, Value>,
) -> Result<(), Error> {
  if config.summary.is_some() {
    return Err(Error::Build(
      "the anthropic messages wire has no reasoning summary axis".to_owned(),
    ));
  }
  if config.enabled == Some(false) && config.effort.is_some() {
    return Err(Error::Build(
      "reasoning cannot be both disabled and given an effort on the anthropic messages wire"
        .to_owned(),
    ));
  }
  match mode {
    MessagesApiCompatMode::Official => {
      if let Some(thinking) = render_claude_thinking(config)? {
        body.insert("thinking".into(), thinking);
      }
    }
    MessagesApiCompatMode::DeepSeek => {
      if let Some(enabled) = config.enabled {
        body.insert("thinking".into(), render_thinking_switch(enabled));
      }
      insert_output_effort(config, body);
    }
    MessagesApiCompatMode::Zai | MessagesApiCompatMode::Mimo => {
      if config.effort.is_some() {
        return Err(Error::Build("a `thinking.type` messages wire has no effort axis".to_owned()));
      }
      if let Some(enabled) = config.enabled {
        body.insert("thinking".into(), render_thinking_switch(enabled));
      }
    }
    MessagesApiCompatMode::MiniMax => {
      if config.effort.is_some() {
        return Err(Error::Build("the minimax messages wire takes no effort".to_owned()));
      }
      match config.enabled {
        Some(true) => {
          body.insert("thinking".into(), json!({"type": "adaptive"}));
        }
        Some(false) => {
          body.insert("thinking".into(), json!({"type": "disabled"}));
        }
        None => {}
      }
    }
    MessagesApiCompatMode::Kimi => {
      if config.enabled.is_some() {
        return Err(Error::Build(
          "the kimi messages wire keeps thinking on at all times".to_owned(),
        ));
      }
      insert_output_effort(config, body);
    }
    MessagesApiCompatMode::Qwen => {
      if config.enabled.is_some() {
        return Err(Error::Build(
          "the bailian messages wire steers thinking with `output_config.effort` alone".to_owned(),
        ));
      }
      insert_output_effort(config, body);
    }
    MessagesApiCompatMode::TokenHub => {
      if config.enabled.is_some() || config.effort.is_some() {
        return Err(Error::Build(
          "the tokenhub messages wire has no reasoning controls".to_owned(),
        ));
      }
    }
  }
  Ok(())
}

/// The caller's tier word as `output_config.effort`, the vendors' own effort field, when it gave
/// one.
fn insert_output_effort(config: &ReasoningConfig, body: &mut Map<String, Value>) {
  if let Some(effort) = &config.effort {
    body.insert("output_config".into(), json!({"effort": effort.to_lowercase()}));
  }
}

fn render_messages(conversation: &[Message]) -> Result<Vec<Value>, Error> {
  let mut turns = AlternatingTurns::default();
  // The calls of the last assistant turn that still wait for their results.
  let mut pending: Vec<String> = Vec::new();
  let mut rest = conversation;
  while let [message, ..] = rest {
    let used = match message {
      Message::UpstreamCompaction { .. } => {
        return Err(Error::Build(
          "the anthropic messages wire cannot carry a compacted conversation".to_owned(),
        ));
      }
      // Past the leading run an instruction has no place in `system`: it rides as user input.
      Message::System { content, .. }
      | Message::Developer { content, .. }
      | Message::User { content, .. } => {
        if let Some(call_id) = pending.first() {
          return Err(build_missing_result_error(call_id));
        }
        turns.push("user", render_user_blocks(content)?);
        1
      }
      Message::Reasoning { .. } | Message::Assistant { .. } | Message::ToolUse { .. } => {
        if let Some(call_id) = pending.first() {
          return Err(build_missing_result_error(call_id));
        }
        let (blocks, used) = render_assistant_turn(rest, &mut pending)?;
        turns.push("assistant", blocks);
        used
      }
      Message::ToolResult { .. } => {
        let blocks = render_tool_results(rest, &pending)?;
        let used = blocks.len();
        pending.clear();
        turns.push("user", blocks);
        used
      }
    };
    rest = &rest[used..];
  }
  if let Some(call_id) = pending.first() {
    return Err(build_missing_result_error(call_id));
  }
  turns.finish_from_user(
    "content",
    "the first turn must be a user message on the anthropic messages wire",
  )
}

/// The assistant turn the run of reasoning, text and calls at the head of `messages` makes, and how
/// many messages it spans; each call joins `pending` until its result arrives.
fn render_assistant_turn(
  messages: &[Message],
  pending: &mut Vec<String>,
) -> Result<(Vec<Value>, usize), Error> {
  let mut blocks: Vec<Value> = Vec::new();
  let mut used = 0;
  let mut seen_content = false;
  let mut opens_with_thinking = false;
  for message in messages {
    match message {
      Message::Reasoning { plaintext, signature, ciphertext, opaque_kind, .. } => {
        if let Some(block) =
          render_replayed_reasoning(plaintext, signature, ciphertext, *opaque_kind)
        {
          // Thinking after content is where output continuation resumed a capped response. The
          // turn keeps the thinking it opened with and drops the resumed one; a turn that opened
          // without any takes it at the front, since a turn that calls tools has to open with
          // thinking.
          if !seen_content {
            blocks.push(block);
            opens_with_thinking = true;
          } else if !opens_with_thinking {
            blocks.insert(0, block);
            opens_with_thinking = true;
          }
        }
      }
      Message::Assistant { content, .. } => {
        seen_content = true;
        append_continued_text(&mut blocks, render_assistant_blocks(content)?);
      }
      Message::ToolUse { call_id, name, arguments, .. } => {
        seen_content = true;
        pending.push(call_id.clone());
        blocks.push(json!({"type": "tool_use", "id": call_id, "name": name, "input": arguments}));
      }
      _ => break,
    }
    used += 1;
  }
  Ok((blocks, used))
}

/// Appends one assistant message's blocks to its turn. Consecutive assistant messages are the
/// segments of one continued response: the resumed text joins the text it continues.
fn append_continued_text(blocks: &mut Vec<Value>, mut rendered: Vec<Value>) {
  if let (Some(last), Some(first)) = (blocks.last_mut(), rendered.first())
    && last["type"] == "text"
    && first["type"] == "text"
  {
    let joined = format!(
      "{}{}",
      last["text"].as_str().unwrap_or_default(),
      first["text"].as_str().unwrap_or_default()
    );
    last["text"] = json!(joined);
    rendered.remove(0);
  }
  blocks.extend(rendered);
}

/// The results at the head of `results`, which must answer the `pending` calls one by one and in
/// order, as the blocks of the user turn that follows the calls.
fn render_tool_results(results: &[Message], pending: &[String]) -> Result<Vec<Value>, Error> {
  if pending.is_empty() {
    return Err(Error::Build(
      "a tool result has no matching tool call on the anthropic messages wire".to_owned(),
    ));
  }
  let mut blocks: Vec<Value> = Vec::new();
  while let Some(Message::ToolResult { call_id, content, .. }) = results.get(blocks.len()) {
    match pending.get(blocks.len()) {
      Some(expected) if expected == call_id => {}
      Some(expected) => {
        return Err(Error::Build(format!(
          "tool result for `{call_id}` does not match the expected tool call `{expected}` on the anthropic messages wire"
        )));
      }
      None => {
        return Err(Error::Build(format!(
          "tool result for `{call_id}` has no matching tool call on the anthropic messages wire"
        )));
      }
    }
    blocks.push(render_tool_result(call_id, content));
  }
  if blocks.len() != pending.len() {
    return Err(build_missing_result_error(&pending[blocks.len()]));
  }
  Ok(blocks)
}

fn build_missing_result_error(call_id: &str) -> Error {
  Error::Build(format!(
    "tool call `{call_id}` is not followed by its tool result on the anthropic messages wire"
  ))
}

fn render_user_blocks(content: &[ContentBlock]) -> Result<Vec<Value>, Error> {
  let mut blocks: Vec<Value> = Vec::new();
  for block in content {
    match block {
      ContentBlock::Text { text: block_text } => {
        blocks.push(json!({"type": "text", "text": block_text}));
      }
      ContentBlock::Image { mime_type, data_base64 } => {
        blocks.push(json!({
          "type": "image",
          "source": {"type": "base64", "media_type": mime_type, "data": data_base64},
        }));
      }
    }
  }
  Ok(blocks)
}

fn render_assistant_blocks(content: &[ContentBlock]) -> Result<Vec<Value>, Error> {
  let mut blocks: Vec<Value> = Vec::new();
  for block in content {
    match block {
      ContentBlock::Text { text: block_text } => {
        blocks.push(json!({"type": "text", "text": block_text}));
      }
      ContentBlock::Image { .. } => {
        return Err(Error::Build(
          "an assistant message cannot carry images on the anthropic messages wire".to_owned(),
        ));
      }
    }
  }
  Ok(blocks)
}

/// One stored reasoning message as the thinking block it replays as, or nothing when it carries
/// nothing this wire takes back.
///
/// Only same-format proofs are valid. A foreign signed block cannot be rewritten as unsigned native
/// thinking without risking an upstream signature mismatch, so it is omitted, while unsigned
/// plaintext thinking stays available to the compatible endpoints.
fn render_replayed_reasoning(
  plaintext: &str,
  signature: &str,
  ciphertext: &str,
  opaque_kind: Option<ReasoningOpaqueKind>,
) -> Option<Value> {
  let unsigned_plaintext = opaque_kind.is_none() && signature.is_empty() && ciphertext.is_empty();
  let signature = ReasoningOpaqueKind::material_if_kind(
    opaque_kind,
    ReasoningOpaqueKind::AnthropicSignature,
    signature,
  );
  let ciphertext = ReasoningOpaqueKind::material_if_kind(
    opaque_kind,
    ReasoningOpaqueKind::AnthropicRedacted,
    ciphertext,
  );
  let plaintext =
    if unsigned_plaintext || opaque_kind == Some(ReasoningOpaqueKind::AnthropicSignature) {
      plaintext
    } else {
      ""
    };
  render_reasoning_block(plaintext, signature, ciphertext)
}

fn render_reasoning_block(plaintext: &str, signature: &str, ciphertext: &str) -> Option<Value> {
  if !ciphertext.is_empty() {
    return Some(json!({"type": "redacted_thinking", "data": ciphertext}));
  }
  if plaintext.is_empty() && signature.is_empty() {
    return None;
  }
  let mut thinking = json!({"type": "thinking", "thinking": plaintext});
  if !signature.is_empty() {
    thinking["signature"] = json!(signature);
  }
  Some(thinking)
}

fn render_tool_result(call_id: &str, content: &Value) -> Value {
  json!({
    "type": "tool_result",
    "tool_use_id": call_id,
    "content": tool_result_text(content),
  })
}

/// End the cached prefix where this wire takes a breakpoint: after the system block, after the
/// tool definitions, and after the last block of the last wire message. The context is append-only
/// within a session, so each request's tail becomes the next request's cached prefix and only the
/// delta appended since is written.
fn mark_breakpoints(body: &mut Map<String, Value>) {
  mark_tail(body.get_mut("system"));
  mark_tail(body.get_mut("tools"));
  mark_tail(
    body
      .get_mut("messages")
      .and_then(|messages| messages.as_array_mut()?.last_mut())
      .and_then(|message| message.get_mut("content")),
  );
}

/// Marks the last block of one wire array as a cache breakpoint, when the field is there and holds
/// an object to hang the marker on.
fn mark_tail(array: Option<&mut Value>) {
  let Some(Value::Object(block)) = array.and_then(|value| value.as_array_mut()?.last_mut()) else {
    return;
  };
  block.insert("cache_control".into(), json!({"type": "ephemeral"}));
}

fn render_tool(tool: &Tool) -> Value {
  json!({
    "name": tool.name,
    "description": tool.description,
    "input_schema": tool.input_schema,
  })
}

fn render_tool_choice(choice: ToolChoice) -> Value {
  match choice {
    ToolChoice::Auto => json!({"type": "auto"}),
    ToolChoice::Required => json!({"type": "any"}),
    ToolChoice::None => unreachable!("tool_choice `none` is rejected before rendering"),
  }
}

//! Anthropic Messages request wire.
//!
//! Conversions:
//! - Leading `System` messages are hoisted into the top-level `system` field, joined with
//!   newlines.
//! - Adjacent same-role messages merge into one turn, `ToolUse` folds into the assistant turn that
//!   requests it, and `ToolResult` messages merge into the single user turn that follows (arriving
//!   in call order).
//! - `Reasoning` replays as `thinking` + `signature`, or as `redacted_thinking` when it carries a
//!   `ciphertext` instead of text, and it leads the assistant turn.
//! - Tool results stay a plain string when text-only, otherwise an image/text block array.
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
//!   field, which only takes text, and a `System` or `Developer` message past the leading run is
//!   rejected.
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
//! - A `Developer` message has no role of its own on this wire, so its text joins the same `system`
//!   field.
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
use crate::protocol::ReasoningOpaqueKind;
use crate::protocol::model_use::request::{ANSWER_HEADROOM, TierBudget, resolve_tier_budget};
use crate::protocol::{ContentBlock, Message, ReasoningConfig, Request, Tool, ToolChoice};
use serde_json::{Map, Value, json};

/// Headers every call carries, besides auth and `content-type`.
pub const HEADERS: &[(&str, &str)] = &[("anthropic-version", "2023-06-01")];

/// Which reasoning extension this Messages endpoint speaks on top of the official wire.
///
/// The official wire steers thinking with `adaptive` or with a budget that follows the shared tier
/// preset; every vendor that reimplements it patches in its own control, so each patch is its own
/// mode however small the difference between two of them is. `Official` is Claude's own shape.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MessagesApiCompatMode {
  /// Claude's own shape: the tier preset (`adaptive`, or `enabled` with a budget).
  #[default]
  Official,
  /// `thinking{type: enabled|disabled}` plus `output_config.effort`.
  DeepSeek,
  /// `thinking{type: enabled|disabled}`; nothing else.
  Zai,
  /// `output_config.effort` only: the k3 wire is always on and takes no `thinking` object.
  Kimi,
  /// `output_config.effort` alone: it is the native replacement for the legacy thinking budget.
  Qwen,
  /// `thinking{type: adaptive|disabled}`; nothing else.
  MiniMax,
  /// `thinking{type: enabled|disabled}`; nothing else.
  Mimo,
  /// No documented controls: thinking blocks are read, nothing is sent.
  TokenHub,
}

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

  let (system, rest) = split_leading_instructions(&request.conversation)?;
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
    // The model refuses to answer at or below the tokens it thinks with: a cap that tight is raised
    // instead of failing the call.
    body.insert(
      "max_tokens".into(),
      json!(if max_tokens > budget_tokens { max_tokens } else { budget_tokens + ANSWER_HEADROOM }),
    );
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
      let thinking = match (config.enabled, config.effort.as_deref()) {
        (Some(false) | None, None) => return Ok(()),
        (_, Some(effort)) => match resolve_tier_budget(effort)? {
          TierBudget::Tokens(budget_tokens) => {
            json!({"type": "enabled", "budget_tokens": budget_tokens})
          }
          TierBudget::Adaptive => json!({"type": "adaptive"}),
        },
        (Some(true), None) => json!({"type": "adaptive"}),
      };
      body.insert("thinking".into(), thinking);
    }
    MessagesApiCompatMode::DeepSeek => {
      if let Some(enabled) = config.enabled {
        body
          .insert("thinking".into(), json!({"type": if enabled { "enabled" } else { "disabled" }}));
      }
      if let Some(effort) = &config.effort {
        body.insert("output_config".into(), json!({"effort": effort.to_lowercase()}));
      }
    }
    MessagesApiCompatMode::Zai | MessagesApiCompatMode::Mimo => {
      if config.effort.is_some() {
        return Err(Error::Build("a `thinking.type` messages wire has no effort axis".to_owned()));
      }
      if let Some(enabled) = config.enabled {
        body
          .insert("thinking".into(), json!({"type": if enabled { "enabled" } else { "disabled" }}));
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
      if let Some(effort) = &config.effort {
        body.insert("output_config".into(), json!({"effort": effort.to_lowercase()}));
      }
    }
    MessagesApiCompatMode::Qwen => {
      if config.enabled.is_some() {
        return Err(Error::Build(
          "the bailian messages wire steers thinking with `output_config.effort` alone".to_owned(),
        ));
      }
      if let Some(effort) = &config.effort {
        body.insert("output_config".into(), json!({"effort": effort.to_lowercase()}));
      }
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

fn split_leading_instructions(
  conversation: &[Message],
) -> Result<(Vec<String>, &[Message]), Error> {
  let mut system: Vec<String> = Vec::new();
  let mut index = 0;
  while let Some(message) = conversation.get(index) {
    let content = match message {
      Message::System { content, .. } | Message::Developer { content, .. } => content,
      _ => break,
    };
    let mut text: Vec<&str> = Vec::new();
    for block in content {
      match block {
        ContentBlock::Text { text: block_text } => text.push(block_text),
        _ => {
          return Err(Error::Build(
            "a system instruction can only carry text blocks on the anthropic messages wire"
              .to_owned(),
          ));
        }
      }
    }
    system.push(text.join("\n"));
    index += 1;
  }
  Ok((system, &conversation[index..]))
}

fn render_messages(conversation: &[Message]) -> Result<Vec<Value>, Error> {
  let mut turns: Vec<(String, Vec<Value>)> = Vec::new();
  let mut pending: Vec<String> = Vec::new();
  let mut index = 0;
  while index < conversation.len() {
    match &conversation[index] {
      Message::System { .. } | Message::Developer { .. } => {
        return Err(Error::Build(
          "a system instruction must lead the conversation on the anthropic messages wire"
            .to_owned(),
        ));
      }
      Message::UpstreamCompaction { .. } => {
        return Err(Error::Build(
          "the anthropic messages wire cannot carry a compacted conversation".to_owned(),
        ));
      }
      Message::User { content, .. } => {
        if let Some(call_id) = pending.first() {
          return Err(build_missing_result_error(call_id));
        }
        push_turn(&mut turns, "user", render_user_blocks(content)?);
        index += 1;
      }
      Message::Reasoning { .. } | Message::Assistant { .. } | Message::ToolUse { .. } => {
        if let Some(call_id) = pending.first() {
          return Err(build_missing_result_error(call_id));
        }
        let mut blocks: Vec<Value> = Vec::new();
        let mut used = 0;
        let mut seen_content = false;
        while let Some(message) = conversation.get(index + used) {
          match message {
            Message::Reasoning { plaintext, signature: proof, ciphertext, opaque_kind, .. } => {
              let unsigned_plaintext = opaque_kind.is_none() && proof.is_empty() && ciphertext.is_empty();
              let signature = ReasoningOpaqueKind::matching(*opaque_kind, ReasoningOpaqueKind::AnthropicSignature, proof);
              let ciphertext = ReasoningOpaqueKind::matching(*opaque_kind, ReasoningOpaqueKind::AnthropicRedacted, ciphertext);
              if seen_content {
                return Err(Error::Build(
                  "a reasoning message must lead its assistant turn on the anthropic messages wire"
                    .to_owned(),
                ));
              }
              // Only same-format proofs are valid. A foreign signed block cannot be rewritten
              // as unsigned native thinking without risking an upstream signature mismatch.
              let plaintext = if unsigned_plaintext
                || *opaque_kind == Some(ReasoningOpaqueKind::AnthropicSignature)
              {
                plaintext.as_str()
              } else {
                ""
              };
              if let Some(block) = render_reasoning_block(plaintext, signature, ciphertext) {
                blocks.push(block);
              }
            }
            Message::Assistant { content, .. } => {
              seen_content = true;
              blocks.extend(render_assistant_blocks(content)?);
            }
            Message::ToolUse { call_id, name, arguments, .. } => {
              seen_content = true;
              pending.push(call_id.clone());
              blocks
                .push(json!({"type": "tool_use", "id": call_id, "name": name, "input": arguments}));
            }
            _ => break,
          }
          used += 1;
        }
        push_turn(&mut turns, "assistant", blocks);
        index += used;
      }
      Message::ToolResult { .. } => {
        if pending.is_empty() {
          return Err(Error::Build(
            "a tool result has no matching tool call on the anthropic messages wire".to_owned(),
          ));
        }
        let mut blocks: Vec<Value> = Vec::new();
        let mut used = 0;
        while let Some(Message::ToolResult { call_id, content, .. }) =
          conversation.get(index + used)
        {
          match pending.get(used) {
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
          used += 1;
        }
        if used != pending.len() {
          return Err(build_missing_result_error(&pending[used]));
        }
        pending.clear();
        push_turn(&mut turns, "user", blocks);
        index += used;
      }
    }
  }
  if let Some(call_id) = pending.first() {
    return Err(build_missing_result_error(call_id));
  }
  match turns.first() {
    Some((role, _)) if role != "user" => Err(Error::Build(
      "the first turn must be a user message on the anthropic messages wire".to_owned(),
    )),
    _ => {
      Ok(turns.into_iter().map(|(role, blocks)| json!({"role": role, "content": blocks})).collect())
    }
  }
}

fn build_missing_result_error(call_id: &str) -> Error {
  Error::Build(format!(
    "tool call `{call_id}` is not followed by its tool result on the anthropic messages wire"
  ))
}

fn push_turn(turns: &mut Vec<(String, Vec<Value>)>, role: &str, mut blocks: Vec<Value>) {
  if blocks.is_empty() {
    return;
  }
  if let Some((last_role, last_blocks)) = turns.last_mut()
    && last_role == role
  {
    last_blocks.append(&mut blocks);
    return;
  }
  turns.push((role.to_owned(), blocks));
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
  let content = match content {
    Value::String(text) => text.clone(),
    other => other.to_string(),
  };
  json!({
    "type": "tool_result",
    "tool_use_id": call_id,
    "content": content,
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

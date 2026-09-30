//! OpenAI Chat Completions request wire.
//!
//! Conversions:
//! - `System` / `Developer` ride as ordinary messages, unlike the wires that hoist instructions into
//!   a top-level field. Which roles they take and where depends on the vendor (see
//!   [`InstructionPlacement`]): the official wire keeps both roles anywhere; vendors without a
//!   `developer` role but with `system` anywhere get `system` for both; the rest get one leading
//!   `system` message, the leading instruction run merged, and every later instruction as user
//!   input.
//! - `User` content is a plain string when text-only, otherwise a parts array with images nested
//!   under `image_url.url` as data URLs.
//! - `Assistant` text merges with the `ToolUse` messages that follow it into one assistant message
//!   with `tool_calls` (`content: null` when there is no text); `ToolUse` messages with no text
//!   ahead of them are one assistant turn too - consecutive ones (with replayed `Reasoning`
//!   between) share a single assistant message, because the wire insists every `tool_calls`
//!   message is followed by its tool replies. A turn that opened with text takes only the calls
//!   right after it: a `Reasoning` there ends the turn and leads the next one.
//! - `ToolResult` becomes `{"role": "tool", "tool_call_id", "content"}`, passing a string payload
//!   through and stringifying anything else.
//! - `Reasoning` is replayed on the assistant turn that follows it: under `reasoning_content` for
//!   the vendor extensions, or as a leading `thinking` chunk of `content` on Mistral's wire.
//!   `Official` keeps the wire plain, and a mode sends whatever knob its vendor needs for the
//!   history to survive.
//! - `ReasoningConfig` maps to the few controls the chat wire has: `effort` becomes the lowercase
//!   `reasoning_effort` word where the vendor documents it, `enabled` is the vendor's own switch, and
//!   a summary is rejected everywhere.
//! - Caching is automatic on this wire, so the only control is `PromptCache::key`, sent as
//!   `prompt_cache_key` to route a prefix to a machine that has it cached; breakpoints have no
//!   spelling here.
//!
//! Constraints:
//! - Stateless: every turn has to carry the whole history again.
//! - Only the official wire takes `developer`: DeepSeek refuses it with 422 and Zhipu with 400,
//!   and most vendors' role lists leave it out. Several take `system` only as the first message
//!   (Qwen, TokenHub, Mistral after an assistant or tool turn, open chat templates such as Qwen3.5
//!   that raise on a later one, or DeepSeek-V3's and MiniMax-M2's that move or drop it).
//! - A `tool` message answers the `tool_calls` of the assistant turn before it and repeats their
//!   `tool_call_id`.
//! - The official wire has nowhere to send reasoning back: a raw thinking block inside `messages`
//!   is rejected, so reasoning can only be steered there.
//!
//! Trade-offs:
//! - `store: false` is always sent and `max_output_tokens` maps to `max_completion_tokens`, except on
//!   Mistral's wire, which takes neither `store` nor `stream_options`, spells the cap `max_tokens`,
//!   and calls "must call a tool" `any`. Tools use the nested
//!   `{"type": "function", "function": ...}` shape without `strict`; streaming adds `stream: true`
//!   plus `stream_options.include_usage` (the usage chunk only arrives with it).
//! - The reasoning field and the knobs beside it are vendor extensions, not part of the wire: `Official`
//!   reads and sends nothing, and a vendor that honors a keep knob bills the replayed history as
//!   input tokens.
//! - `reasoning_details` arrays and vendors that embed thoughts as `<think>` tags in `content` are
//!   not modelled yet.

use crate::protocol::error::Error;
use crate::protocol::model_use::message::image_data_url;
use crate::protocol::model_use::mistral_chunks::render_thinking_chunk;
use crate::protocol::model_use::mode::REASONING_FIELD;
use crate::protocol::model_use::request::{
  collect_instruction_text, render_thinking_switch, split_leading_instructions,
};
use crate::protocol::model_use::tool::tool_result_text;
use crate::protocol::{ContentBlock, Message, ReasoningConfig, Request, Tool, ToolChoice};
use serde_json::{Map, Value, json};

pub use crate::protocol::model_use::mode::ChatCompletionApiCompatMode;

/// Headers every call carries, besides auth and `content-type`.
pub const HEADERS: &[(&str, &str)] = &[];

pub fn render(request: &Request, mode: ChatCompletionApiCompatMode) -> Result<Value, Error> {
  let mut body = Map::new();
  body.insert("model".into(), json!(request.model));
  if !mode.is_mistral() {
    body.insert("store".into(), json!(false));
  }
  if let Some(key) = request.cache.as_ref().and_then(|cache| cache.key.as_deref()) {
    body.insert("prompt_cache_key".into(), json!(key));
  }
  if request.stream {
    body.insert("stream".into(), json!(true));
    if !mode.is_mistral() {
      // Usage only arrives in the stream when it is asked for.
      body.insert("stream_options".into(), json!({ "include_usage": true }));
    }
  }
  body.insert("messages".into(), Value::Array(render_messages(&request.conversation, mode)?));
  if let Some(max_output_tokens) = request.max_output_tokens {
    let field = if mode.is_mistral() { "max_tokens" } else { "max_completion_tokens" };
    body.insert(field.into(), json!(max_output_tokens));
  }
  render_reasoning(mode, request.reasoning.as_ref(), &mut body)?;
  if !request.tools.is_empty() {
    let tools: Vec<Value> = request.tools.iter().map(render_tool).collect();
    body.insert("tools".into(), Value::Array(tools));
    if let Some(choice) = request.tool_choice {
      body.insert("tool_choice".into(), json!(render_tool_choice(choice, mode)));
    }
  }
  Ok(Value::Object(body))
}

/// The reasoning controls this vendor takes on the chat wire: the round-trip knobs the mode always
/// sends, plus whichever of the caller's reasoning axes the vendor documents. An axis the vendor
/// has no spelling for is rejected, never dropped.
fn render_reasoning(
  mode: ChatCompletionApiCompatMode,
  config: Option<&ReasoningConfig>,
  body: &mut Map<String, Value>,
) -> Result<(), Error> {
  use ChatCompletionApiCompatMode as Mode;
  let enabled = config.and_then(|config| config.enabled);
  let effort = config.and_then(|config| config.effort.as_deref());
  if let Some(config) = config {
    if config.summary.is_some() {
      return Err(Error::Build(
        "the chat completions wire has no reasoning summary axis".to_owned(),
      ));
    }
    if enabled == Some(false) && effort.is_some() {
      return Err(Error::Build(
        "reasoning cannot be both disabled and given an effort on the chat completions wire"
          .to_owned(),
      ));
    }
  }
  match mode {
    Mode::Official | Mode::Compatible => {
      if enabled.is_some() {
        return Err(Error::Build(
          "the plain chat completions wire has no reasoning on/off switch".to_owned(),
        ));
      }
    }
    Mode::DeepSeek | Mode::Mimo => {
      if let Some(enabled) = enabled {
        body.insert("thinking".into(), render_thinking_switch(enabled));
      }
    }
    Mode::Zai => {
      let thinking = match enabled {
        Some(false) => json!({"type": "disabled"}),
        _ => json!({"type": "enabled", "clear_thinking": false}),
      };
      body.insert("thinking".into(), thinking);
    }
    Mode::KimiK2 => {
      let thinking = match enabled {
        Some(false) => json!({"type": "disabled"}),
        _ => json!({"type": "enabled", "keep": "all"}),
      };
      body.insert("thinking".into(), thinking);
    }
    Mode::KimiK3 => {
      if enabled.is_some() {
        return Err(Error::Build("kimi-k3 keeps thinking on at all times".to_owned()));
      }
    }
    Mode::Qwen => {
      body.insert("preserve_thinking".into(), json!(true));
      if let Some(enabled) = enabled {
        body.insert("enable_thinking".into(), json!(enabled));
      }
    }
    Mode::MiniMax => {
      body.insert("reasoning_split".into(), json!(true));
      match enabled {
        Some(false) => {
          return Err(Error::Build(
            "minimax has no thinking off switch on the chat wire".to_owned(),
          ));
        }
        Some(true) => {
          body.insert("thinking".into(), json!({"type": "adaptive"}));
        }
        None => {}
      }
    }
    Mode::TokenHub => {
      if enabled.is_some() {
        return Err(Error::Build("the tokenhub chat wire has no reasoning controls".to_owned()));
      }
    }
    Mode::Mistral => {
      // One knob with two ends: the on/off axis is spelled as an effort word when the caller gave
      // no word of their own.
      if let Some(enabled) = enabled.filter(|_| effort.is_none()) {
        body.insert("reasoning_effort".into(), json!(if enabled { "high" } else { "none" }));
      }
    }
  }
  if let Some(effort) = effort {
    let takes_effort = matches!(
      mode,
      Mode::Official
        | Mode::Compatible
        | Mode::DeepSeek
        | Mode::Zai
        | Mode::KimiK3
        | Mode::Qwen
        | Mode::Mistral
    );
    if !takes_effort {
      return Err(Error::Build(
        "this vendor takes no reasoning effort on the chat wire".to_owned(),
      ));
    }
    body.insert("reasoning_effort".into(), json!(effort.to_lowercase()));
  }
  Ok(())
}

fn render_messages(
  conversation: &[Message],
  mode: ChatCompletionApiCompatMode,
) -> Result<Vec<Value>, Error> {
  let mut messages: Vec<Value> = Vec::new();
  let mut pending_reasoning: Option<String> = None;
  let placement = get_instruction_placement(mode);
  let mut rest = conversation;
  if placement == InstructionPlacement::LeadingSystem {
    let (run, after) = split_leading_instructions(
      conversation,
      "a `system` message can only carry text blocks on the chat completions wire",
    )?;
    if !run.is_empty() {
      let text: Vec<String> = run.iter().map(|text| text.join("\n")).collect();
      messages.push(json!({ "role": "system", "content": text.join("\n\n") }));
    }
    rest = after;
  }
  while let [message, tail @ ..] = rest {
    rest = tail;
    match message {
      Message::System { content, .. } => {
        pending_reasoning = None;
        messages.push(render_placed_instruction("system", content, placement)?);
      }
      Message::UpstreamCompaction { .. } => {
        return Err(Error::Build("the chat wire cannot carry a compacted conversation".to_owned()));
      }
      Message::Developer { content, .. } => {
        pending_reasoning = None;
        messages.push(render_placed_instruction("developer", content, placement)?);
      }
      Message::User { content, .. } => {
        pending_reasoning = None;
        messages.push(render_user(content)?);
      }
      Message::Assistant { content, .. } => {
        let mut tool_calls = Vec::new();
        rest = &rest[collect_tool_calls(rest, &mut tool_calls, None, mode)..];
        messages.push(render_assistant(content, tool_calls, pending_reasoning.take(), mode)?);
      }
      Message::ToolUse { call_id, name, arguments, .. } => {
        // A turn that called tools without writing text still is one assistant turn. Parallel
        // calls arrive as consecutive `ToolUse` messages, and the wire answers each `tool_calls`
        // message with its tool replies immediately: a second assistant message before them is
        // rejected upstream, so the whole run shares one message here.
        let mut reasoning = pending_reasoning.take().unwrap_or_default();
        let mut tool_calls = vec![render_tool_use(call_id, name, arguments)];
        rest = &rest[collect_tool_calls(rest, &mut tool_calls, Some(&mut reasoning), mode)..];
        let mut message = json!({
          "role": "assistant",
          "content": Value::Null,
          "tool_calls": tool_calls,
        });
        attach_reasoning(&mut message, (!reasoning.is_empty()).then_some(reasoning), mode);
        messages.push(message);
      }
      Message::Reasoning { plaintext, .. } => {
        // Replayed reasoning is dropped before the plain wire, which has no field for it.
        if !mode.is_plain() {
          pending_reasoning.get_or_insert_with(String::new).push_str(plaintext);
        }
      }
      Message::ToolResult { call_id, content, .. } => {
        pending_reasoning = None;
        messages.push(render_tool_result(call_id, content));
      }
    }
  }
  Ok(messages)
}

/// Collects the `ToolUse` run at the head of `following` into `tool_calls` and says how many
/// messages it spans.
///
/// A turn that opened with a call takes in the `Reasoning` between its calls as well, appending its
/// text to `reasoning` where the wire replays it; a turn that opened with text passes `None` and
/// stops at the first message that is not a call, so a `Reasoning` there leads the next turn.
fn collect_tool_calls(
  following: &[Message],
  tool_calls: &mut Vec<Value>,
  mut reasoning: Option<&mut String>,
  mode: ChatCompletionApiCompatMode,
) -> usize {
  let mut used = 0;
  for message in following {
    match (message, reasoning.as_mut()) {
      (Message::ToolUse { call_id, name, arguments, .. }, _) => {
        tool_calls.push(render_tool_use(call_id, name, arguments));
      }
      (Message::Reasoning { plaintext, .. }, Some(reasoning)) => {
        if !mode.is_plain() {
          reasoning.push_str(plaintext);
        }
      }
      _ => break,
    }
    used += 1;
  }
  used
}

/// Where a vendor takes instruction messages, and under which role.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InstructionPlacement {
  /// `system` and `developer` keep their roles wherever they are.
  Native,
  /// `developer` rides as `system`, and `system` is taken anywhere in the conversation.
  System,
  /// One `system` message, first: the leading instruction run merges into it, and a later
  /// instruction rides as user input.
  LeadingSystem,
}

/// Where this vendor takes instruction messages, and under which role.
fn get_instruction_placement(mode: ChatCompletionApiCompatMode) -> InstructionPlacement {
  use ChatCompletionApiCompatMode as Mode;
  match mode {
    Mode::Official => InstructionPlacement::Native,
    // Verified against the live APIs (DeepSeek, Zhipu) or documented (Kimi's system prompt
    // re-inserted after many turns).
    Mode::DeepSeek | Mode::Zai | Mode::KimiK2 | Mode::KimiK3 => InstructionPlacement::System,
    // Documented as first-only (Qwen, TokenHub, Mistral), or undocumented on a vendor whose open
    // template drops a later one (MiniMax), or undocumented (MiMo, anything compatible).
    Mode::Qwen | Mode::MiniMax | Mode::Mimo | Mode::TokenHub | Mode::Mistral | Mode::Compatible => {
      InstructionPlacement::LeadingSystem
    }
  }
}

/// One instruction message where the conversation has it. Under `LeadingSystem` the leading run is
/// already rendered, so this one comes later and rides as user input.
fn render_placed_instruction(
  role: &str,
  content: &[ContentBlock],
  placement: InstructionPlacement,
) -> Result<Value, Error> {
  match placement {
    InstructionPlacement::Native => render_instruction(role, content),
    InstructionPlacement::System => render_instruction("system", content),
    InstructionPlacement::LeadingSystem => render_user(content),
  }
}

fn render_instruction(role: &str, content: &[ContentBlock]) -> Result<Value, Error> {
  let text = collect_instruction_text(
    content,
    &format!("a `{role}` message can only carry text blocks on the chat completions wire"),
  )?;
  Ok(json!({ "role": role, "content": text.join("\n") }))
}

fn render_user(content: &[ContentBlock]) -> Result<Value, Error> {
  if content.iter().all(|block| matches!(block, ContentBlock::Text { .. })) {
    let mut text: Vec<&str> = Vec::new();
    for block in content {
      if let ContentBlock::Text { text: block_text } = block {
        text.push(block_text);
      }
    }
    return Ok(json!({ "role": "user", "content": text.join("\n") }));
  }

  let mut parts: Vec<Value> = Vec::new();
  for block in content {
    match block {
      ContentBlock::Text { text: block_text } => {
        parts.push(json!({ "type": "text", "text": block_text }));
      }
      ContentBlock::Image { mime_type, data_base64 } => {
        parts.push(json!({
          "type": "image_url",
          "image_url": { "url": image_data_url(mime_type, data_base64) },
        }));
      }
    }
  }
  Ok(json!({ "role": "user", "content": parts }))
}

/// An assistant message: its text, the `tool_calls` that follow it, and the reasoning replayed
/// ahead of it.
fn render_assistant(
  content: &[ContentBlock],
  tool_calls: Vec<Value>,
  reasoning: Option<String>,
  mode: ChatCompletionApiCompatMode,
) -> Result<Value, Error> {
  let mut text: Vec<&str> = Vec::new();
  for block in content {
    match block {
      ContentBlock::Text { text: block_text } => text.push(block_text),
      ContentBlock::Image { .. } => {
        return Err(Error::Build(
          "an assistant message cannot carry images on the chat completions wire".to_owned(),
        ));
      }
    }
  }
  let text = text.join("\n");
  let content_value =
    if text.is_empty() && !tool_calls.is_empty() { Value::Null } else { json!(text) };
  let mut message = json!({ "role": "assistant", "content": content_value });
  if !tool_calls.is_empty() {
    message["tool_calls"] = Value::Array(tool_calls);
  }
  attach_reasoning(&mut message, reasoning, mode);
  Ok(message)
}

/// Replays the reasoning of an assistant turn: under the protocol's field, or as the leading
/// `thinking` chunk of `content` on a wire that carries reasoning there.
fn attach_reasoning(
  message: &mut Value,
  reasoning: Option<String>,
  mode: ChatCompletionApiCompatMode,
) {
  let Some(reasoning) = reasoning.filter(|reasoning| !reasoning.is_empty()) else { return };
  if mode.is_mistral() {
    let mut chunks = vec![render_thinking_chunk(&reasoning)];
    if let Some(Value::String(text)) = message.get("content")
      && !text.is_empty()
    {
      chunks.push(json!({ "type": "text", "text": text }));
    }
    message["content"] = Value::Array(chunks);
    return;
  }
  message[REASONING_FIELD] = json!(reasoning);
}

fn render_tool_use(call_id: &str, name: &str, arguments: &Value) -> Value {
  json!({
    "id": call_id,
    "type": "function",
    "function": { "name": name, "arguments": arguments.to_string() },
  })
}

fn render_tool_result(call_id: &str, content: &Value) -> Value {
  json!({
    "role": "tool",
    "tool_call_id": call_id,
    "content": tool_result_text(content),
  })
}

fn render_tool(tool: &Tool) -> Value {
  json!({
    "type": "function",
    "function": {
      "name": tool.name,
      "description": tool.description,
      "parameters": tool.input_schema,
    },
  })
}

fn render_tool_choice(choice: ToolChoice, mode: ChatCompletionApiCompatMode) -> &'static str {
  match choice {
    ToolChoice::Auto => "auto",
    ToolChoice::None => "none",
    ToolChoice::Required if mode.is_mistral() => "any",
    ToolChoice::Required => "required",
  }
}

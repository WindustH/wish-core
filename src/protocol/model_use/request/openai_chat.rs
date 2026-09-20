//! OpenAI Chat Completions request wire.
//!
//! Conversions:
//! - `System` / `Developer` ride as ordinary messages with those roles, unlike the wires that hoist
//!   instructions into a top-level field.
//! - `User` content is a plain string when text-only, otherwise a parts array with images nested
//!   under `image_url.url` as data URLs.
//! - `Assistant` text merges with the `ToolUse` messages that follow it into one assistant message
//!   with `tool_calls` (`content: null` when there is no text); an orphan `ToolUse` becomes a
//!   standalone assistant message.
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
use crate::protocol::{ContentBlock, Message, ReasoningConfig, Request, Tool, ToolChoice};
use serde_json::{Map, Value, json};

/// Headers every call carries, besides auth and `content-type`.
pub const HEADERS: &[(&str, &str)] = &[];

/// The assistant-message field plaintext reasoning rides in.
pub(crate) const REASONING_FIELD: &str = "reasoning_content";

/// Which reasoning extension this chat endpoint speaks on top of the official wire.
///
/// The official wire has no reasoning round trip at all; every vendor patches one in differently,
/// so each patch is its own mode however small the difference between two of them is. `Official`
/// is the plain wire: nothing is read and nothing is sent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ChatCompletionApiCompatMode {
  /// Plain wire: reasoning is neither read nor sent.
  #[default]
  Official,
  /// `reasoning_content`, which must be replayed whenever tools are in play.
  DeepSeek,
  /// `reasoning_content` plus `thinking.clear_thinking: false` (the default strips the history).
  Zai,
  /// `reasoning_content` plus `thinking.keep: "all"` (k2.6 needs it; k2.7-code fixes it to that).
  KimiK2,
  /// `reasoning_content`; k3 has no `thinking` object.
  KimiK3,
  /// `reasoning_content` plus `preserve_thinking: true` (Bailian keeps nothing by default).
  Qwen,
  /// `reasoning_content` plus `reasoning_split: true` (otherwise the thoughts stay in `content`).
  MiniMax,
  /// `reasoning_content`; the history is kept unconditionally.
  Mimo,
  /// `reasoning_content`; the history is kept unconditionally.
  TokenHub,
  /// Reasoning rides in `content` as `thinking` chunks, and the history is replayed the same way.
  Mistral,
}

impl ChatCompletionApiCompatMode {
  /// Whether this is Mistral's own chat wire, which spells a few things its own way: reasoning in
  /// `content` chunks, `max_tokens` for the cap, no `store` or `stream_options`, `any` for a
  /// required tool call, and `model_length` for context exhaustion.
  pub(crate) fn is_mistral(self) -> bool {
    matches!(self, ChatCompletionApiCompatMode::Mistral)
  }

  /// The controls this vendor takes on the chat wire: the round-trip knobs the mode always sends,
  /// plus whichever of the caller's reasoning axes the vendor documents. An axis the vendor has no
  /// spelling for is rejected, never dropped.
  fn render_controls(
    self,
    config: Option<&ReasoningConfig>,
    body: &mut Map<String, Value>,
  ) -> Result<(), Error> {
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
    match self {
      ChatCompletionApiCompatMode::Official => {
        if enabled.is_some() {
          return Err(Error::Build(
            "the plain chat completions wire has no reasoning on/off switch".to_owned(),
          ));
        }
      }
      ChatCompletionApiCompatMode::DeepSeek => {
        if let Some(enabled) = enabled {
          body.insert(
            "thinking".into(),
            json!({"type": if enabled { "enabled" } else { "disabled" }}),
          );
        }
      }
      ChatCompletionApiCompatMode::Zai => {
        let thinking = match enabled {
          Some(false) => json!({"type": "disabled"}),
          _ => json!({"type": "enabled", "clear_thinking": false}),
        };
        body.insert("thinking".into(), thinking);
      }
      ChatCompletionApiCompatMode::KimiK2 => {
        let thinking = match enabled {
          Some(false) => json!({"type": "disabled"}),
          _ => json!({"type": "enabled", "keep": "all"}),
        };
        body.insert("thinking".into(), thinking);
      }
      ChatCompletionApiCompatMode::KimiK3 => {
        if enabled.is_some() {
          return Err(Error::Build("kimi-k3 keeps thinking on at all times".to_owned()));
        }
      }
      ChatCompletionApiCompatMode::Qwen => {
        body.insert("preserve_thinking".into(), json!(true));
        if let Some(enabled) = enabled {
          body.insert("enable_thinking".into(), json!(enabled));
        }
      }
      ChatCompletionApiCompatMode::MiniMax => {
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
      ChatCompletionApiCompatMode::Mimo => {
        if let Some(enabled) = enabled {
          body.insert(
            "thinking".into(),
            json!({"type": if enabled { "enabled" } else { "disabled" }}),
          );
        }
      }
      ChatCompletionApiCompatMode::TokenHub => {
        if enabled.is_some() {
          return Err(Error::Build("the tokenhub chat wire has no reasoning controls".to_owned()));
        }
      }
      ChatCompletionApiCompatMode::Mistral => {
        // One knob with two ends: the on/off axis is spelled as an effort word when the caller gave
        // no word of their own.
        if let Some(enabled) = enabled.filter(|_| effort.is_none()) {
          body.insert("reasoning_effort".into(), json!(if enabled { "high" } else { "none" }));
        }
      }
    }
    match (
      self,
      matches!(
        self,
        ChatCompletionApiCompatMode::Official
          | ChatCompletionApiCompatMode::DeepSeek
          | ChatCompletionApiCompatMode::Zai
          | ChatCompletionApiCompatMode::KimiK3
          | ChatCompletionApiCompatMode::Qwen
          | ChatCompletionApiCompatMode::Mistral
      ),
      effort,
    ) {
      (_, true, Some(effort)) => {
        body.insert("reasoning_effort".into(), json!(effort.to_lowercase()));
      }
      (_, false, Some(_)) => {
        return Err(Error::Build(
          "this vendor takes no reasoning effort on the chat wire".to_owned(),
        ));
      }
      (_, _, None) => {}
    }
    Ok(())
  }
}

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
  mode.render_controls(request.reasoning.as_ref(), &mut body)?;
  if !request.tools.is_empty() {
    let tools: Vec<Value> = request.tools.iter().map(render_tool).collect();
    body.insert("tools".into(), Value::Array(tools));
    if let Some(choice) = request.tool_choice {
      body.insert("tool_choice".into(), json!(render_tool_choice(choice, mode)));
    }
  }
  Ok(Value::Object(body))
}

fn render_messages(
  conversation: &[Message],
  mode: ChatCompletionApiCompatMode,
) -> Result<Vec<Value>, Error> {
  let mut messages: Vec<Value> = Vec::new();
  let mut pending_reasoning: Option<String> = None;
  let mut index = 0;
  while index < conversation.len() {
    match &conversation[index] {
      Message::System { content, .. } => {
        pending_reasoning = None;
        messages.push(render_instruction("system", content)?);
      }
      Message::UpstreamCompaction { .. } => {
        return Err(Error::Build("the chat wire cannot carry a compacted conversation".to_owned()));
      }
      Message::Developer { content, .. } => {
        pending_reasoning = None;
        messages.push(render_instruction("developer", content)?);
      }
      Message::User { content, .. } => {
        pending_reasoning = None;
        messages.push(render_user(content)?);
      }
      Message::Assistant { content, .. } => {
        let (message, used) =
          render_assistant(content, &conversation[index + 1..], pending_reasoning.take(), mode)?;
        messages.push(message);
        index += used;
      }
      Message::ToolUse { call_id, name, arguments, .. } => {
        let mut message = json!({
          "role": "assistant",
          "content": Value::Null,
          "tool_calls": [render_tool_use(call_id, name, arguments)],
        });
        attach_reasoning(&mut message, pending_reasoning.take(), mode);
        messages.push(message);
      }
      Message::Reasoning { plaintext, .. } => {
        // Replayed reasoning is dropped before the official wire, which has no field for it.
        if !matches!(mode, ChatCompletionApiCompatMode::Official) {
          pending_reasoning.get_or_insert_with(String::new).push_str(plaintext);
        }
      }
      Message::ToolResult { call_id, content, .. } => {
        pending_reasoning = None;
        messages.push(render_tool_result(call_id, content));
      }
    }
    index += 1;
  }
  Ok(messages)
}

fn render_instruction(role: &str, content: &[ContentBlock]) -> Result<Value, Error> {
  let mut text: Vec<&str> = Vec::new();
  for block in content {
    match block {
      ContentBlock::Text { text: block_text } => text.push(block_text),
      _ => {
        return Err(Error::Build(format!(
          "a `{role}` message can only carry text blocks on the chat completions wire"
        )));
      }
    }
  }
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
          "image_url": { "url": format!("data:{mime_type};base64,{data_base64}") },
        }));
      }
    }
  }
  Ok(json!({ "role": "user", "content": parts }))
}

fn render_assistant(
  content: &[ContentBlock],
  following: &[Message],
  reasoning: Option<String>,
  mode: ChatCompletionApiCompatMode,
) -> Result<(Value, usize), Error> {
  let mut tool_uses: Vec<Value> = Vec::new();
  let mut used = 0;
  while let Some(Message::ToolUse { call_id, name, arguments, .. }) = following.get(used) {
    tool_uses.push(render_tool_use(call_id, name, arguments));
    used += 1;
  }

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
    if text.is_empty() && !tool_uses.is_empty() { Value::Null } else { json!(text) };
  let mut message = json!({ "role": "assistant", "content": content_value });
  if !tool_uses.is_empty() {
    message["tool_calls"] = Value::Array(tool_uses);
  }
  attach_reasoning(&mut message, reasoning, mode);
  Ok((message, used))
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

/// The chunk a replayed thought travels in on this wire.
fn render_thinking_chunk(reasoning: &str) -> Value {
  json!({
    "type": "thinking",
    "closed": true,
    "thinking": [{ "type": "text", "text": reasoning }],
  })
}

fn render_tool_use(call_id: &str, name: &str, arguments: &Value) -> Value {
  json!({
    "id": call_id,
    "type": "function",
    "function": { "name": name, "arguments": arguments.to_string() },
  })
}

fn render_tool_result(call_id: &str, content: &Value) -> Value {
  let content = match content {
    Value::String(text) => text.clone(),
    other => other.to_string(),
  };
  json!({
    "role": "tool",
    "tool_call_id": call_id,
    "content": content,
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

//! OpenAI Responses request wire. The Codex deployment shares this wire and differs in what it asks
//! for beside the body - its own markers, and the account the subscription belongs to - so it needs
//! no request renderer of its own either.
//!
//! Conversions:
//! - The whole conversation becomes `input[]` items: `system` / `developer` / `user` keep their
//!   roles as message items, `Assistant` text becomes an `output_text` message, `ToolUse` becomes
//!   `function_call` and `ToolResult` becomes `function_call_output` (string payload passed through,
//!   anything else stringified).
//! - `Reasoning` becomes a `reasoning` item: `ciphertext` -> `encrypted_content`, `plaintext` ->
//!   `reasoning_text` content, and `display` -> the required `summary` array.
//!   The mode's `reasoning_form` decides whether encrypted reasoning is requested
//!   (`include: ["reasoning.encrypted_content"]`) and sent back, or sent back as plaintext
//!   reasoning; a reasoning item without the material its form sends is dropped.
//! - A compacted history becomes a `compaction` item, carrying the opaque payload back verbatim
//!   with the service's own id when it gave one.
//! - Tools are flat (`{"type": "function", "name", ...}` without `strict`) and the choice is a plain
//!   string. Caching is automatic on this wire, so `PromptCache::key` is the only control and rides
//!   as `prompt_cache_key`; breakpoints have no spelling here.
//! - `ReasoningConfig` renders as the `reasoning` object: every tier is available, `enabled: false`
//!   becomes `effort: "none"`, and `Auto` asks for a summary.
//!
//! Constraints:
//! - With `store: false` the client holds the state, so encrypted reasoning the wire handed back
//!   has to be sent again on the next turn, and `include: ["reasoning.encrypted_content"]` is what
//!   asks for it in the first place.
//! - `function_call_output` answers one `function_call` and repeats its `call_id`.
//! - The Codex deployment takes no `max_output_tokens`: its backend caps an answer by the plan's own
//!   window, and a cap asked for there is refused as an unsupported parameter.
//! - The Codex deployment serves streamed calls only: a buffered call is refused here, before
//!   anything is sent, rather than left to be turned away by the backend.
//!
//! Trade-offs:
//! - Flat `display` text becomes one `summary_text` part when replayed; the original summary part
//!   boundaries are not retained by the shared message representation.
//! - `store: false` is always sent, and system text stays an input item instead of being hoisted into
//!   the top-level `instructions` field: one ordering rule across all providers is worth more than
//!   matching this protocol exactly. The assistant `phase` field is not modeled. Streaming adds
//!   `stream: true`; the terminal response event then carries the usage, so nothing else is asked for.

use crate::protocol::error::Error;
use crate::protocol::model_use::message::image_data_url;
use crate::protocol::model_use::tool::tool_result_text;
use crate::protocol::{
  ContentBlock, Message, ReasoningConfig, ReasoningOpaqueKind, ReasoningSummary, Request, Tool,
  ToolChoice,
};
use serde_json::{Map, Value, json};

pub use crate::protocol::model_use::mode::{ReasoningForm, ResponsesApiMode, ResponsesDeployment};

/// Headers the mode's deployment asks for besides auth and `content-type`.
pub(crate) fn get_headers(mode: ResponsesApiMode) -> &'static [(&'static str, &'static str)] {
  match mode.deployment {
    ResponsesDeployment::Platform => HEADERS,
    ResponsesDeployment::Codex => CODEX_HEADERS,
  }
}

/// Headers every call carries, besides auth and `content-type`.
pub const HEADERS: &[(&str, &str)] = &[];

/// What the Codex deployment asks for on top of this wire: the product it is asked as and the
/// client family its backend routes by. The rest of what it expects is the caller's material,
/// placed with the call: `chatgpt-account-id` names the account, and `session-id` /
/// `x-client-request-id` name the conversation the answer belongs to.
pub const CODEX_HEADERS: &[(&str, &str)] =
  &[("originator", "codex_cli_rs"), ("oai-product-sku", "codex")];

pub fn render(request: &Request, mode: ResponsesApiMode) -> Result<Value, Error> {
  let mut body = Map::new();
  body.insert("model".into(), json!(request.model));
  body.insert("store".into(), json!(false));
  if let Some(key) = request.cache.as_ref().and_then(|cache| cache.key.as_deref()) {
    body.insert("prompt_cache_key".into(), json!(key));
  }
  // The Codex backend serves only streamed calls, so a buffered one asks for a reply it will not
  // give; refused here rather than by the service, like the output cap below.
  if mode.deployment == ResponsesDeployment::Codex && !request.stream {
    return Err(Error::Build("the Codex deployment serves only streamed calls".to_owned()));
  }
  if request.stream {
    body.insert("stream".into(), json!(true));
  }
  if mode.reasoning_form == ReasoningForm::Ciphertext {
    body.insert("include".into(), json!(["reasoning.encrypted_content"]));
  }
  if let Some(max_output_tokens) = request.max_output_tokens {
    if mode.deployment == ResponsesDeployment::Codex {
      return Err(Error::Build("the Codex deployment takes no `max_output_tokens`".to_owned()));
    }
    body.insert("max_output_tokens".into(), json!(max_output_tokens));
  }
  if let Some(reasoning) = &request.reasoning {
    render_reasoning(reasoning, &mut body)?;
  }
  body.insert("input".into(), Value::Array(render_items(&request.conversation, mode)?));
  if !request.tools.is_empty() {
    let tools: Vec<Value> = request.tools.iter().map(render_tool).collect();
    body.insert("tools".into(), Value::Array(tools));
    if let Some(choice) = request.tool_choice {
      body.insert("tool_choice".into(), json!(render_tool_choice(choice)));
    }
  }
  Ok(Value::Object(body))
}

/// The conversation as this wire's `input[]` items.
///
/// Shared with the platform compaction call, which sends a whole history without any of a model
/// call's controls beside it.
pub(crate) fn render_items(
  conversation: &[Message],
  mode: ResponsesApiMode,
) -> Result<Vec<Value>, Error> {
  let mut items: Vec<Value> = Vec::new();
  for message in conversation {
    match message {
      Message::System { content, .. } => items.push(render_input_message("system", content)),
      Message::Developer { content, .. } => items.push(render_input_message("developer", content)),
      Message::User { content, .. } => items.push(render_input_message("user", content)),
      Message::ToolUse { call_id, name, arguments, .. } => {
        items.push(render_function_call_item(call_id, name, arguments))
      }
      Message::ToolResult { call_id, content, .. } => {
        items.push(render_function_output_item(call_id, content))
      }
      Message::Reasoning { plaintext, display, ciphertext, opaque_kind, .. } => {
        let ciphertext = ReasoningOpaqueKind::material_if_kind(
          *opaque_kind,
          ReasoningOpaqueKind::OpenAiEncrypted,
          ciphertext,
        );
        if let Some(item) = render_reasoning_item(plaintext, display, ciphertext, mode) {
          items.push(item);
        }
      }
      Message::Assistant { content, .. } => {
        if let Some(item) = render_assistant_item(content)? {
          items.push(item);
        }
      }
      Message::UpstreamCompaction { id, encrypted_content, .. } => {
        items.push(render_compaction_item(id.as_deref(), encrypted_content));
      }
    }
  }
  Ok(items)
}

/// The item a compacted history travels as: the service's id when there is one, and the opaque
/// payload, which is never read or altered here.
pub(crate) fn render_compaction_item(id: Option<&str>, encrypted_content: &str) -> Value {
  match id {
    Some(id) => json!({"type": "compaction", "id": id, "encrypted_content": encrypted_content}),
    None => json!({"type": "compaction", "encrypted_content": encrypted_content}),
  }
}

/// The wire's `reasoning` object: the caller's tier travels lowercase, "off" is spelled
/// `effort: "none"`, and `Auto` asks for a summary beside the effort (the wire has no "none"
/// summary value, so `None` is expressed by omission).
fn render_reasoning(config: &ReasoningConfig, body: &mut Map<String, Value>) -> Result<(), Error> {
  if config.enabled == Some(false) && config.effort.is_some() {
    return Err(Error::Build(
      "reasoning cannot be both disabled and given an effort on the responses wire".to_owned(),
    ));
  }
  let mut reasoning = Map::new();
  match (config.enabled, config.effort.as_deref()) {
    (Some(false), None) => {
      reasoning.insert("effort".into(), json!("none"));
    }
    (_, Some(effort)) => {
      reasoning.insert("effort".into(), json!(effort.to_lowercase()));
    }
    (_, None) => {}
  }
  if config.summary == Some(ReasoningSummary::Auto) {
    reasoning.insert("summary".into(), json!("auto"));
  }
  if !reasoning.is_empty() {
    body.insert("reasoning".into(), Value::Object(reasoning));
  }
  Ok(())
}

fn render_tool_choice(choice: ToolChoice) -> &'static str {
  match choice {
    ToolChoice::Auto => "auto",
    ToolChoice::None => "none",
    ToolChoice::Required => "required",
  }
}

fn render_tool(tool: &Tool) -> Value {
  json!({
    "type": "function",
    "name": tool.name,
    "description": tool.description,
    "parameters": tool.input_schema,
  })
}

fn render_assistant_item(content: &[ContentBlock]) -> Result<Option<Value>, Error> {
  let mut parts: Vec<Value> = Vec::new();
  for block in content {
    match block {
      ContentBlock::Text { text } => parts.push(json!({"type": "output_text", "text": text})),
      ContentBlock::Image { .. } => {
        return Err(Error::Build(
          "assistant content block `image` cannot be expressed on responses wire".to_owned(),
        ));
      }
    }
  }
  if parts.is_empty() {
    return Ok(None);
  }
  Ok(Some(json!({"type": "message", "role": "assistant", "content": parts})))
}

fn render_reasoning_item(
  plaintext: &str,
  display: &str,
  ciphertext: &str,
  mode: ResponsesApiMode,
) -> Option<Value> {
  let summary = if display.is_empty() {
    Vec::new()
  } else {
    vec![json!({"type": "summary_text", "text": display})]
  };
  match mode.reasoning_form {
    ReasoningForm::Ciphertext if !ciphertext.is_empty() => {
      Some(json!({"type": "reasoning", "summary": summary, "encrypted_content": ciphertext}))
    }
    ReasoningForm::Plaintext if !plaintext.is_empty() => Some(json!({"type": "reasoning",
      "summary": summary,
      "content": [{"type": "reasoning_text", "text": plaintext}]})),
    _ => None,
  }
}

fn render_input_part(block: &ContentBlock) -> Value {
  match block {
    ContentBlock::Text { text } => json!({"type": "input_text", "text": text}),
    // The image travels in the data-URL shape this wire's `input_image` part expects.
    ContentBlock::Image { mime_type, data_base64 } => {
      json!({"type": "input_image", "image_url": image_data_url(mime_type, data_base64)})
    }
  }
}

fn render_input_message(role: &str, blocks: &[ContentBlock]) -> Value {
  let content: Vec<Value> = blocks.iter().map(render_input_part).collect();
  json!({"role": role, "content": content})
}

fn render_function_call_item(call_id: &str, name: &str, arguments: &Value) -> Value {
  json!({
    "type": "function_call",
    "call_id": call_id,
    "name": name,
    "arguments": serde_json::to_string(arguments).unwrap_or_else(|_| "{}".to_owned()),
  })
}

fn render_function_output_item(call_id: &str, content: &Value) -> Value {
  json!({
    "type": "function_call_output",
    "call_id": call_id,
    "output": tool_result_text(content),
  })
}

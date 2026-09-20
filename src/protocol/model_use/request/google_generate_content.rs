//! Google `generateContent` request wire. `google_vertex` shares this wire and differs only in URL
//! and authentication, so it needs no request renderer of its own.
//!
//! Conversions:
//! - Leading `System` messages become `systemInstruction`, joined into a single text part.
//! - Adjacent same-role messages merge into one turn, and `ToolResult` messages merge into the
//!   single user turn that follows.
//! - `ToolUse` becomes `functionCall` parts inside the model turn. A `Reasoning` that has text
//!   becomes a thought part (`{"thought": true, "text": ...}`) carrying its signature, while one
//!   that is only a signature is held back and attached to the part it seals, because Gemini marks
//!   its thought parts and its function calls with that signature.
//! - Tools are double wrapped (`tools[].functionDeclarations[]`) and the choice is spelled with the
//!   uppercase `AUTO` / `ANY` / `NONE` mode.
//! - `ReasoningConfig` renders inside `generationConfig.thinkingConfig`: `effort` becomes the
//!   uppercase `thinkingLevel` word, `enabled: false` becomes the zero `thinkingBudget` budget (the
//!   only off switch this wire documents), and a summary toggles `includeThoughts`.
//!
//! Constraints:
//! - `contents[]` alternates `user` / `model` strictly, and instructions belong in the top-level
//!   `systemInstruction` object rather than a turn: an instruction past the leading run, or a
//!   non-text instruction block, is rejected.
//! - A `thoughtSignature` must be echoed back exactly where it arrived - the thought part or the
//!   `functionCall` part it seals - when the call is continued; dropping it loses the model's
//!   reasoning state.
//! - Every `functionCall` is answered by a `functionResponse` in the `user` turn that follows it.
//!
//! Trade-offs:
//! - A `Developer` message has no role of its own on this wire, so its text joins the same
//!   `systemInstruction` text.
//! - Function calls carry no usable id on this wire, so results are paired by tool name and call
//!   order; a decoded call without `id` keeps an empty `call_id` and the id is omitted on render.
//! - Tool results send `{"name", "response"}` where an object payload passes through and anything
//!   else is wrapped as `{"content": ...}`; a missing result block is an error.
//! - Caching is implicit here, so `PromptCache` is not sent: explicit cached content
//!   (`cachedContents`) is not modeled.

use crate::protocol::error::Error;
use crate::protocol::{
  ContentBlock, Message, ReasoningConfig, ReasoningSummary, Request, Tool, ToolChoice,
};
use serde_json::{Map, Value, json};

/// Headers every call carries, besides auth and `content-type`.
pub const HEADERS: &[(&str, &str)] = &[];

pub fn render(request: &Request) -> Result<Value, Error> {
  let (system, rest) = split_leading_instructions(&request.conversation)?;
  let contents = render_contents(rest)?;
  if contents.is_empty() {
    return Err(Error::Build("request has no contents".to_owned()));
  }

  let mut body = Map::new();
  if !system.is_empty() {
    body.insert(
      "systemInstruction".into(),
      json!({"role": "user", "parts": [{"text": system.join("\n")}]}),
    );
  }
  body.insert("contents".into(), Value::Array(contents));
  if !request.tools.is_empty() {
    let declarations: Vec<Value> = request.tools.iter().map(render_function_declaration).collect();
    body.insert("tools".into(), json!([{"functionDeclarations": declarations}]));
    if let Some(choice) = request.tool_choice {
      body.insert(
        "toolConfig".into(),
        json!({"functionCallingConfig": {"mode": render_tool_choice(choice)}}),
      );
    }
  }
  let mut generation_config = Map::new();
  if let Some(max_output_tokens) = request.max_output_tokens {
    generation_config.insert("maxOutputTokens".into(), json!(max_output_tokens));
  }
  if let Some(reasoning) = &request.reasoning
    && let Some(thinking_config) = render_thinking_config(reasoning)?
  {
    generation_config.insert("thinkingConfig".into(), thinking_config);
  }
  if !generation_config.is_empty() {
    body.insert("generationConfig".into(), Value::Object(generation_config));
  }
  Ok(Value::Object(body))
}

/// The wire's `thinkingConfig`: the caller's depth tier becomes the uppercase `thinkingLevel` word,
/// the off switch becomes a zero `thinkingBudget` (the only budget this wire documents), and
/// `includeThoughts` carries the summary request.
fn render_thinking_config(config: &ReasoningConfig) -> Result<Option<Value>, Error> {
  if config.enabled == Some(false) && config.effort.is_some() {
    return Err(Error::Build(
      "thinking cannot be both disabled and given a depth tier on the generateContent wire"
        .to_owned(),
    ));
  }
  let mut thinking = Map::new();
  if let Some(effort) = &config.effort {
    thinking.insert("thinkingLevel".into(), json!(effort.to_uppercase()));
  } else if config.enabled == Some(false) {
    thinking.insert("thinkingBudget".into(), json!(0));
  }
  match config.summary {
    Some(ReasoningSummary::Auto) => {
      thinking.insert("includeThoughts".into(), json!(true));
    }
    Some(ReasoningSummary::None) => {
      thinking.insert("includeThoughts".into(), json!(false));
    }
    None => {}
  }
  if thinking.is_empty() {
    return Ok(None);
  }
  Ok(Some(Value::Object(thinking)))
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
            "a system instruction can only carry text blocks on the generateContent wire"
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

fn render_contents(conversation: &[Message]) -> Result<Vec<Value>, Error> {
  let mut contents: Vec<(&'static str, Vec<Value>)> = Vec::new();
  let mut pending: Vec<(String, String)> = Vec::new();
  let mut index = 0;
  while index < conversation.len() {
    match &conversation[index] {
      Message::System { .. } | Message::Developer { .. } => {
        return Err(Error::Build(
          "a system instruction must lead the conversation on the generateContent wire".to_owned(),
        ));
      }
      Message::UpstreamCompaction { .. } => {
        return Err(Error::Build(
          "the generateContent wire cannot carry a compacted conversation".to_owned(),
        ));
      }
      Message::User { content, .. } => {
        if let Some((_, name)) = pending.first() {
          return Err(build_missing_response_error(name));
        }
        push_turn(&mut contents, "user", render_user_parts(content)?);
        index += 1;
      }
      Message::Reasoning { .. } | Message::Assistant { .. } | Message::ToolUse { .. } => {
        if let Some((_, name)) = pending.first() {
          return Err(build_missing_response_error(name));
        }
        let mut parts: Vec<Value> = Vec::new();
        let mut signature: Option<String> = None;
        let push_part =
          |parts: &mut Vec<Value>, mut part: Value, signature: &mut Option<String>| {
            if part.get("thoughtSignature").is_none()
              && let Some(value) = signature.take()
            {
              part["thoughtSignature"] = json!(value);
            }
            parts.push(part);
          };
        let mut used = 0;
        while let Some(message) = conversation.get(index + used) {
          match message {
            Message::Reasoning { plaintext, signature: proof, .. } => {
              if plaintext.is_empty() {
                if !proof.is_empty() {
                  signature = Some(proof.clone());
                }
              } else {
                let mut part = json!({"thought": true, "text": plaintext});
                if !proof.is_empty() {
                  part["thoughtSignature"] = json!(proof);
                }
                push_part(&mut parts, part, &mut signature);
              }
            }
            Message::Assistant { content, .. } => {
              for block in content {
                match block {
                  ContentBlock::Text { text } => {
                    push_part(&mut parts, json!({"text": text}), &mut signature);
                  }
                  ContentBlock::Image { .. } => {
                    return Err(Error::Build(
                      "an assistant message cannot carry images on the generateContent wire"
                        .to_owned(),
                    ));
                  }
                }
              }
            }
            Message::ToolUse { call_id, name, arguments, .. } => {
              let mut function_call = json!({"name": name, "args": arguments});
              if !call_id.is_empty() {
                function_call["id"] = json!(call_id);
              }
              pending.push((call_id.clone(), name.clone()));
              push_part(&mut parts, json!({"functionCall": function_call}), &mut signature);
            }
            _ => break,
          }
          used += 1;
        }
        push_turn(&mut contents, "model", parts);
        index += used;
      }
      Message::ToolResult { .. } => {
        if pending.is_empty() {
          return Err(Error::Build(
            "a tool result has no matching tool call on the generateContent wire".to_owned(),
          ));
        }
        let mut parts: Vec<Value> = Vec::new();
        let mut used = 0;
        while let Some(Message::ToolResult { call_id, name, content, .. }) =
          conversation.get(index + used)
        {
          match pending.get(used) {
            Some((_, expected)) if expected == name => {}
            Some((_, expected)) => {
              return Err(Error::Build(format!(
                "tool result for `{name}` does not match the expected tool call `{expected}` on the generateContent wire"
              )));
            }
            None => {
              return Err(Error::Build(format!(
                "tool result for `{name}` has no matching tool call on the generateContent wire"
              )));
            }
          }
          parts.push(render_function_response(call_id, name, content));
          used += 1;
        }
        if used != pending.len() {
          return Err(build_missing_response_error(&pending[used].1));
        }
        pending.clear();
        push_turn(&mut contents, "user", parts);
        index += used;
      }
    }
  }
  if let Some((_, name)) = pending.first() {
    return Err(build_missing_response_error(name));
  }
  match contents.first() {
    Some((role, _)) if *role != "user" => Err(Error::Build(
      "the first turn must be a user message on the generateContent wire".to_owned(),
    )),
    _ => {
      Ok(contents.into_iter().map(|(role, parts)| json!({"role": role, "parts": parts})).collect())
    }
  }
}

fn build_missing_response_error(name: &str) -> Error {
  Error::Build(format!(
    "tool call `{name}` is not followed by its tool result on the generateContent wire"
  ))
}

fn push_turn(
  turns: &mut Vec<(&'static str, Vec<Value>)>,
  role: &'static str,
  mut parts: Vec<Value>,
) {
  if parts.is_empty() {
    return;
  }
  if let Some((last_role, last_parts)) = turns.last_mut()
    && *last_role == role
  {
    last_parts.append(&mut parts);
    return;
  }
  turns.push((role, parts));
}

fn render_user_parts(content: &[ContentBlock]) -> Result<Vec<Value>, Error> {
  let mut parts: Vec<Value> = Vec::new();
  for block in content {
    match block {
      ContentBlock::Text { text } => parts.push(json!({"text": text})),
      ContentBlock::Image { mime_type, data_base64 } => {
        parts.push(json!({"inlineData": {"mimeType": mime_type, "data": data_base64}}));
      }
    }
  }
  Ok(parts)
}

fn render_function_response(call_id: &str, name: &str, content: &Value) -> Value {
  let response = match content {
    Value::Object(_) => content.clone(),
    other => json!({"content": other}),
  };
  let mut function_response = json!({"name": name, "response": response});
  if !call_id.is_empty() {
    function_response["id"] = json!(call_id);
  }
  json!({"functionResponse": function_response})
}

fn render_function_declaration(tool: &Tool) -> Value {
  json!({
    "name": tool.name,
    "description": tool.description,
    "parameters": tool.input_schema,
  })
}

fn render_tool_choice(choice: ToolChoice) -> &'static str {
  match choice {
    ToolChoice::Auto => "AUTO",
    ToolChoice::None => "NONE",
    ToolChoice::Required => "ANY",
  }
}

/// The streaming endpoint for the same request body.
pub fn resolve_stream_path(path: &str) -> String {
  let mut url = path.to_owned();
  if url.contains(":generateContent") && !url.contains(":streamGenerateContent") {
    url = url.replace(":generateContent", ":streamGenerateContent");
  }
  if url.contains('?') {
    if !url.contains("alt=sse") {
      url.push_str("&alt=sse");
    }
  } else {
    url.push_str("?alt=sse");
  }
  url
}

//! OpenAI Responses compaction wire, shared by the platform API and the Codex deployment.
//!
//! Conversions:
//! - The body is the model and the history: the conversation travels as the same `input[]` items a
//!   model call sends, rendered by that protocol's own item renderer, and a history that was compacted
//!   before travels as its own `compaction` item, opaque payload and all.
//! - The platform API takes the call on an endpoint of its own, `POST <base>/responses/compact`,
//!   which accepts the model and the history and nothing else; the service keeps the compaction it
//!   was asked for there.
//! - The Codex deployment takes it on the call it always serves, the streamed `/responses`, and is
//!   told what is being asked for by its last input item, `{"type": "compaction_trigger"}`: a
//!   request control rather than part of the conversation, appended here and never carried in the
//!   caller's history. Everything before that item is the model call's own body, tools, reasoning
//!   and `prompt_cache_key` included, so the compaction repeats the prefix the session's calls
//!   cached, the way the Codex CLI's own compaction call does.
//!
//! Constraints:
//! - The reply's items have to be sent back verbatim, so nothing here reads, rewrites or drops a
//!   `compaction` item already in the history.
//! - The Codex deployment routes the call by the purpose it declares, so the compaction marker and
//!   the feature it advertises travel with the call.
//!
//! Trade-offs:
//! - The wire's `previous_response_id` is not modeled: this crate holds the state (`store: false`),
//!   so the history is always sent whole.
//! - `instructions` is not modeled either; instructions travel as messages, the way the model call
//!   sends them.
//! - The platform endpoint takes the model and the history only; the model call's tools, reasoning
//!   and caching are not sent there.
//! - The Codex CLI also sends `parallel_tool_calls: true`; the model call here does not, and the
//!   compaction matches the model call rather than the CLI.

use crate::protocol::Request;
use crate::protocol::error::Error;
use crate::protocol::model_use::request::openai_responses::{
  self as model_use, ResponsesApiCompatMode, ResponsesDeployment, render_items,
};
use crate::protocol::upstream_compaction::UpstreamCompactionRequest;
use serde_json::{Map, Value, json};

/// The purpose a Codex compaction call names, in the header its backend reads turn metadata from.
///
/// The CLI fills that payload with the session and turn it belongs to as well; what matters here is
/// the purpose, which is also the part the backend routes by.
const COMPACTION_MARKER: &str = r#"{"request_kind":"compaction"}"#;

/// What this deployment adds to a compaction call beside what its calls always carry.
pub(crate) fn get_extra_headers(
  variant: ResponsesApiCompatMode,
) -> &'static [(&'static str, &'static str)] {
  match variant.deployment {
    // The platform endpoint is told what the call is by the path it goes to.
    ResponsesDeployment::Platform => &[],
    // The Codex backend is told by what the call says about itself: the feature it advertises as
    // supported, and the purpose it names.
    ResponsesDeployment::Codex => &[
      ("x-codex-beta-features", "remote_compaction_v2"),
      ("x-codex-turn-metadata", COMPACTION_MARKER),
    ],
  }
}

pub fn render(
  request: &UpstreamCompactionRequest,
  variant: ResponsesApiCompatMode,
) -> Result<Value, Error> {
  match variant.deployment {
    ResponsesDeployment::Platform => {
      let mut body = Map::new();
      body.insert("model".into(), json!(request.model));
      body.insert("input".into(), Value::Array(render_items(&request.conversation, variant)?));
      Ok(Value::Object(body))
    }
    ResponsesDeployment::Codex => {
      // The backend keeps no state and answers this call on a stream, the way it answers every
      // call; the body is that call's, with no output cap because the deployment takes none.
      let mut body = model_use::render(
        &Request {
          stream: true,
          model: request.model.clone(),
          conversation: request.conversation.clone(),
          tools: request.tools.clone(),
          tool_choice: request.tool_choice,
          max_output_tokens: None,
          reasoning: request.reasoning.clone(),
          cache: request.cache.clone(),
        },
        variant,
      )?;
      body
        .get_mut("input")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| Error::Build("the model call body has no `input` array".into()))?
        .push(json!({ "type": "compaction_trigger" }));
      Ok(body)
    }
  }
}

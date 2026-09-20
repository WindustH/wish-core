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
//!   caller's history.
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
//! - The Codex deployment's own compaction call carries the tools of the session it compacts and
//!   asks for a reasoning summary; neither is sent here, because what a history compacts to is the
//!   service's business and none of it comes back as an answer.

use crate::protocol::error::Error;
use crate::protocol::model_use::request::openai_responses::{
  ResponsesApiCompatMode, ResponsesDeployment, render_items,
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
  let mut body = Map::new();
  body.insert("model".into(), json!(request.model));
  let mut items = render_items(&request.conversation, variant)?;
  if variant.deployment == ResponsesDeployment::Codex {
    // The backend keeps no state and answers this call on a stream, the way it answers every call.
    body.insert("store".into(), json!(false));
    body.insert("stream".into(), json!(true));
    items.push(json!({ "type": "compaction_trigger" }));
  }
  body.insert("input".into(), Value::Array(items));
  Ok(Value::Object(body))
}

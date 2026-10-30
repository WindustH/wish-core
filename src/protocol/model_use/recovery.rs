//! Native stream recovery. Tokens are private call state, never messages or stream events.
use super::ModelUseProtocol;
use crate::protocol::{Usage, attempt::Call, error::Error};
use serde_json::Value;

/// A generation checkpoint resumes computation; it is not an event-stream cursor.
#[derive(Clone, Copy)]
pub enum RecoveryProtocol {
  GoogleGeneration { enveloped: bool },
}
pub struct Checkpoint {
  pub token: String,
  pub continuation: bool,
}
impl RecoveryProtocol {
  pub fn for_call(protocol: ModelUseProtocol, model: &str) -> Option<Self> {
    // A Cloud Code endpoint also serves non-Gemini models. Never infer their semantics.
    if !model.starts_with("gemini-") {
      return None;
    }
    match protocol {
      ModelUseProtocol::GoogleGenerateContent | ModelUseProtocol::GoogleVertexGenerateContent => {
        Some(Self::GoogleGeneration { enveloped: false })
      }
      ModelUseProtocol::GoogleAntigravityGenerateContent => {
        Some(Self::GoogleGeneration { enveloped: true })
      }
      _ => None,
    }
  }

  /// Remove the hop boundary before the content decoder sees it. It is not a logical stop.
  pub fn inspect(self, data: &str) -> Result<(String, Option<Checkpoint>), Error> {
    let mut body: Value = serde_json::from_str(data)
      .map_err(|_| Error::Malformed("recovery record is not JSON".into()))?;
    let Self::GoogleGeneration { enveloped } = self;
    let root = if enveloped { &mut body["response"] } else { &mut body };
    let Some(candidate) = root.pointer_mut("/candidates/0") else {
      return Ok((data.to_owned(), None));
    };
    let continuation = candidate["finishReason"] == "CONTINUATION";
    let token = candidate
      .get("continuationToken")
      .and_then(Value::as_str)
      .filter(|s| !s.is_empty())
      .map(str::to_owned);
    if continuation && token.is_none() {
      return Err(Error::Malformed("CONTINUATION without a checkpoint".into()));
    }
    if continuation {
      candidate.as_object_mut().expect("candidate with finishReason").remove("finishReason");
    }
    Ok((body.to_string(), token.map(|token| Checkpoint { token, continuation })))
  }

  pub fn resume(self, original: &Call, token: &str) -> Result<Call, Error> {
    let mut call = original.clone();
    let mut body: Value = serde_json::from_slice(&call.body)
      .map_err(|_| Error::Build("invalid original recovery request".into()))?;
    let Self::GoogleGeneration { enveloped } = self;
    let request = if enveloped { &mut body["request"] } else { &mut body };
    request["continuationToken"] = Value::String(token.to_owned());
    call.body = serde_json::to_vec(&body).map_err(|e| Error::Build(e.to_string()))?;
    Ok(call)
  }

  pub fn permits(self, error: &Error) -> bool {
    matches!(error, Error::Transport(_)) || error.is_retryable()
  }
}

/// Usage is cumulative within a hop, additive between committed hops.
pub fn sum_usage(a: Usage, b: Usage) -> Usage {
  fn add(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    match (a, b) {
      (Some(a), Some(b)) => Some(a.saturating_add(b)),
      _ => a.or(b),
    }
  }
  Usage {
    input_tokens: add(a.input_tokens, b.input_tokens),
    cached_input_tokens: add(a.cached_input_tokens, b.cached_input_tokens),
    cache_write_input_tokens: add(a.cache_write_input_tokens, b.cache_write_input_tokens),
    output_tokens: add(a.output_tokens, b.output_tokens),
    reasoning_tokens: add(a.reasoning_tokens, b.reasoning_tokens),
    total_tokens: add(a.total_tokens, b.total_tokens),
  }
}

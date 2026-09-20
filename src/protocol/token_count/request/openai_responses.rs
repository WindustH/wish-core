use crate::protocol::{
  Request,
  error::Error,
  model_use::request::openai_responses::{self, ResponsesApiCompatMode, ResponsesDeployment},
};
use serde_json::Value;

pub fn render(request: &Request, mode: ResponsesApiCompatMode) -> Result<Value, Error> {
  if mode.deployment != ResponsesDeployment::Platform {
    return Err(Error::build_unsupported(
      "token counting",
      "openai_responses",
      "requires the platform deployment",
    ));
  }
  let mut body = openai_responses::render(request, mode)?;
  // The count endpoint has its own input schema: never forward generation-only controls.
  body.as_object_mut().expect("renderer returns an object").retain(|key, _| {
    matches!(key.as_str(), "model" | "input" | "tools" | "tool_choice" | "reasoning")
  });
  Ok(body)
}

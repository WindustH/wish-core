use crate::protocol::{
  Request,
  error::Error,
  model_use::{mode::ResponsesApiMode, request::openai_responses},
};
use serde_json::Value;

/// The count body of the platform deployment, the only one this endpoint pairs with - which
/// [`super::render`] has already checked.
pub fn render(request: &Request, mode: ResponsesApiMode) -> Result<Value, Error> {
  let mut body = openai_responses::render(request, mode)?;
  // The count endpoint has its own input schema: never forward generation-only controls.
  body.as_object_mut().expect("renderer returns an object").retain(|key, _| {
    matches!(key.as_str(), "model" | "input" | "tools" | "tool_choice" | "reasoning")
  });
  Ok(body)
}

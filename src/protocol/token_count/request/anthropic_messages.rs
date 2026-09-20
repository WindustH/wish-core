use crate::protocol::{Request, error::Error, model_use::request::anthropic_messages};
use serde_json::Value;

pub fn render(request: &Request) -> Result<Value, Error> {
  anthropic_messages::render_for_token_count(request)
}

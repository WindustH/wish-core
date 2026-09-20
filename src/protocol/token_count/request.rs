//! Count renderers reuse model-use input conversion, including tools and reasoning replay.
pub mod anthropic_messages;
pub mod google_generate_content;
pub mod openai_responses;

use super::TokenCountProtocol;
use crate::protocol::{Request, error::Error, model_use::ModelUseProtocol};
use serde_json::Value;

pub fn render(
  protocol: TokenCountProtocol,
  model_use: ModelUseProtocol,
  request: &Request,
) -> Result<Value, Error> {
  protocol.validate_model_use(model_use)?;
  match model_use {
    ModelUseProtocol::OpenAiResponses(mode) => openai_responses::render(request, mode),
    ModelUseProtocol::AnthropicMessages(_) => anthropic_messages::render(request),
    ModelUseProtocol::GoogleGenerateContent => google_generate_content::render(request),
    _ => unreachable!("validated token-count protocol pairing"),
  }
}

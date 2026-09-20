use super::{TokenCount, TokenCountProtocol};
use crate::protocol::error::Error;
use serde_json::Value;

pub fn decode(protocol: TokenCountProtocol, body: &Value) -> Result<TokenCount, Error> {
  let field = match protocol {
    TokenCountProtocol::OpenAiResponses | TokenCountProtocol::AnthropicMessages => "input_tokens",
    TokenCountProtocol::GoogleGenerateContent => "totalTokens",
  };
  let input_tokens = read_count(body, field)?;
  let cached_input_tokens = if protocol == TokenCountProtocol::GoogleGenerateContent
    && body.get("cachedContentTokenCount").is_some()
  {
    Some(read_count(body, "cachedContentTokenCount")?)
  } else {
    None
  };
  Ok(TokenCount { input_tokens, cached_input_tokens })
}
fn read_count(body: &Value, field: &str) -> Result<u64, Error> {
  body.get(field).and_then(Value::as_u64).ok_or_else(|| {
    Error::Malformed(format!("token count response requires a non-negative integer `{field}`"))
  })
}

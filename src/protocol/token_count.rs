//! Provider-side input token counting for the same Request used to call a model.
//! Availability is explicitly configured; a compatible generation API need not support counting.
pub mod request;
pub mod response;

use crate::protocol::{
  error::Error,
  model_use::{
    ModelUseProtocol,
    request::{anthropic_messages::MessagesApiCompatMode, openai_responses::ResponsesDeployment},
  },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenCountProtocol {
  OpenAiResponses,
  AnthropicMessages,
  GoogleGenerateContent,
}
impl TokenCountProtocol {
  pub const fn get_id(self) -> &'static str {
    match self {
      Self::OpenAiResponses => "openai_responses",
      Self::AnthropicMessages => "anthropic_messages",
      Self::GoogleGenerateContent => "google_generate_content",
    }
  }
  pub fn validate_model_use(self, model_use: ModelUseProtocol) -> Result<(), Error> {
    let supported = match (self, model_use) {
      (Self::OpenAiResponses, ModelUseProtocol::OpenAiResponses(mode)) => {
        mode.deployment == ResponsesDeployment::Platform
      }
      (
        Self::AnthropicMessages,
        ModelUseProtocol::AnthropicMessages(MessagesApiCompatMode::Official),
      ) => true,
      (Self::GoogleGenerateContent, ModelUseProtocol::GoogleGenerateContent) => true,
      _ => false,
    };
    if supported {
      Ok(())
    } else {
      Err(Error::build_unsupported(
        "token counting",
        self.get_id(),
        "does not support this model-use protocol or deployment",
      ))
    }
  }
  /// Derive a count endpoint from the configured, model-resolved generation endpoint.
  pub fn resolve_path(self, model_path: &str) -> Result<String, Error> {
    let path = model_path.trim_end_matches('/');
    match self {
      Self::OpenAiResponses => Ok(format!("{path}/input_tokens")),
      Self::AnthropicMessages => Ok(format!("{path}/count_tokens")),
      Self::GoogleGenerateContent => {
        let prefix = path
          .strip_suffix(":generateContent")
          .or_else(|| path.strip_suffix(":streamGenerateContent"))
          .ok_or_else(|| {
            Error::Build("token counting requires a generateContent endpoint".into())
          })?;
        Ok(format!("{prefix}:countTokens"))
      }
    }
  }
}

/// The provider's input count, not billed usage or a guarantee the model will accept the request.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TokenCount {
  pub input_tokens: u64,
  /// Present only when the counting endpoint reports a cached-input count.
  pub cached_input_tokens: Option<u64>,
}

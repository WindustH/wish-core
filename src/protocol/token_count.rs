//! A provider's count of the input tokens of the same [`Request`](crate::protocol::Request) a model
//! call would send, made without generating anything.
//!
//! Counting is its own endpoint beside the generation one, and not every service that generates
//! can count: availability is configured explicitly, and a pairing of a count protocol with a
//! model-use protocol this crate does not know is refused before any request. The count body reuses
//! the model-use renderer of the same wire - tools, reasoning replay and all - so what is counted is
//! what would be sent; [`request`] renders it and [`response`] reads the count back.

pub mod request;
pub mod response;

use crate::protocol::{
  error::Error,
  model_use::{
    ModelUseProtocol,
    mode::{MessagesApiCompatMode, ResponsesDeployment},
  },
};

text_id_enum! {
  /// The count endpoints this crate can ask, each beside the generation wire it counts for.
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub enum TokenCountProtocol (unknown: "unknown token-count protocol `{}`") {
    /// OpenAI Responses' `input_tokens` endpoint, on the platform deployment.
    OpenAiResponses => "openai_responses",
    /// Anthropic Messages' `count_tokens` endpoint, on Anthropic's own service.
    AnthropicMessages => "anthropic_messages",
    /// Gemini's `countTokens` verb, beside `generateContent`.
    GoogleGenerateContent => "google_generate_content",
  }
}

impl TokenCountProtocol {
  /// Refuses a pairing with a model-use protocol or deployment this count endpoint does not
  /// serve, before anything is sent.
  ///
  /// # Errors
  ///
  /// [`Error::Unsupported`] for such a pairing.
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
  /// The count endpoint's path, derived from the configured, model-resolved generation path.
  ///
  /// # Errors
  ///
  /// [`Error::Build`] when a Gemini path is not a `generateContent` one.
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

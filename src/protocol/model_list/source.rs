//! How each model-list protocol is asked: this tree's slice of the read-side table.
//!
//! The list protocols are implemented by many providers, so the host and the path always come from
//! the caller's query, and an entry only states what the wire requires - where the credential goes,
//! which headers the wire insists on. Bedrock signs its call with the account's own AWS
//! credentials instead of placing a key.
//!
//! The call an entry describes is built through [`Source`], with the caller's credentials.

use super::ModelListProtocol;
use crate::protocol::endpoint::{
  AuthScheme, CredentialField, Source, SourceHeader, copilot_oauth::API_HEADERS,
};

/// The source that answers for a model-list protocol. Every protocol has one.
pub(crate) fn find_source(protocol: ModelListProtocol) -> Source {
  let asked_with = |auth: AuthScheme, headers: &'static [SourceHeader]| Source {
    protocol: protocol.get_id(),
    base_url: None,
    path: None,
    auth,
    headers,
  };
  match protocol {
    // The list of a subscription sits beside its account endpoint and is read the same way.
    ModelListProtocol::OpenAiCodexModels => asked_with(
      AuthScheme::Bearer(None),
      &[SourceHeader::Credential("chatgpt-account-id", CredentialField::AccountId)],
    ),
    // DeepSeek mounts the OpenAI list at `/models`, almost everyone else at `/v1/models`, which is
    // why the host and the path belong to the caller's own configuration.
    ModelListProtocol::OpenAiModels | ModelListProtocol::QwenModels => {
      asked_with(AuthScheme::Bearer(None), &[])
    }
    // Anthropic reads its credential from a header of its own, and dates the API in another.
    ModelListProtocol::AnthropicModels => asked_with(
      AuthScheme::Header("x-api-key"),
      &[SourceHeader::Literal("anthropic-version", "2023-06-01")],
    ),
    // Google takes the key in `x-goog-api-key` (the `?key=` form is the other spelling of the same
    // thing, and a credential in a URL is a credential in every log line that ever touches it).
    ModelListProtocol::GoogleModels => asked_with(AuthScheme::Header("x-goog-api-key"), &[]),
    // Bedrock's list answers the account its signature names, on a host the region decides, and
    // the account's AWS credentials sign the call.
    ModelListProtocol::BedrockModels => asked_with(AuthScheme::SigV4, &[]),
    // Copilot answers the session token, asked as its editor asks it.
    ModelListProtocol::GitHubCopilotModels => asked_with(AuthScheme::Bearer(None), API_HEADERS),
  }
}

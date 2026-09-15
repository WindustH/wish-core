//! Where each catalog protocol is read from: this tree's slice of the read-side table.
//!
//! The catalog protocols are implemented by many providers, so most entries leave the host and the
//! path to the caller's own configuration and only state what the wire requires - where the
//! credential goes, which headers the wire insists on. Bedrock is the exception: its host is the
//! region's, and its call is signed with the account's own AWS material.
//!
//! The call an entry describes is built by [`Source`](crate::protocol::outbound::Source), with the
//! caller's credentials.

use super::ModelListProtocol;
use crate::protocol::outbound::{AuthProtocol, CredentialField};
use crate::protocol::outbound::{Source, SourceHeader};

/// The source that answers for a catalog protocol, when there is one.
pub(crate) fn source(protocol: ModelListProtocol) -> Option<&'static Source> {
  SOURCES.iter().find(|source| source.protocol == protocol.id())
}

/// Every model-list read this crate knows: one entry per readable protocol id.
const SOURCES: &[Source] = &[
  // The catalog of a subscription sits beside its account endpoint and is read the same way.
  Source {
    protocol: ModelListProtocol::OpenAiCodexModels.id(),
    base_url: Some("https://chatgpt.com"),
    path: Some("/backend-api/codex/models"),
    auth: AuthProtocol::Bearer(None),
    headers: &[SourceHeader::Credential("chatgpt-account-id", CredentialField::AccountId)],
  },
  // DeepSeek mounts the OpenAI list at `/models`, almost everyone else at `/v1/models`, so the host
  // and the path belong to the caller's own configuration.
  Source {
    protocol: ModelListProtocol::OpenAiModels.id(),
    base_url: None,
    path: None,
    auth: AuthProtocol::Bearer(None),
    headers: &[],
  },
  Source {
    protocol: ModelListProtocol::QwenModels.id(),
    base_url: None,
    path: None,
    auth: AuthProtocol::Bearer(None),
    headers: &[],
  },
  // Anthropic reads its credential from a header of its own, and dates the API in another.
  Source {
    protocol: ModelListProtocol::AnthropicModels.id(),
    base_url: None,
    path: None,
    auth: AuthProtocol::Header("x-api-key"),
    headers: &[SourceHeader::Literal("anthropic-version", "2023-06-01")],
  },
  // Google takes the key in `x-goog-api-key` (the `?key=` form is the other spelling of the same
  // thing, and a credential in a URL is a credential in every log line that ever touches it).
  Source {
    protocol: ModelListProtocol::GoogleModels.id(),
    base_url: None,
    path: None,
    auth: AuthProtocol::Header("x-goog-api-key"),
    headers: &[],
  },
  // Bedrock's catalog answers the account its signature names, on a host the region decides: the
  // base URL and the path are the caller's facts, and the account's AWS material signs the call.
  Source {
    protocol: ModelListProtocol::BedrockModels.id(),
    base_url: None,
    path: None,
    auth: AuthProtocol::SigV4,
    headers: &[],
  },
];

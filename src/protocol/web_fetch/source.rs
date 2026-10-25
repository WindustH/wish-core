//! Webpage-reading endpoints and authentication, this protocol tree's source table.
use super::FetchProtocol;
use crate::protocol::endpoint::{AuthScheme, CredentialField, Source, SourceHeader};
pub(super) fn find_source(protocol: FetchProtocol, has_key: bool) -> Source {
  let (base_url, path) = match protocol {
    FetchProtocol::DeepseekFetch => ("https://api.deepseek.com", "/v1/tools/fetch"),
    FetchProtocol::ZaiReader => ("https://api.z.ai/api/paas/v4", "/reader"),
    FetchProtocol::JinaReader => ("https://r.jina.ai", "/"),
    FetchProtocol::CodexFetch => ("https://chatgpt.com/backend-api/codex", "/alpha/search"),
  };
  Source {
    protocol: protocol.get_id(),
    base_url: Some(base_url),
    path: Some(path),
    auth: if protocol == FetchProtocol::JinaReader && !has_key {
      AuthScheme::None
    } else {
      AuthScheme::Bearer(None)
    },
    headers: if protocol == FetchProtocol::CodexFetch {
      &[
        SourceHeader::Literal("accept", "application/json"),
        SourceHeader::Literal("originator", "codex_cli_rs"),
        SourceHeader::Credential("chatgpt-account-id", CredentialField::AccountId),
      ]
    } else {
      &[SourceHeader::Literal("accept", "application/json")]
    },
  }
}

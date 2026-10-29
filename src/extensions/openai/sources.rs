//! Supplier-owned web service endpoints and authentication declarations.
use crate::protocol::endpoint::{AuthScheme, CredentialField, Source, SourceHeader};
pub fn search(id: &'static str) -> Source {
  Source {
    protocol: id,
    base_url: None,
    path: Some("/alpha/search"),
    auth: AuthScheme::Bearer(None),
    headers: &[
      SourceHeader::Literal("originator", "codex_cli_rs"),
      SourceHeader::Credential("chatgpt-account-id", CredentialField::AccountId),
    ],
  }
}
pub fn fetch(id: &'static str, _has_key: bool) -> Source {
  Source {
    protocol: id,
    base_url: Some("https://chatgpt.com/backend-api/codex"),
    path: Some("/alpha/search"),
    auth: AuthScheme::Bearer(None),
    headers: &[
      SourceHeader::Literal("accept", "application/json"),
      SourceHeader::Literal("originator", "codex_cli_rs"),
      SourceHeader::Credential("chatgpt-account-id", CredentialField::AccountId),
    ],
  }
}

//! Where each search protocol is asked: this tree's slice of the read-side table.
//!
//! A service with one host pins it here; a self-hosted one (SearXNG) leaves it to the caller's
//! configuration, and so does a subscription's search that lives beside the model API it came
//! with (Codex asks `{base}/alpha/search`, with the model provider's own base URL).

use super::SearchProtocol;
use crate::protocol::endpoint::{AuthScheme, CredentialField, Source, SourceHeader};

/// The source that answers for a search protocol. Every protocol has one.
pub(super) fn find_source(protocol: SearchProtocol) -> Source {
  let id = protocol.get_id();
  let at = |base_url: Option<&'static str>, path: &'static str, auth: AuthScheme| Source {
    protocol: id,
    base_url,
    path: Some(path),
    auth,
    headers: &[],
  };
  match protocol {
    // The same bearer and account header as the subscription's model calls, which already carry
    // the client's identity; the base URL is the model provider's.
    SearchProtocol::CodexAlphaSearch => Source {
      headers: &[
        SourceHeader::Literal("originator", "codex_cli_rs"),
        SourceHeader::Credential("chatgpt-account-id", CredentialField::AccountId),
      ],
      ..at(None, "/alpha/search", AuthScheme::Bearer(None))
    },
    SearchProtocol::TavilySearch => {
      at(Some("https://api.tavily.com"), "/search", AuthScheme::Bearer(None))
    }
    // Tavily's free access: the same endpoint, with the mode named instead of a key.
    SearchProtocol::TavilyKeylessSearch => Source {
      headers: &[SourceHeader::Literal("x-tavily-access-mode", "keyless")],
      ..at(Some("https://api.tavily.com"), "/search", AuthScheme::None)
    },
    SearchProtocol::ExaSearch => {
      at(Some("https://api.exa.ai"), "/search", AuthScheme::Header("x-api-key"))
    }
    SearchProtocol::PerplexitySearch => {
      at(Some("https://api.perplexity.ai"), "/search", AuthScheme::Bearer(None))
    }
    SearchProtocol::BraveSearch => Source {
      headers: &[SourceHeader::Literal("accept", "application/json")],
      ..at(
        Some("https://api.search.brave.com"),
        "/res/v1/web/search",
        AuthScheme::Header("x-subscription-token"),
      )
    },
    SearchProtocol::SerperSearch => {
      at(Some("https://google.serper.dev"), "/search", AuthScheme::Header("x-api-key"))
    }
    SearchProtocol::JinaSearch => at(Some("https://s.jina.ai"), "/", AuthScheme::Bearer(None)),
    SearchProtocol::BochaSearch => {
      at(Some("https://api.bocha.cn"), "/v1/web-search", AuthScheme::Bearer(None))
    }
    SearchProtocol::MetasoSearch => Source {
      headers: &[SourceHeader::Literal("accept", "application/json")],
      ..at(Some("https://metaso.cn"), "/api/v1/search", AuthScheme::Bearer(None))
    },
    // Beside the Kimi Code model API: the base URL is the model provider's.
    SearchProtocol::KimiCodeSearch => at(None, "/search", AuthScheme::Bearer(None)),
    // On the host of the plan's region, which the lending provider's preset decides.
    SearchProtocol::MinimaxCodingPlanSearch => {
      at(None, "/v1/coding_plan/search", AuthScheme::Bearer(None))
    }
    // Wherever the instance is: the caller's configuration says.
    SearchProtocol::SearxngSearch => at(None, "/search", AuthScheme::None),
  }
}

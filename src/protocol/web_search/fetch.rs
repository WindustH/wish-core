//! One search over the network.
//!
//! The round trip between the two pure halves of a protocol: its draft of the request and its
//! reading of the reply. Around them, what every service shares: the reply's status and error
//! envelope, and the filters and limit a service could not apply itself. One attempt, with no
//! policy of its own: whether another service should be asked is the caller's decision.

use std::collections::HashSet;

use super::source::find_source;
use super::{SearchProtocol, SearchQuery, SearchResult, SearchResults};
use crate::protocol::attempt::Transport;
use crate::protocol::endpoint::Credentials;
use crate::protocol::error::Error;
use crate::protocol::http_error;

/// Searches once.
///
/// `base_url` overrides the source's own host, for a service at an address of its own (a
/// self-hosted instance, a regional twin); `headers` are added to the request, for what the
/// caller's configuration places there.
///
/// # Errors
///
/// [`Error::Build`] when the request cannot be addressed, [`Error::Transport`] when the network
/// fails before a reply exists, [`Error::Upstream`] for a failure the service reports, in a
/// non-`2xx` reply or inside a successful one, and [`Error::Malformed`] for a body that is not
/// what the service promised.
pub async fn search<T: Transport>(
  transport: &T,
  protocol: SearchProtocol,
  credentials: &Credentials,
  base_url: Option<&str>,
  headers: &[(String, String)],
  query: &SearchQuery,
  now: u64,
) -> Result<SearchResults, Error> {
  let mut draft = protocol.build_draft(query, now)?;
  draft.headers.extend(headers.iter().cloned());
  let call =
    find_source(protocol).to_endpoint(base_url, None)?.build_call(draft, credentials, now)?;
  let reply = transport.execute(&call).await?;
  if let Some(error) = protocol.explain_refusal(reply.status) {
    return Err(error);
  }
  let body = http_error::read_provider_json(reply, protocol.get_id())?;
  Ok(apply_local_filters(protocol, query, protocol.parse_body(&body)?))
}

/// Applies what the service could not be trusted with: the domain filters, the limit, and one
/// entry per page; and says so when the period asked for could not be applied at all.
fn apply_local_filters(
  protocol: SearchProtocol,
  query: &SearchQuery,
  found: Vec<SearchResult>,
) -> SearchResults {
  let mut warnings = Vec::new();
  let mut seen = HashSet::new();
  let results = found
    .into_iter()
    .filter(|result| !result.url.is_empty() && seen.insert(result.url.clone()))
    .filter(|result| {
      query.allowed_domains.is_empty() || is_on_any(&result.url, &query.allowed_domains)
    })
    .filter(|result| !is_on_any(&result.url, &query.blocked_domains))
    .take(query.limit.max(1))
    .collect();
  if query.recency.is_some() && !protocol.can_filter_recency() {
    warnings.push(
      "This search service cannot limit results by date; check each result's date yourself."
        .to_owned(),
    );
  }
  SearchResults { results, warnings }
}

/// Whether a page's host is one of the domains or below one of them.
fn is_on_any(url: &str, domains: &[String]) -> bool {
  let Some(host) = get_host(url) else { return false };
  domains.iter().any(|domain| {
    let domain =
      domain.trim().trim_start_matches("www.").trim_end_matches('.').to_ascii_lowercase();
    !domain.is_empty() && (host == domain || host.ends_with(&format!(".{domain}")))
  })
}

fn get_host(url: &str) -> Option<String> {
  let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
  let authority = rest.split(['/', '?', '#']).next()?;
  let host = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
  let host = host.split(':').next()?.trim_end_matches('.').to_ascii_lowercase();
  (!host.is_empty()).then(|| host.trim_start_matches("www.").to_owned())
}

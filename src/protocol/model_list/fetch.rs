//! Reading a provider's model list over the network.
//!
//! The same round trip as an account read: the host and path from this tree's own source table,
//! the wire's query, one `GET`, then the protocol's reading of the body. It carries no policy of
//! its own: one attempt, and whether a failure is worth another try is the caller's decision.
//! Paging is the caller's loop too - this reads one page and hands the next page's cursor back.

use super::source::source;
use crate::protocol::error::Error;
use crate::protocol::http_error;
use crate::protocol::model_list::{
  ModelCatalog, ModelListProtocol, Unsupported, page_query, parse_catalog_page,
};
use crate::protocol::outbound::Credentials;
use crate::protocol::wire::Transport;

/// The page size asked for when a caller has no opinion.
pub const DEFAULT_PAGE_SIZE: u32 = 100;

/// Which page of which list to read.
#[derive(Clone, Debug)]
pub struct ModelListQuery {
  /// The host, which the caller owns: one catalog protocol is served from many providers.
  pub base_url: String,
  /// The path on that host, `/v1/models` or `/models` where the service mounts it at the root.
  pub path: String,
  /// The cursor the previous page ended with; `None` reads the first page.
  pub cursor: Option<String>,
  /// How many models to ask for, where the wire has a page size.
  pub page_size: u32,
}

impl ModelListQuery {
  /// The first page of a list, at this crate's default page size.
  pub fn first(base_url: &str, path: &str) -> Self {
    Self {
      base_url: base_url.to_owned(),
      path: path.to_owned(),
      cursor: None,
      page_size: DEFAULT_PAGE_SIZE,
    }
  }
}

/// Reads one page of a provider's model list.
///
/// # Errors
///
/// [`Error::Unsupported`] when no source serves the protocol or the credentials are incomplete,
/// [`Error::Transport`] when the
/// network fails before a reply exists, [`Error::Upstream`] for a non-`2xx` reply, and
/// [`Error::Malformed`] when the body is not the page its protocol promised.
pub async fn fetch<T: Transport>(
  transport: &T,
  protocol: ModelListProtocol,
  query: &ModelListQuery,
  credentials: &Credentials,
  now: u64,
) -> Result<ModelCatalog, Error> {
  let source = source(protocol)
    .ok_or_else(|| Error::unsupported("model list", protocol, Unsupported::NoListing.text()))?;
  let wire_query = page_query(protocol, query.cursor.as_deref(), query.page_size)?;
  let call =
    source.call(Some(&query.base_url), Some(&query.path), &wire_query, credentials, now)?;
  let reply = transport.execute(&call).await?;
  if !reply.is_success() {
    let error = http_error::provider_envelope(reply.status, &reply.body);
    return Err(error.with_retry_after(reply.retry_after_ms()));
  }
  let body = http_error::json_body(protocol.id(), &reply.body)?;
  parse_catalog_page(protocol, &body)
}

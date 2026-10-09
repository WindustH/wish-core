//! Reading a provider's model list over the network.
//!
//! The same round trip as an account read: the auth and headers from this tree's own source table
//! over the headers the provider's configuration sets, the host, path and wire query of the page
//! asked for, one `GET`, then the protocol's reading of the body. It carries no policy of its own: one attempt, and whether a failure is worth another
//! try is the caller's decision. Paging is the caller's loop too - this reads one page and hands the
//! next page's cursor back.

use super::source::find_source;
use crate::protocol::attempt::Transport;
use crate::protocol::endpoint::{AuthScheme, Credentials, Draft};
use crate::protocol::error::Error;
use crate::protocol::http_error;
use crate::protocol::model_list::{ModelListPage, ModelListProtocol, build_page_query, parse_page};

/// The page size asked for when a caller has no opinion.
pub const DEFAULT_PAGE_SIZE: u32 = 100;

/// Which page of which list to read.
#[derive(Clone, Debug)]
pub struct ModelListQuery {
  /// The host, which the caller owns: one list protocol is served from many providers.
  pub base_url: String,
  /// The path on that host, `/v1/models` or `/models` where the service mounts it at the root.
  pub path: String,
  /// The cursor the previous page ended with; `None` reads the first page.
  pub cursor: Option<String>,
  /// How many models to ask for, where the wire has a page size.
  pub page_size: u32,
  /// Read an OpenAI-shaped list without a key (for local servers with no authentication).
  pub unauthenticated: bool,
}

impl ModelListQuery {
  /// The first page of a list, at this crate's default page size.
  pub fn first(base_url: &str, path: &str) -> Self {
    Self {
      base_url: base_url.to_owned(),
      path: path.to_owned(),
      cursor: None,
      page_size: DEFAULT_PAGE_SIZE,
      unauthenticated: false,
    }
  }
}

/// Reads one page of a provider's model list.
///
/// `headers` are the ones the provider's configuration sets on every call to it - a workspace an
/// organization key is addressed to, say. The list protocol's own headers go on top.
///
/// # Errors
///
/// [`Error::Build`] when the credentials are incomplete, [`Error::Transport`] when the network
/// fails before a reply exists, [`Error::Upstream`] for a non-`2xx` reply, and [`Error::Malformed`]
/// when the body is not the page its protocol promised.
pub async fn fetch<T: Transport>(
  transport: &T,
  protocol: ModelListProtocol,
  query: &ModelListQuery,
  headers: &[(String, String)],
  credentials: &Credentials,
  now: u64,
) -> Result<ModelListPage, Error> {
  let mut source = find_source(protocol);
  if query.unauthenticated && protocol == ModelListProtocol::OpenAiModels {
    source.auth = AuthScheme::None;
  }
  let draft = Draft::get(build_page_query(protocol, query.cursor.as_deref(), query.page_size));
  let call = source
    .to_endpoint(Some(&query.base_url), Some(&query.path))?
    .beneath(headers)
    .build_call(draft, credentials, now)?;
  let reply = transport.execute(&call).await?;
  parse_page(protocol, &http_error::read_provider_json(reply, protocol.get_id())?)
}

//! The static form of an endpoint, as a read-side tree states one for each of its protocols.
//!
//! Where a protocol is read from is that protocol's own knowledge, so each tree that reads a
//! service - [`account_state`](crate::protocol::account_state),
//! [`model_list`](crate::protocol::model_list), [`web_search`](crate::protocol::web_search) - keeps
//! a `find_source` of its own: one match over its protocols, so the compiler checks that none is
//! left without its row. What lives here is what every row shares: its shape (`&'static`
//! everything, so a row costs nothing to state), and its turning into an [`Endpoint`], where the
//! caller's host and path fill in or override the row's, and a row left without either is refused
//! with the protocol's own name.
//!
//! A source is data, and how its calls are proven is part of that data - an [`AuthScheme`] naming
//! where a credential sits, or the account's own AWS credentials when that is what the service
//! takes. What a source asks for is not always one key: a path may carry `{field}` placeholders - a
//! workspace, a team - and a header may take its value from the account's own credentials. Both
//! are filled when the call is built, and an account that does not carry what the source names is
//! a configuration mistake rather than a request sent half-addressed. The method, the query and
//! the body are the draft's: an account read or a model list is a `GET`, a search is often a
//! `POST`, and the same row serves either.

use crate::protocol::endpoint::{AuthScheme, CredentialField, Endpoint};
use crate::protocol::error::Error;

/// One endpoint a read-side protocol is read from.
#[derive(Clone, Debug)]
pub(crate) struct Source {
  /// The protocol this source answers for, by the id it is known by in text.
  pub(crate) protocol: &'static str,
  /// The host that serves it, for the protocols that have exactly one.
  pub(crate) base_url: Option<&'static str>,
  /// The path on that host, for the protocols whose path is the same everywhere.
  pub(crate) path: Option<&'static str>,
  /// How the call proves the account that asked for the read.
  pub(crate) auth: AuthScheme,
  /// Headers the wire requires, independent of what an account answers for.
  pub(crate) headers: &'static [SourceHeader],
}

/// One header a wire requires, and where its value comes from.
#[derive(Clone, Copy, Debug)]
pub(crate) enum SourceHeader {
  /// The same value for every account.
  Literal(&'static str, &'static str),
  /// A value out of the account's own credentials, such as the workspace it is addressed by.
  Credential(&'static str, CredentialField),
}

impl Source {
  /// The endpoint this source describes, with `base_url` and `path` filling in - or overriding -
  /// the host and the path the source states.
  pub(crate) fn to_endpoint(
    &self,
    base_url: Option<&str>,
    path: Option<&str>,
  ) -> Result<Endpoint, Error> {
    let base = base_url
      .or(self.base_url)
      .ok_or_else(|| Error::Build(format!("`{}` needs a base URL", self.protocol)))?;
    let path = path
      .or(self.path)
      .ok_or_else(|| Error::Build(format!("`{}` needs a path", self.protocol)))?;
    Ok(Endpoint {
      base_url: base.to_owned(),
      path: path.to_owned(),
      headers: self
        .headers
        .iter()
        .filter_map(|header| match header {
          SourceHeader::Literal(name, value) => Some(((*name).to_owned(), (*value).to_owned())),
          SourceHeader::Credential(..) => None,
        })
        .collect(),
      credential_headers: self
        .headers
        .iter()
        .filter_map(|header| match header {
          SourceHeader::Credential(name, credential) => Some((*name, *credential)),
          SourceHeader::Literal(..) => None,
        })
        .collect(),
      auth: self.auth,
      session_id: None,
    })
  }
}

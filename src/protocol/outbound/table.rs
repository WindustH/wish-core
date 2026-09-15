//! The static form a read-side table states, materialized into a target when a call is made.
//!
//! The tables of entries live in the trees they serve - `account`, `model_list` - because where a
//! protocol is read from is that protocol's own knowledge. What lives here is what every entry
//! shares: the shape of one entry as a table can state it (`&'static` everything, so a whole
//! table is one `const`), and its materialization into an [`Outbound`], where the caller's
//! overrides are applied and the missing parts are refused with the protocol's own name.
//!
//! Every read is a `GET` with no body, and a source is data. How the call is proven is part of
//! that data - an [`AuthProtocol`] naming where a credential sits, or the account's own AWS material
//! when that is what the service takes. What a source asks for is not always one key: a path may
//! carry `{field}` placeholders - a workspace, a team - and a header may take its value from the
//! account's own material. Both are filled when the call is dispatched, and an account that does
//! not carry what the source names is a configuration mistake rather than a request sent
//! half-addressed.

use crate::protocol::error::Error;
use crate::protocol::outbound::{AuthProtocol, CredentialField, Draft, Outbound};
use crate::protocol::wire::{Call, Method};

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
  pub(crate) auth: AuthProtocol,
  /// Headers the wire requires, independent of what an account answers for.
  pub(crate) headers: &'static [SourceHeader],
}

/// One header a wire requires, and where its value comes from.
#[derive(Clone, Copy, Debug)]
pub(crate) enum SourceHeader {
  /// The same value for every account.
  Literal(&'static str, &'static str),
  /// A value out of the account's own material, such as the workspace it is addressed by.
  Credential(&'static str, CredentialField),
}

impl Source {
  /// Builds the one call this source describes, with the caller's credential placed in it.
  ///
  /// `base_url` and `path` fill in what the source leaves open; `query` is rendered in order,
  /// with values percent-encoded, because a cursor is an opaque token that may carry anything.
  pub(crate) fn call(
    &self,
    base_url: Option<&str>,
    path: Option<&str>,
    query: &[(String, String)],
    credentials: &crate::protocol::outbound::Credentials,
    now: u64,
  ) -> Result<Call, Error> {
    let draft = Draft {
      method: Method::Get,
      path: None,
      query: query.to_vec(),
      headers: Vec::new(),
      body: Vec::new(),
    };
    self.outbound(base_url, path)?.dispatch(draft, credentials, now)
  }

  fn outbound(&self, base_url: Option<&str>, path: Option<&str>) -> Result<Outbound, Error> {
    let base = base_url
      .or(self.base_url)
      .ok_or_else(|| Error::Build(format!("`{}` needs a base URL", self.protocol)))?;
    let path = path
      .or(self.path)
      .ok_or_else(|| Error::Build(format!("`{}` needs a path", self.protocol)))?;
    Ok(Outbound {
      base_url: base.to_owned(),
      path: Some(path.to_owned()),
      headers: self
        .headers
        .iter()
        .filter_map(|header| match header {
          SourceHeader::Literal(name, value) => Some(((*name).to_owned(), (*value).to_owned())),
          SourceHeader::Credential(..) => None,
        })
        .collect(),
      material_headers: self
        .headers
        .iter()
        .filter_map(|header| match header {
          SourceHeader::Credential(name, credential) => Some((*name, *credential)),
          SourceHeader::Literal(..) => None,
        })
        .collect(),
      auth: self.auth.clone(),
    })
  }
}

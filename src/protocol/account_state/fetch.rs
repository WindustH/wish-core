//! Reading one account's usage over the network.
//!
//! The round trip between the two pure halves: `source` knows where a protocol is read from,
//! [`parse_account_body`] knows what its body means. What happens here is one attempt at that call:
//! it carries no policy of its own, because whether a failure is worth another try is the caller's
//! decision to make.

use super::source::find_source;
use super::{AccountState, AccountStateProtocol, Unsupported, headers, parse_account_body};
use crate::protocol::attempt::Transport;
use crate::protocol::endpoint::{Credentials, Draft};
use crate::protocol::error::Error;
use crate::protocol::http_error;

/// Reads one account's usage.
///
/// `protocol` names the read-side protocol and `credentials` the account to read it for.
/// `base_url` overrides the source's own host for an account that lives in another region; most
/// callers pass `None`.
///
/// # Errors
///
/// [`Error::Unsupported`] when no source serves the protocol (a reading that rides a served reply
/// instead), [`Error::Build`] when the credentials are incomplete, [`Error::Transport`] when the
/// network fails before a reply exists, [`Error::Upstream`] for a non-`2xx` reply, and
/// [`Error::Malformed`] when the body is not the JSON it promised. A failure the service reports
/// inside a successful body is not an error: it lands in [`AccountState::failure`].
pub async fn fetch<T: Transport>(
  transport: &T,
  protocol: AccountStateProtocol,
  credentials: &Credentials,
  base_url: Option<&str>,
  now: u64,
) -> Result<AccountState, Error> {
  let Some(source) = find_source(protocol) else {
    let reason = if headers::is_header_dialect(protocol) {
      Unsupported::RidesTheHeaders
    } else {
      Unsupported::RidesAReply
    };
    return Err(Error::build_unsupported("account state", protocol, reason.get_text()));
  };
  let call =
    source.to_endpoint(base_url, None)?.build_call(Draft::get(Vec::new()), credentials, now)?;
  let reply = transport.execute(&call).await?;
  parse_account_body(protocol, &http_error::read_provider_json(reply, protocol.get_id())?)
}

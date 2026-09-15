//! Reading one account's usage over the network.
//!
//! The round trip between the two pure halves: `source` knows
//! where a protocol is read from, [`parse_account_body`] knows what its
//! body means. What happens here is one attempt at that call: it carries no policy of its own,
//! because whether a failure is worth another try is the caller's decision to make.

use super::Unsupported;
use super::headers;
use super::source::source;
use crate::protocol::error::Error;
use crate::protocol::http_error;
use crate::protocol::outbound::Credentials;
use crate::protocol::wire::Transport;

use super::{AccountState, AccountStateProtocol, parse_account_body};

/// Reads one account's usage.
///
/// `protocol` names the read-side protocol and `credentials` the account to read it for.
/// `base_url` overrides the source's own host for an account that lives in another region; most
/// callers pass `None`.
///
/// # Errors
///
/// [`Error::Unsupported`] when no source serves the protocol (a passive protocol, or one whose endpoint is
/// not pinned down yet) or the credentials are incomplete, [`Error::Transport`] when the network
/// fails before a reply exists, [`Error::Upstream`] for a non-`2xx` reply, and [`Error::Malformed`]
/// when the body is not the JSON it promised. A failure the service reports inside a successful
/// body is not an error: it lands in [`AccountState::failure`].
pub async fn fetch<T: Transport>(
  transport: &T,
  protocol: AccountStateProtocol,
  credentials: &Credentials,
  base_url: Option<&str>,
  now: u64,
) -> Result<AccountState, Error> {
  let reason = if headers::is_header_dialect(protocol) {
    Unsupported::RidesTheHeaders
  } else {
    Unsupported::RidesAReply
  };
  let source =
    source(protocol).ok_or_else(|| Error::unsupported("account state", protocol, reason.text()))?;
  let call = source.call(base_url, None, &[], credentials, now)?;
  let reply = transport.execute(&call).await?;
  if !reply.is_success() {
    let error = http_error::provider_envelope(reply.status, &reply.body);
    return Err(error.with_retry_after(reply.retry_after_ms()));
  }
  parse_account_body(protocol, &http_error::json_body(protocol.id(), &reply.body)?)
}

//! A single page-reading attempt through an injected transport. No retry or service policy.
use super::{FetchProtocol, FetchRequest, FetchResult, source::find_source};
use crate::protocol::{attempt::Transport, endpoint::Credentials, error::Error, http_error};
pub async fn fetch<T: Transport>(
  transport: &T,
  protocol: FetchProtocol,
  credentials: &Credentials,
  base_url: Option<&str>,
  headers: &[(String, String)],
  request: &FetchRequest,
  now: u64,
) -> Result<FetchResult, Error> {
  let mut draft = protocol.build_draft(request)?;
  draft.headers.extend(headers.iter().cloned());
  let call = find_source(protocol, !credentials.api_key.is_empty())
    .to_endpoint(base_url, None)?
    .build_call(draft, credentials, now)?;
  let reply = transport.execute(&call).await?;
  let body = http_error::read_provider_json(reply, protocol.get_id())?;
  protocol.parse_body(&body)
}

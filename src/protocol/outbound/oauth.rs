//! The one request that exchanges account material for account material: a token endpoint.
//!
//! Conversions:
//! - A refresh is a `POST` with a **JSON** body - not the form encoding token endpoints usually
//!   take - naming the client, the grant and the refresh token, answered with an access token, a
//!   rotated refresh token and an id token.
//! - The account a subscription is addressed by is the response's own field where it names one, and
//!   the id token's `https://api.openai.com/auth.chatgpt_account_id` claim otherwise.
//! - `expires_at` is the access token's own `exp` claim: the token is read, never verified, because
//!   the service that signed it is the one that will check it.
//!
//! Constraints:
//! - A credential without a refresh token is a configuration mistake, not a request: the exchange is
//!   not attempted.
//! - A response without an access token is not a token pair, whatever else it carries.
//!
//! Trade-offs:
//! - One attempt, no retry: whether a refused exchange (`invalid_grant`, say) is worth trying again
//!   is the caller's decision, exactly as it is for an account read.
//! - What comes back is what the endpoint said. Deciding when to refresh at all, and storing the
//!   rotated refresh token, stays with the caller - this crate holds no account state.
//! - The client the exchange is asked as is the vendor's own login client, because the endpoint only
//!   knows that one. A caller who needs another sends it themselves; nothing here ships a second.

use serde_json::{Value, json};

use crate::Error;
use crate::protocol::http_error;
use crate::protocol::outbound::{AuthProtocol, Credentials, Draft, Outbound, Tokens};
use crate::protocol::wire::{Method, Transport};

/// Who the exchange is asked as.
pub(crate) const ISSUER: &str = "https://auth.openai.com";
/// Where the issuer answers token requests.
const TOKEN_PATH: &str = "/oauth/token";
/// The client the vendor's own login flow authorises.
pub(crate) const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

/// Exchanges a refresh token for a fresh token pair.
///
/// # Errors
///
/// [`Error::Build`] when the credential carries no refresh token or its body cannot be built,
/// [`Error::Transport`] when the network fails before a reply exists, [`Error::Upstream`] for a
/// refused exchange, and [`Error::Malformed`] when the reply is not a token pair.
pub async fn refresh<T: Transport>(
  transport: &T,
  credentials: &Credentials,
) -> Result<Tokens, Error> {
  let refresh_token =
    credentials.refresh_token.as_deref().filter(|token| !token.is_empty()).ok_or_else(|| {
      Error::Build(
        "the token endpoint wants a refresh token, and this credential carries none".to_owned(),
      )
    })?;
  let body = json!({
    "client_id": CLIENT_ID,
    "grant_type": "refresh_token",
    "refresh_token": refresh_token,
  });
  // The token request is dispatched through the same join every other request takes, so the
  // exchange holds no HTTP of its own: the target is a constant, the draft is the one body.
  let issuer = {
    #[cfg(debug_assertions)]
    {
      std::env::var("WISH_TEST_CODEX_ISSUER").unwrap_or_else(|_| ISSUER.to_owned())
    }
    #[cfg(not(debug_assertions))]
    {
      ISSUER.to_owned()
    }
  };
  let target = Outbound::new(&issuer, TOKEN_PATH, AuthProtocol::None)?;
  let draft = Draft {
    method: Method::Post,
    path: None,
    query: Vec::new(),
    headers: vec![("content-type".to_owned(), "application/json".to_owned())],
    body: serde_json::to_vec(&body)
      .map_err(|error| Error::Build(format!("token request body is not serializable: {error}")))?,
  };
  // The exchange dispatches no material of the account it renews: the judgment dispatch makes
  // cannot refuse the renewal itself.
  let call = target.dispatch(draft, &Credentials::default(), 0)?;
  let reply = transport.execute(&call).await?;
  if !reply.is_success() {
    let error = http_error::decode_provider_envelope(reply.status, &reply.body);
    return Err(error.with_retry_after(reply.get_retry_after_ms()));
  }
  let body = http_error::decode_json_body("openai_codex_oauth", &reply.body)?;
  decode_tokens(&body)
}

/// Reads a token response: the pair, plus what the tokens claim about the account.
pub(crate) fn decode_tokens(body: &Value) -> Result<Tokens, Error> {
  let access_token = body
    .get("access_token")
    .and_then(Value::as_str)
    .filter(|token| !token.is_empty())
    .ok_or_else(|| Error::Malformed("the token response carries no `access_token`".to_owned()))?;
  let id_token = body.get("id_token").and_then(Value::as_str).filter(|token| !token.is_empty());
  Ok(Tokens {
    access_token: access_token.to_owned(),
    refresh_token: body
      .get("refresh_token")
      .and_then(Value::as_str)
      .filter(|value| !value.is_empty())
      .map(str::to_owned),
    id_token: id_token.map(str::to_owned),
    account_id: body
      .get("account_id")
      .and_then(Value::as_str)
      .filter(|value| !value.is_empty())
      .map(str::to_owned)
      .or_else(|| {
        // Codex's ID token puts the account in its namespaced auth claim.
        id_token
          .and_then(|token| token.split('.').nth(1))
          .and_then(decode_base64url)
          .and_then(|payload| serde_json::from_slice::<Value>(&payload).ok())
          .and_then(|claims| {
            claims
              .get("https://api.openai.com/auth")
              .and_then(|auth| auth.get("chatgpt_account_id"))
              .or_else(|| claims.get("chatgpt_account_id"))
              .and_then(Value::as_str)
              .filter(|value| !value.is_empty())
              .map(str::to_owned)
          })
      }),
    // The same one-claim read for the lifetime, in seconds since the epoch.
    expires_at: access_token
      .split('.')
      .nth(1)
      .and_then(decode_base64url)
      .and_then(|payload| serde_json::from_slice::<Value>(&payload).ok())
      .and_then(|claims| claims.get("exp").cloned())
      .and_then(|exp| exp.as_u64())
      .or_else(|| {
        body.get("expires_in").and_then(Value::as_u64).map(|seconds| {
          std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .saturating_add(seconds)
        })
      }),
  })
}

/// The decode_base64url payload of a JWT segment, decoded here rather than through a dependency: one claim
/// reader is not worth a crate.
fn decode_base64url(encoded: &str) -> Option<Vec<u8>> {
  let mut bytes = Vec::with_capacity(encoded.len() / 4 * 3);
  let mut accumulator = 0u32;
  let mut bits = 0u32;
  for character in encoded.bytes() {
    let value = match character {
      b'A'..=b'Z' => character - b'A',
      b'a'..=b'z' => character - b'a' + 26,
      b'0'..=b'9' => character - b'0' + 52,
      b'-' => 62,
      b'_' => 63,
      // Padding is optional in this alphabet and carries no bits.
      b'=' => continue,
      _ => return None,
    };
    accumulator = (accumulator << 6) | u32::from(value);
    bits += 6;
    if bits >= 8 {
      bits -= 8;
      bytes.push((accumulator >> bits) as u8);
    }
  }
  Some(bytes)
}

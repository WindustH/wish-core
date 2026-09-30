//! The Codex subscription's token endpoint: the requests that exchange credentials for credentials.
//!
//! Conversions:
//! - A refresh is a `POST` with a **JSON** body - not the form encoding token endpoints usually
//!   take - naming the client, the grant and the refresh token, answered with an access token, a
//!   rotated refresh token and an id token.
//! - A browser login's first exchange is the usual form-encoded `authorization_code` grant: the
//!   code the issuer redirected with, the redirect it was sent to, and the PKCE verifier its
//!   challenge was made from.
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
use crate::protocol::attempt::{Reply, Transport};
use crate::protocol::endpoint::{AuthScheme, Credentials, Draft, Endpoint, Tokens};
use crate::protocol::http_error;
use crate::protocol::json_read::read_string_member;
use crate::utils::time::unix_seconds;

/// Who the exchanges are asked of.
const ISSUER: &str = "https://auth.openai.com";
/// Where the issuer answers token requests.
const TOKEN_PATH: &str = "/oauth/token";
/// The client the vendor's own login flow authorises.
pub(crate) const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

/// The issuer the exchanges and a browser login are asked of, without a trailing `/`, so a path
/// can follow it.
///
/// A debug build asks the one `WISH_TEST_CODEX_ISSUER` names instead, so the tests can stand one up.
pub(crate) fn issuer() -> String {
  #[cfg(debug_assertions)]
  if let Ok(value) = std::env::var("WISH_TEST_CODEX_ISSUER") {
    return value.trim_end_matches('/').to_owned();
  }
  ISSUER.to_owned()
}

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
  let draft = Draft::post_json(&json!({
    "client_id": CLIENT_ID,
    "grant_type": "refresh_token",
    "refresh_token": refresh_token,
  }))?;
  let reply = send_token_request(transport, &issuer(), draft).await?;
  decode_tokens(&http_error::read_provider_json(reply, "openai_codex_oauth")?)
}

/// Sends a browser login's authorization code to `issuer`'s token endpoint, with the redirect it
/// came back to and the PKCE verifier its challenge was made from.
///
/// The reply is handed back unread: how a refused login reads to the person who just signed in is
/// the login's to word, and [`decode_tokens`] reads the pair out of a successful one.
///
/// # Errors
///
/// [`Error::Build`] when the call cannot be built, and [`Error::Transport`] when the network fails
/// before a reply exists.
pub(crate) async fn post_authorization_code<T: Transport>(
  transport: &T,
  issuer: &str,
  code: &str,
  redirect_uri: &str,
  verifier: &str,
) -> Result<Reply, Error> {
  // The URL's own query serializer is the form encoding a token endpoint reads.
  let mut form = reqwest::Url::parse("https://form.invalid/")
    .map_err(|error| Error::Build(format!("the token request form cannot be built: {error}")))?;
  form
    .query_pairs_mut()
    .append_pair("grant_type", "authorization_code")
    .append_pair("client_id", CLIENT_ID)
    .append_pair("code", code)
    .append_pair("redirect_uri", redirect_uri)
    .append_pair("code_verifier", verifier);
  send_token_request(transport, issuer, Draft::post_form(form.query().unwrap_or_default())).await
}

/// Sends one request to the issuer's token endpoint.
///
/// The token request goes through the same join every other request takes, so the exchange holds
/// no HTTP of its own. It carries no credentials of the account it renews, so the expiry check that
/// join makes cannot refuse the renewal itself.
async fn send_token_request<T: Transport>(
  transport: &T,
  issuer: &str,
  draft: Draft,
) -> Result<Reply, Error> {
  let endpoint = Endpoint::new(issuer, TOKEN_PATH, AuthScheme::None)?;
  let call = endpoint.build_call(draft, &Credentials::default(), 0)?;
  transport.execute(&call).await
}

/// Reads a token response: the pair, plus what the tokens claim about the account.
pub(crate) fn decode_tokens(body: &Value) -> Result<Tokens, Error> {
  let access_token = read_string_member(Some(body), "access_token")
    .ok_or_else(|| Error::Malformed("the token response carries no `access_token`".to_owned()))?;
  let id_token = read_string_member(Some(body), "id_token");
  let account_id = read_string_member(Some(body), "account_id").or_else(|| {
    // Codex's ID token puts the account in its namespaced auth claim.
    let claims = read_jwt_claims(id_token.as_deref()?)?;
    let account = claims
      .get("https://api.openai.com/auth")
      .and_then(|auth| auth.get("chatgpt_account_id"))
      .or_else(|| claims.get("chatgpt_account_id"))?;
    account.as_str().filter(|value| !value.is_empty()).map(str::to_owned)
  });
  // The lifetime is the access token's own claim, in seconds since the epoch, or failing that the
  // response's `expires_in` counted from now.
  let expires_at =
    read_jwt_claims(&access_token).and_then(|claims| claims.get("exp")?.as_u64()).or_else(|| {
      let seconds = body.get("expires_in").and_then(Value::as_u64)?;
      Some(unix_seconds().saturating_add(seconds))
    });
  Ok(Tokens {
    refresh_token: read_string_member(Some(body), "refresh_token"),
    access_token,
    id_token,
    account_id,
    expires_at,
  })
}

/// The claims a JWT carries in its payload segment, read and never verified.
fn read_jwt_claims(token: &str) -> Option<Value> {
  let payload = decode_base64url(token.split('.').nth(1)?)?;
  serde_json::from_slice(&payload).ok()
}

/// The bytes of a base64url segment, decoded here rather than through a dependency: one claim
/// reader is not worth a crate, and a reader this lenient - padding anywhere, trailing bits kept or
/// dropped as they fall - is what a token read and never verified needs.
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

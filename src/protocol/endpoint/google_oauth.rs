//! Antigravity's Google OAuth grants. Each exchange makes one transport attempt; the
//! application owns login state, project discovery, persistence and renewal timing.
use serde_json::Value;

use super::{AuthScheme, Credentials, Draft, Endpoint, Tokens};
use crate::protocol::{attempt::Transport, error::Error, http_error};
use crate::utils::time::unix_seconds;

// The public desktop client's registration, shared by the Antigravity login flow.
const CLIENT_ID: &str = "1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com";
const CLIENT_SECRET: &str = "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";
const SCOPES: &str = "https://www.googleapis.com/auth/cloud-platform https://www.googleapis.com/auth/userinfo.email https://www.googleapis.com/auth/userinfo.profile https://www.googleapis.com/auth/cclog https://www.googleapis.com/auth/experimentsandconfigs";

fn token_host() -> String {
  #[cfg(debug_assertions)]
  if let Ok(host) = std::env::var("WISH_TEST_GOOGLE_ISSUER") {
    return host.trim_end_matches('/').to_owned();
  }
  "https://oauth2.googleapis.com".to_owned()
}

pub(crate) fn authorization_url(
  redirect: &str,
  state: &str,
  verifier: &str,
) -> Result<String, Error> {
  use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
  use sha2::{Digest, Sha256};
  let mut url = reqwest::Url::parse("https://accounts.google.com/o/oauth2/v2/auth")
    .map_err(|e| Error::Build(e.to_string()))?;
  #[cfg(debug_assertions)]
  if let Ok(host) = std::env::var("WISH_TEST_GOOGLE_ISSUER") {
    url = reqwest::Url::parse(&format!("{}/authorize", host.trim_end_matches('/')))
      .map_err(|e| Error::Build(e.to_string()))?;
  }
  url
    .query_pairs_mut()
    .append_pair("client_id", CLIENT_ID)
    .append_pair("redirect_uri", redirect)
    .append_pair("response_type", "code")
    .append_pair("scope", SCOPES)
    .append_pair("access_type", "offline")
    .append_pair("prompt", "consent")
    .append_pair("state", state)
    .append_pair("code_challenge", &URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())))
    .append_pair("code_challenge_method", "S256");
  Ok(url.to_string())
}

pub(crate) async fn exchange_code<T: Transport>(
  transport: &T,
  code: &str,
  redirect: &str,
  verifier: &str,
) -> Result<Tokens, Error> {
  exchange(
    transport,
    &[
      ("grant_type", "authorization_code"),
      ("code", code),
      ("redirect_uri", redirect),
      ("code_verifier", verifier),
    ],
  )
  .await
}

pub async fn refresh<T: Transport>(
  transport: &T,
  credentials: &Credentials,
) -> Result<Tokens, Error> {
  let token = credentials
    .refresh_token
    .as_deref()
    .filter(|v| !v.is_empty())
    .ok_or_else(|| Error::Build("Google token renewal needs a refresh token".to_owned()))?;
  exchange(transport, &[("grant_type", "refresh_token"), ("refresh_token", token)]).await
}

async fn exchange<T: Transport>(transport: &T, fields: &[(&str, &str)]) -> Result<Tokens, Error> {
  let mut form =
    reqwest::Url::parse("https://form.invalid/").map_err(|e| Error::Build(e.to_string()))?;
  form
    .query_pairs_mut()
    .append_pair("client_id", CLIENT_ID)
    .append_pair("client_secret", CLIENT_SECRET)
    .extend_pairs(fields.iter().copied());
  let endpoint = Endpoint::new(&token_host(), "/token", AuthScheme::None)?;
  let call = endpoint.build_call(
    Draft::post_form(form.query().unwrap_or_default()),
    &Credentials::default(),
    0,
  )?;
  let reply = transport.execute(&call).await?;
  // OAuth errors may echo grant material. Keep them out of saved login errors and logs.
  if !reply.is_success() {
    return Err(Error::Build(format!("Google token exchange returned HTTP {}", reply.status)));
  }
  let body = http_error::decode_json_body("Google token exchange", &reply.body)?;
  let access_token = body
    .get("access_token")
    .and_then(Value::as_str)
    .filter(|v| !v.is_empty())
    .ok_or_else(|| Error::Malformed("Google token response has no access_token".to_owned()))?;
  let expires_in =
    body.get("expires_in").and_then(Value::as_u64).filter(|v| *v > 0).ok_or_else(|| {
      Error::Malformed("Google token response has no positive expires_in".to_owned())
    })?;
  Ok(Tokens {
    access_token: access_token.to_owned(),
    refresh_token: body
      .get("refresh_token")
      .and_then(Value::as_str)
      .filter(|v| !v.is_empty())
      .map(str::to_owned),
    expires_at: Some(unix_seconds().saturating_add(expires_in)),
    ..Default::default()
  })
}

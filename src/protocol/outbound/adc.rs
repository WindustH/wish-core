//! Google ADC: the one credential file every Google SDK reads the same way, and the exchanges
//! that turn it into the access token a Vertex call is proven with.
//!
//! Conversions:
//! - The file is the caller's to find and read - this crate touches no filesystem and no
//!   environment - and [`parse`] turns its text into the [`Adc`] struct, or the caller assembles
//!   the struct itself. What `gcloud auth application-default login` wrote is enough, and gcloud
//!   itself is not needed again: the renewals happen here.
//! - An `authorized_user` file (a login) is renewed by the same refresh grant a subscription is.
//!   The exchange differs in its wire details, not its shape: the endpoint is the file's own
//!   `token_uri`, the body is form-encoded, and the client secret joins it.
//! - A `service_account` file (a key) is not renewed at all: each token is a fresh assertion,
//!   signed RS256 over who the key is and what it may do, exchanged once and good for an hour.
//! - The response's `expires_in` seconds become the material's `expires_at`, against the `now`
//!   the caller reads: Google's access tokens are opaque, so nothing is parsed out of the token
//!   itself, and an early margin is the caller's to ask for.
//!
//! Constraints:
//! - Only the two personal forms are understood. `external_account` (workload identity
//!   federation) is named and refused: it asks an outside identity provider that only its owner
//!   can answer.
//! - A response without an access token is not a token response, whatever else it carries.
//!
//! Trade-offs:
//! - One attempt, no retry, exactly as a subscription's exchange: whether a refused exchange is
//!   worth another is the caller's decision, and this crate holds no account state.
//! - The scope a service account asks for is the caller's sentence (Vertex wants
//!   `https://www.googleapis.com/auth/cloud-platform`); an authorized user's refresh asks for no
//!   scope, because the grant was already made at login.
//! - `iat` and `exp` are stamped from the caller's `now`, so a clock that disagrees with
//!   Google's by minutes will be told so by the endpoint.
//!
//! The RSA and base64 routines are the ones the TLS stack already compiles (`ring`, `base64`
//! under reqwest's rustls), so signing a key costs the tree nothing it was not already paying.

use serde_json::{Value, json};

use crate::Error;
use crate::protocol::http_error;
use crate::protocol::outbound::{AuthProtocol, Credentials, Draft, Outbound};
use crate::protocol::wire::{Method, Reply, Transport};

/// How long a service account's assertion claims to live, in seconds: the hour Google's own
/// libraries ask for.
const ASSERTION_LIFETIME: u64 = 3600;

/// One ADC file, in the two forms that can be answered here.
///
/// The third form a file may carry, `external_account`, is refused at the parse rather than
/// modeled: it delegates to an identity provider this crate has no way to ask.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Adc {
  /// What `gcloud auth application-default login` writes: a user's grant, renewed by refresh.
  AuthorizedUser {
    /// The client the login was made as (gcloud's own).
    client_id: String,
    /// The client's secret, which travels with a desktop client by Google's design.
    client_secret: String,
    /// The token the next exchange takes; Google does not rotate it.
    refresh_token: String,
    /// Where the exchange is asked (`https://oauth2.googleapis.com/token`).
    token_uri: String,
  },
  /// A service account key: not renewed, but signed into a fresh assertion per token.
  ServiceAccount {
    /// The identity the key speaks for, and the assertion's `iss`.
    client_email: String,
    /// The PKCS#8 PEM the file carried.
    private_key: String,
    /// Where the assertion is exchanged (`https://oauth2.googleapis.com/token`).
    token_uri: String,
  },
}

/// What an exchange hands back: the access token the call is proven with, and when it stops
/// being accepted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdcTokens {
  /// The token, ready to become a credential's `api_key`.
  pub access_token: String,
  /// The unix time the endpoint said the token lives to, read off `expires_in`.
  pub expires_at: u64,
}

/// Parses one ADC file's text - read by the caller, its own way - into the [`Adc`] struct the
/// exchanges take.
///
/// # Errors
///
/// [`Error::Build`] when the text cannot be parsed, and for a form this crate does not answer
/// (`external_account` and friends).
pub fn parse(text: &str) -> Result<Adc, Error> {
  let value = serde_json::from_str::<Value>(text)
    .map_err(|error| Error::Build(format!("the ADC file is not JSON: {error}")))?;
  let kind = read_string_field(&value, "type")?;
  match kind.as_str() {
    "authorized_user" => Ok(Adc::AuthorizedUser {
      client_id: read_string_field(&value, "client_id")?,
      client_secret: read_string_field(&value, "client_secret")?,
      refresh_token: read_string_field(&value, "refresh_token")?,
      token_uri: read_string_field(&value, "token_uri")?,
    }),
    "service_account" => Ok(Adc::ServiceAccount {
      client_email: read_string_field(&value, "client_email")?,
      private_key: read_string_field(&value, "private_key")?,
      token_uri: read_string_field(&value, "token_uri")?,
    }),
    other => Err(Error::Build(format!(
      "the ADC file's `{other}` form is not answered here: only `authorized_user` (a login) and \
       `service_account` (a key) are"
    ))),
  }
}

fn read_string_field(value: &Value, name: &str) -> Result<String, Error> {
  value
    .get(name)
    .and_then(Value::as_str)
    .filter(|field| !field.is_empty())
    .map(str::to_owned)
    .ok_or_else(|| Error::Build(format!("the ADC file carries no `{name}`")))
}

/// Exchanges one ADC file for its access token.
///
/// One entry point for both forms, because the caller holds the file's form, not a preference
/// between exchanges: a login is refreshed, a key is signed into a fresh assertion. The `scope`
/// sentence is the service account's to make; a login's grant was made at login and asks for
/// none. Dispatched through the same join every other request takes, so the exchange holds no
/// HTTP of its own.
///
/// # Errors
///
/// [`Error::Build`] when the form's request cannot be built, [`Error::Transport`] when the
/// network fails before a reply exists, [`Error::Upstream`] for a refused exchange, and
/// [`Error::Malformed`] when the reply is not a token response.
pub async fn fetch_token<T: Transport>(
  transport: &T,
  adc: &Adc,
  scope: &str,
  now: u64,
) -> Result<AdcTokens, Error> {
  let (uri, body) = match adc {
    Adc::AuthorizedUser { client_id, client_secret, refresh_token, token_uri } => (
      token_uri.as_str(),
      encode_form(&[
        ("client_id", client_id.as_str()),
        ("client_secret", client_secret.as_str()),
        ("refresh_token", refresh_token.as_str()),
        ("grant_type", "refresh_token"),
      ]),
    ),
    Adc::ServiceAccount { client_email, private_key, token_uri } => (
      token_uri.as_str(),
      encode_form(&[
        ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
        ("assertion", &build_assertion(client_email, private_key, token_uri, scope, now)?),
      ]),
    ),
  };
  let (base, path) = split_uri(uri)?;
  let target = Outbound::new(&base, &path, AuthProtocol::None)?;
  let draft = Draft {
    method: Method::Post,
    path: None,
    query: Vec::new(),
    headers: vec![("content-type".to_owned(), "application/x-www-form-urlencoded".to_owned())],
    body,
  };
  let call = target.dispatch(draft, &Credentials::default(), 0)?;
  let reply = transport.execute(&call).await?;
  decode_tokens(&reply, now)
}

fn decode_tokens(reply: &Reply, now: u64) -> Result<AdcTokens, Error> {
  if !reply.is_success() {
    let error = http_error::decode_provider_envelope(reply.status, &reply.body);
    return Err(error.with_retry_after(reply.get_retry_after_ms()));
  }
  let body = http_error::decode_json_body("google_adc", &reply.body)?;
  let access_token = body
    .get("access_token")
    .and_then(Value::as_str)
    .filter(|token| !token.is_empty())
    .ok_or_else(|| Error::Malformed("the token response carries no `access_token`".to_owned()))?;
  let expires_in = body
    .get("expires_in")
    .and_then(Value::as_u64)
    .ok_or_else(|| Error::Malformed("the token response carries no `expires_in`".to_owned()))?;
  Ok(AdcTokens {
    access_token: access_token.to_owned(),
    expires_at: now.saturating_add(expires_in),
  })
}

fn split_uri(uri: &str) -> Result<(String, String), Error> {
  let (scheme, rest) = uri
    .split_once("://")
    .ok_or_else(|| Error::Build(format!("the ADC file's `token_uri` `{uri}` names no scheme")))?;
  if !matches!(scheme, "http" | "https") {
    return Err(Error::Build(format!("the ADC file's `token_uri` `{uri}` is not an HTTP(S) URI")));
  }
  let (host, path) = match rest.split_once('/') {
    Some((host, path)) => (host, path),
    None => (rest, ""),
  };
  if host.is_empty() {
    return Err(Error::Build(format!("the ADC file's `token_uri` `{uri}` names no host")));
  }
  Ok((format!("{scheme}://{host}"), format!("/{path}")))
}

fn encode_form(fields: &[(&str, &str)]) -> Vec<u8> {
  let mut body = Vec::new();
  for (index, (name, value)) in fields.iter().enumerate() {
    if index > 0 {
      body.push(b'&');
    }
    append_form_encoded(&mut body, name.as_bytes());
    body.push(b'=');
    append_form_encoded(&mut body, value.as_bytes());
  }
  body
}

fn append_form_encoded(out: &mut Vec<u8>, bytes: &[u8]) {
  for &byte in bytes {
    match byte {
      b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(byte),
      b' ' => out.push(b'+'),
      _ => {
        out.extend_from_slice(format!("%{byte:02X}").as_bytes());
      }
    }
  }
}

/// Signs one service account's assertion: the RS256 JWT Google's jwt-bearer grant takes.
fn build_assertion(
  client_email: &str,
  private_key: &str,
  token_uri: &str,
  scope: &str,
  now: u64,
) -> Result<String, Error> {
  let key = ring::rsa::KeyPair::from_pkcs8(&decode_pem_to_der(private_key)?)
    .map_err(|_| Error::Build("the ADC private key is not an RSA PKCS#8 key".to_owned()))?;
  let header = json!({"alg": "RS256", "typ": "JWT"});
  let claims = json!({
    "iss": client_email,
    "scope": scope,
    "aud": token_uri,
    "iat": now,
    "exp": now + ASSERTION_LIFETIME,
  });
  let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
  use base64::Engine as _;
  let header =
    engine.encode(serde_json::to_vec(&header).map_err(|error| {
      Error::Build(format!("the assertion header is not serializable: {error}"))
    })?);
  let claims = engine.encode(serde_json::to_vec(&claims).map_err(|error| {
    Error::Build(format!("the assertion claims are not serializable: {error}"))
  })?);
  let signing_input = format!("{header}.{claims}");
  let mut signature = vec![0; key.public().modulus_len()];
  key
    .sign(
      &ring::signature::RSA_PKCS1_SHA256,
      &ring::rand::SystemRandom::new(),
      signing_input.as_bytes(),
      &mut signature,
    )
    .map_err(|_| Error::Build("the assertion could not be signed".to_owned()))?;
  Ok(format!("{signing_input}.{}", engine.encode(signature)))
}

/// Decodes a PKCS#8 PEM body (the `-----BEGIN PRIVATE KEY-----` form a service account file
/// carries) into the DER bytes `ring` parses.
fn decode_pem_to_der(pem: &str) -> Result<Vec<u8>, Error> {
  let mut body = String::new();
  let mut inside = false;
  for line in pem.lines() {
    let line = line.trim();
    if line.starts_with("-----BEGIN") {
      inside = true;
    } else if line.starts_with("-----END") {
      break;
    } else if inside {
      body.push_str(line);
    }
  }
  if body.is_empty() {
    return Err(Error::Build("the ADC private key carries no PEM body".to_owned()));
  }
  use base64::Engine as _;
  base64::engine::general_purpose::STANDARD
    .decode(body.as_bytes())
    .map_err(|_| Error::Build("the ADC private key's body is not base64".to_owned()))
}

//! The GitHub Copilot subscription's token endpoints: GitHub's device flow, which hands out a
//! GitHub token, and the exchange that turns that token into a short-lived Copilot session.
//!
//! Conversions:
//! - A sign-in is GitHub's device flow under the OAuth app Copilot's editors sign in with: a
//!   form-encoded `POST /login/device/code` answers a code for the person to type at
//!   `verification_uri`, and `POST /login/oauth/access_token` is polled with the device code until
//!   GitHub answers a token, or says why it will not.
//! - The GitHub token is the account's long-lived credential, kept as the refresh token. Copilot's
//!   API takes a session token instead, which `GET /copilot_internal/v2/token` hands out for the
//!   GitHub token sent as `Authorization: token <token>`; its `token` becomes the access token and
//!   its `expires_at`, in Unix seconds, the expiry.
//!
//! Constraints:
//! - A credential without a GitHub token is a configuration mistake, not a request: the exchange
//!   is not attempted.
//! - GitHub answers a pending, slowed or refused device poll with `200` and an `error` field, so
//!   the poll's outcome is read from the body, never from the status alone.
//!
//! Trade-offs:
//! - The calls are asked as Copilot's own chat extension asks them, with its editor headers:
//!   Copilot keys its client policy on them, and this crate has no client of its own that GitHub
//!   would recognise.
//! - The exchange does not rotate the GitHub token, so the renewed credentials keep the one they
//!   had; the session's `endpoints.api` is not followed, because the configured base URL is the
//!   address the provider's calls go to.

use serde_json::Value;

use crate::Error;
use crate::protocol::attempt::Transport;
use crate::protocol::endpoint::{
  AuthScheme, Credentials, Draft, Endpoint, Source, SourceHeader, Tokens,
};
use crate::protocol::http_error;
use crate::protocol::json_read::read_string_member;

/// Where GitHub's device flow is asked.
const GITHUB: &str = "https://github.com";
/// Where GitHub's API, the Copilot token exchange and the account's entitlement among it, answers.
const GITHUB_API: &str = "https://api.github.com";
/// The OAuth app Copilot's editors sign in with.
const CLIENT_ID: &str = "Iv1.b507a08c87ecfe98";
/// The page a person types the device code at, when GitHub names none.
const DEVICE_PAGE: &str = "https://github.com/login/device";

/// What GitHub's API is told the caller is: Copilot's chat extension in its editor.
pub(crate) const EDITOR_HEADERS: &[SourceHeader] = &[
  SourceHeader::Literal("editor-version", "vscode/1.140.0"),
  SourceHeader::Literal("editor-plugin-version", "copilot-chat/0.68.0"),
  SourceHeader::Literal("copilot-integration-id", "vscode-chat"),
  SourceHeader::Literal("user-agent", "GitHubCopilotChat/0.68.0"),
  SourceHeader::Literal("accept", "application/json"),
];

/// What Copilot's own API is told: the editor, and the version of the chat API it speaks.
pub(crate) const API_HEADERS: &[SourceHeader] = &[
  SourceHeader::Literal("editor-version", "vscode/1.140.0"),
  SourceHeader::Literal("editor-plugin-version", "copilot-chat/0.68.0"),
  SourceHeader::Literal("copilot-integration-id", "vscode-chat"),
  SourceHeader::Literal("user-agent", "GitHubCopilotChat/0.68.0"),
  SourceHeader::Literal("x-github-api-version", "2026-01-09"),
];

/// The exchange of a GitHub token for a Copilot session.
const TOKEN_SOURCE: Source = Source {
  protocol: "github_copilot_token",
  base_url: None,
  path: Some("/copilot_internal/v2/token"),
  auth: AuthScheme::GitHubToken,
  headers: EDITOR_HEADERS,
};

/// GitHub's host and its API's, without a trailing `/`.
///
/// A debug build asks the one `WISH_TEST_GITHUB` names for both instead, so the tests can stand
/// one up.
pub(crate) fn get_hosts() -> (String, String) {
  #[cfg(debug_assertions)]
  if let Ok(value) = std::env::var("WISH_TEST_GITHUB") {
    let host = value.trim_end_matches('/').to_owned();
    return (host.clone(), host);
  }
  (GITHUB.to_owned(), GITHUB_API.to_owned())
}

/// A device sign-in GitHub has started: the code a person types, and where.
#[derive(Clone, Debug)]
pub struct DeviceCode {
  /// What the poll is asked with; never shown.
  pub device_code: String,
  /// What the person types at `verification_uri`.
  pub user_code: String,
  /// The page the code is typed at.
  pub verification_uri: String,
  /// Seconds until the code stops being accepted.
  pub expires_in: u64,
  /// Seconds GitHub asks a poll to wait before the next.
  pub interval: u64,
}

/// What one poll of a device sign-in found.
#[derive(Clone, Debug)]
pub enum DevicePoll {
  /// The person has not typed the code yet.
  Pending,
  /// GitHub asks the polls to come further apart.
  SlowDown,
  /// The person approved: the GitHub token.
  Approved(String),
  /// GitHub will not hand a token out for this code, in its own words.
  Refused(String),
}

/// Starts a device sign-in.
///
/// # Errors
///
/// [`Error::Transport`] when the network fails, [`Error::Upstream`] for a refused request, and
/// [`Error::Malformed`] when the reply carries no code.
pub async fn request_device_code<T: Transport>(transport: &T) -> Result<DeviceCode, Error> {
  let body =
    post_form(transport, "/login/device/code", &[("client_id", CLIENT_ID), ("scope", "read:user")])
      .await?;
  let (Some(device_code), Some(user_code)) =
    (read_string_member(Some(&body), "device_code"), read_string_member(Some(&body), "user_code"))
  else {
    return Err(Error::Malformed("GitHub's device sign-in answered no code".to_owned()));
  };
  Ok(DeviceCode {
    device_code,
    user_code,
    verification_uri: read_string_member(Some(&body), "verification_uri")
      .unwrap_or_else(|| DEVICE_PAGE.to_owned()),
    expires_in: body.get("expires_in").and_then(Value::as_u64).unwrap_or(900),
    interval: body.get("interval").and_then(Value::as_u64).unwrap_or(5),
  })
}

/// Asks once whether a device sign-in was approved.
///
/// # Errors
///
/// [`Error::Transport`] when the network fails and [`Error::Upstream`] for a refused request.
pub async fn poll_device_code<T: Transport>(
  transport: &T,
  device_code: &str,
) -> Result<DevicePoll, Error> {
  let body = post_form(
    transport,
    "/login/oauth/access_token",
    &[
      ("client_id", CLIENT_ID),
      ("device_code", device_code),
      ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
    ],
  )
  .await?;
  if let Some(token) = read_string_member(Some(&body), "access_token") {
    return Ok(DevicePoll::Approved(token));
  }
  Ok(match read_string_member(Some(&body), "error").as_deref() {
    Some("authorization_pending") => DevicePoll::Pending,
    Some("slow_down") => DevicePoll::SlowDown,
    error => DevicePoll::Refused(
      read_string_member(Some(&body), "error_description")
        .or_else(|| error.map(str::to_owned))
        .unwrap_or_else(|| "GitHub answered no token".to_owned()),
    ),
  })
}

/// Exchanges the account's GitHub token for a Copilot session.
///
/// # Errors
///
/// [`Error::Build`] when the credentials carry no GitHub token, [`Error::Transport`] when the
/// network fails, [`Error::Upstream`] for a refused exchange - a token GitHub no longer accepts,
/// or an account without Copilot - and [`Error::Malformed`] when the reply carries no session.
pub async fn exchange<T: Transport>(
  transport: &T,
  credentials: &Credentials,
) -> Result<Tokens, Error> {
  let (_, api) = get_hosts();
  let call = TOKEN_SOURCE.to_endpoint(Some(&api), None)?.build_call(
    Draft::get(Vec::new()),
    credentials,
    0,
  )?;
  let body =
    http_error::read_provider_json(transport.execute(&call).await?, TOKEN_SOURCE.protocol)?;
  let access_token = read_string_member(Some(&body), "token")
    .ok_or_else(|| Error::Malformed("the Copilot token response carries no `token`".to_owned()))?;
  Ok(Tokens {
    access_token,
    refresh_token: None,
    id_token: None,
    account_id: None,
    expires_at: body.get("expires_at").and_then(Value::as_u64),
  })
}

/// Posts one form to GitHub's sign-in host and reads the JSON it answers.
async fn post_form<T: Transport>(
  transport: &T,
  path: &str,
  pairs: &[(&str, &str)],
) -> Result<Value, Error> {
  // The URL's own query serializer is the form encoding the endpoint reads.
  let mut form = reqwest::Url::parse("https://form.invalid/")
    .map_err(|error| Error::Build(format!("the sign-in form cannot be built: {error}")))?;
  form.query_pairs_mut().extend_pairs(pairs);
  let mut draft = Draft::post_form(form.query().unwrap_or_default());
  // GitHub answers a form-encoded body unless asked for JSON.
  draft.headers.push(("accept".to_owned(), "application/json".to_owned()));
  let (github, _) = get_hosts();
  let call = Endpoint::new(&github, path, AuthScheme::None)?.build_call(
    draft,
    &Credentials::default(),
    0,
  )?;
  http_error::read_provider_json(transport.execute(&call).await?, "github_device_flow")
}

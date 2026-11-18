//! Antigravity account enrollment and optional Cloud Code project discovery. These are single attempts, through the same transport
//! as model calls; the login application decides whether to onboard and when to poll.
use super::{AuthScheme, Credentials, Draft, Endpoint};
use crate::protocol::{attempt::Transport, error::Error, http_error};
use crate::utils::time::unix_seconds;
use serde_json::{Value, json};

// Version defaults follow antigravity-claude-proxy; model-service headers can override them.
pub const USER_AGENT: &str = "antigravity/hub/2.25.0";
pub const API_HEADERS: &[super::SourceHeader] = &[
  super::SourceHeader::Literal("user-agent", USER_AGENT),
  super::SourceHeader::Literal("x-client-name", "antigravity"),
  super::SourceHeader::Literal("x-client-version", "1.110.0"),
];

fn host() -> String {
  #[cfg(debug_assertions)]
  if let Ok(host) = std::env::var("WISH_TEST_GOOGLE_CLOUDCODE") {
    return host.trim_end_matches('/').to_owned();
  }
  "https://cloudcode-pa.googleapis.com".to_owned()
}

pub fn project(body: &Value) -> Option<String> {
  let value = body.get("cloudaicompanionProject")?;
  value
    .as_str()
    .or_else(|| value.get("id")?.as_str())
    .map(str::trim)
    .filter(|v| !v.is_empty())
    .map(str::to_owned)
}

/// Consumer accounts need no project; omit it instead of sending null or inventing one.
pub fn project_body(project: Option<&str>) -> Value {
  let mut body = json!({});
  if let Some(project) = project.map(str::trim).filter(|p| !p.is_empty()) {
    body["project"] = json!(project);
  }
  body
}

pub async fn load<T: Transport>(transport: &T, credentials: &Credentials) -> Result<Value, Error> {
  let mut body = json!({"metadata": metadata()});
  if let Some(project) = credentials.project.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
    body["cloudaicompanionProject"] = json!(project);
    body["metadata"]["duetProject"] = json!(project);
  }
  send(transport, credentials, "/v1internal:loadCodeAssist", Draft::post_json(&body)?).await
}

pub async fn onboard<T: Transport>(
  transport: &T,
  credentials: &Credentials,
  tier: &str,
) -> Result<Value, Error> {
  let mut body = json!({"tierId": tier, "metadata": metadata()});
  // Consumer enrollment does not use a caller-supplied project.
  if !matches!(tier, "free-tier" | "FREE")
    && let Some(project) = credentials.project.as_deref().map(str::trim).filter(|p| !p.is_empty())
  {
    body["cloudaicompanionProject"] = json!(project);
    body["metadata"]["duetProject"] = json!(project);
  }
  send(transport, credentials, "/v1internal:onboardUser", Draft::post_json(&body)?).await
}

/// Poll only relative operation names on the Cloud Code host, never a returned URL.
pub async fn operation<T: Transport>(
  transport: &T,
  credentials: &Credentials,
  name: &str,
) -> Result<Value, Error> {
  if name.is_empty()
    || !name.split('/').all(|part| {
      !part.is_empty()
        && part != "."
        && part != ".."
        && part.bytes().all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
    })
  {
    return Err(Error::Malformed(
      "Google returned an invalid onboarding operation name".to_owned(),
    ));
  }
  send(transport, credentials, &format!("/v1internal/{name}"), Draft::get(vec![])).await
}

fn metadata() -> Value {
  json!({"ideType": 9, "platform": 0, "pluginType": 2})
}

async fn send<T: Transport>(
  transport: &T,
  credentials: &Credentials,
  path: &str,
  draft: Draft,
) -> Result<Value, Error> {
  let mut endpoint = Endpoint::new(&host(), path, AuthScheme::Bearer(None))?;
  for header in API_HEADERS {
    if let super::SourceHeader::Literal(name, value) = header {
      endpoint = endpoint.with_header(name, value);
    }
  }
  let call = endpoint.build_call(draft, credentials, unix_seconds())?;
  let body = http_error::read_provider_json(transport.execute(&call).await?, "google_antigravity")?;
  // A completed long-running operation can carry an error even with HTTP 200.
  if body.get("error").is_some_and(|error| !error.is_null()) {
    return Err(http_error::decode_in_band(
      body.get("error"),
      &["status"],
      "Google project setup failed",
    ));
  }
  Ok(body)
}

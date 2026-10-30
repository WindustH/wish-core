//! Cloud Code project discovery. These are single attempts, through the same transport
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
  value.as_str().or_else(|| value.get("id")?.as_str()).filter(|v| !v.is_empty()).map(str::to_owned)
}

pub async fn load<T: Transport>(transport: &T, credentials: &Credentials) -> Result<Value, Error> {
  send(transport, credentials, "/v1internal:loadCodeAssist", json!({"metadata": metadata()})).await
}

pub async fn onboard<T: Transport>(
  transport: &T,
  credentials: &Credentials,
  tier: &str,
) -> Result<Value, Error> {
  send(
    transport,
    credentials,
    "/v1internal:onboardUser",
    json!({"tierId": tier, "metadata": metadata()}),
  )
  .await
}

fn metadata() -> Value {
  json!({"ideType": 9, "platform": 0, "pluginType": 2})
}

async fn send<T: Transport>(
  transport: &T,
  credentials: &Credentials,
  path: &str,
  body: Value,
) -> Result<Value, Error> {
  let mut endpoint = Endpoint::new(&host(), path, AuthScheme::Bearer(None))?;
  for header in API_HEADERS {
    if let super::SourceHeader::Literal(name, value) = header {
      endpoint = endpoint.with_header(name, value);
    }
  }
  let call = endpoint.build_call(Draft::post_json(&body)?, credentials, unix_seconds())?;
  http_error::read_provider_json(transport.execute(&call).await?, "google_antigravity")
}

//! The half of the bridge that `wish mcp` and `wish skill` run: who they speak for, read from the
//! environment the session's shell gives them, and asking the server for it.
use serde_json::Value;

/// A command's exit status, and what it says on standard error.
pub type Failure = (i32, String);

pub struct BridgeClient {
  pub client: reqwest::Client,
  /// The session's own part of the API: `<url>/sessions/<session>`.
  base: String,
  token: String,
}

impl BridgeClient {
  /// The bridge the shell's environment names; `command` is the one being run, for the message
  /// when it runs anywhere else.
  pub fn from_environment(command: &str) -> Result<Self, Failure> {
    let read = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
    let (Some(url), Some(session), Some(token)) =
      (read("WISH_URL"), read("WISH_SESSION"), read("WISH_SESSION_TOKEN"))
    else {
      return Err((2, format!("`wish {command}` runs in the shell of a wish session")));
    };
    // The bridge is on this machine: a proxy from the environment must not stand in between.
    let client = reqwest::Client::builder()
      .no_proxy()
      .build()
      .map_err(|error| (2, format!("could not set up a client: {error}")))?;
    Ok(Self { client, base: format!("{url}/sessions/{session}"), token })
  }

  /// The address of `path` under the session's part of the API, with a query.
  pub fn url(&self, path: &str, query: &[(&str, &str)]) -> Result<reqwest::Url, Failure> {
    let mut url = reqwest::Url::parse(&format!("{}/{path}", self.base))
      .map_err(|error| (2, format!("WISH_URL is not a valid address: {error}")))?;
    if !query.is_empty() {
      url.query_pairs_mut().extend_pairs(query);
    }
    Ok(url)
  }

  /// Sends a request as the session, and reads its JSON answer. A refusal comes back as status 2,
  /// with the server's message; one whose outcome is unknown, as 3.
  pub async fn send(&self, request: reqwest::RequestBuilder) -> Result<Value, Failure> {
    let response = request
      .bearer_auth(&self.token)
      .send()
      .await
      .map_err(|error| (2, format!("could not reach wish: {error}")))?;
    let status = response.status();
    let body =
      response.bytes().await.map_err(|error| (3, format!("the answer broke off: {error}")))?;
    let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    if status.is_success() {
      return Ok(body);
    }
    let message =
      body["error"]["message"].as_str().map_or_else(|| format!("HTTP {status}"), str::to_owned);
    let code = if body["error"]["details"]["kind"] == "unknown" { 3 } else { 2 };
    Err((code, message))
  }
}

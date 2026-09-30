//! Wish's HTTP client under rmcp's Streamable HTTP transport.
//!
//! rmcp keeps the session: the handshake, `Mcp-Session-Id`, recovery after a 404. What it asks of a
//! client is one request at a time, read into a JSON-RPC message, an SSE stream or an
//! acknowledgement. This one sends them with wish's `reqwest` under wish's proxy policy, and takes
//! any success that carries no answer as an acknowledgement when none is due: some servers answer a
//! notification with a bare 200 that has neither a length nor a content type.

use futures_util::{StreamExt, stream::BoxStream};
use reqwest::{
  StatusCode,
  header::{ACCEPT, CONTENT_TYPE, HeaderName, HeaderValue},
};
use rmcp::{
  model::{ClientJsonRpcMessage, ClientRequest, ErrorData, ServerJsonRpcMessage},
  transport::streamable_http_client::{
    SseError, StreamableHttpClient, StreamableHttpError, StreamableHttpPostResponse,
  },
};
use sse_stream::{Sse, SseStream};
use std::{borrow::Cow, collections::HashMap, sync::Arc, time::Duration};

use crate::transport::{Proxy, apply_proxy};

const SESSION_ID: &str = "mcp-session-id";
const LAST_EVENT_ID: &str = "last-event-id";
const EVENT_STREAM: &str = "text/event-stream";
const JSON: &str = "application/json";

type Failure = StreamableHttpError<reqwest::Error>;

#[derive(Clone)]
pub struct HttpClient {
  client: reqwest::Client,
}

impl HttpClient {
  pub fn new(proxy: Proxy) -> Result<Self, crate::Error> {
    let builder = reqwest::Client::builder().connect_timeout(Duration::from_secs(10));
    let client = apply_proxy(builder, proxy)?
      .build()
      .map_err(|error| crate::Error::Build(format!("client build failed: {error}")))?;
    Ok(Self { client })
  }

  /// `request` with what every request to the server carries: the accepted types, the auth and
  /// custom headers rmcp hands over, and the session once there is one.
  fn with_common_headers(
    &self,
    request: reqwest::RequestBuilder,
    session_id: Option<&str>,
    auth_header: Option<String>,
    custom_headers: HashMap<HeaderName, HeaderValue>,
  ) -> reqwest::RequestBuilder {
    let mut request = request.header(ACCEPT, format!("{EVENT_STREAM}, {JSON}"));
    if let Some(token) = auth_header {
      request = request.bearer_auth(token);
    }
    for (name, value) in custom_headers {
      request = request.header(name, value);
    }
    if let Some(id) = session_id {
      request = request.header(SESSION_ID, id);
    }
    request
  }
}

fn read_header(
  response: &reqwest::Response,
  name: impl reqwest::header::AsHeaderName,
) -> Option<String> {
  response.headers().get(name).and_then(|value| value.to_str().ok()).map(str::to_owned)
}

fn read_events(response: reqwest::Response) -> BoxStream<'static, Result<Sse, SseError>> {
  SseStream::from_bytes_stream(response.bytes_stream()).boxed()
}

/// A body kept in an error: short, and readable whatever it held.
fn preview(body: &[u8]) -> String {
  let text = String::from_utf8_lossy(body);
  if text.trim().is_empty() {
    return "<empty>".to_owned();
  }
  text.chars().take(256).collect()
}

impl StreamableHttpClient for HttpClient {
  type Error = reqwest::Error;

  async fn post_message(
    &self,
    uri: Arc<str>,
    message: ClientJsonRpcMessage,
    session_id: Option<Arc<str>>,
    auth_header: Option<String>,
    custom_headers: HashMap<HeaderName, HeaderValue>,
  ) -> Result<StreamableHttpPostResponse, Failure> {
    let attached = session_id.is_some();
    let body = serde_json::to_vec(&message)?;
    let response = self
      .with_common_headers(
        self.client.post(uri.as_ref()),
        session_id.as_deref(),
        auth_header,
        custom_headers,
      )
      .header(CONTENT_TYPE, JSON)
      .body(body)
      .send()
      .await
      .map_err(StreamableHttpError::Client)?;
    let status = response.status();
    if matches!(status, StatusCode::ACCEPTED | StatusCode::NO_CONTENT) {
      return Ok(StreamableHttpPostResponse::Accepted);
    }
    if status == StatusCode::NOT_FOUND && attached {
      return Err(StreamableHttpError::SessionExpired);
    }
    let content_type = read_header(&response, CONTENT_TYPE).unwrap_or_default();
    let session = read_header(&response, SESSION_ID);
    let expects_answer = matches!(message, ClientJsonRpcMessage::Request(_));
    if content_type.starts_with(EVENT_STREAM) && status.is_success() {
      return Ok(StreamableHttpPostResponse::Sse(read_events(response), session));
    }
    let body = response.bytes().await.map_err(StreamableHttpError::Client)?;
    if !status.is_success() {
      // A server that predates `server/discover` rejects it; the rejection, answered with the
      // request's own id, is what sends the handshake back to `initialize`.
      if let ClientJsonRpcMessage::Request(request) = &message
        && matches!(request.request, ClientRequest::DiscoverRequest(_))
        && !attached
        && status.is_client_error()
        && !matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN)
      {
        let error = match serde_json::from_slice::<ServerJsonRpcMessage>(&body) {
          Ok(ServerJsonRpcMessage::Error(error)) => error.error,
          _ => ErrorData::invalid_request(
            format!("server/discover rejected with HTTP {status}: {}", preview(&body)),
            None,
          ),
        };
        return Ok(StreamableHttpPostResponse::Json(
          ServerJsonRpcMessage::error(error, Some(request.id.clone())),
          None,
        ));
      }
      // A JSON-RPC error in a failed reply still answers the request.
      if let Ok(error @ ServerJsonRpcMessage::Error(_)) =
        serde_json::from_slice::<ServerJsonRpcMessage>(&body)
      {
        return Ok(StreamableHttpPostResponse::Json(error, session));
      }
      return Err(StreamableHttpError::UnexpectedServerResponse(Cow::Owned(format!(
        "HTTP {status}: {}",
        preview(&body)
      ))));
    }
    match serde_json::from_slice::<ServerJsonRpcMessage>(&body) {
      Ok(parsed) => Ok(StreamableHttpPostResponse::Json(parsed, session)),
      // A notification or a reply is owed nothing back, whatever the body holds.
      Err(_) if !expects_answer => Ok(StreamableHttpPostResponse::Accepted),
      Err(error) => Err(StreamableHttpError::UnexpectedServerResponse(Cow::Owned(format!(
        "{error}: {}",
        preview(&body)
      )))),
    }
  }

  async fn delete_session(
    &self,
    uri: Arc<str>,
    session_id: Arc<str>,
    auth_header: Option<String>,
    custom_headers: HashMap<HeaderName, HeaderValue>,
  ) -> Result<(), Failure> {
    let response = self
      .with_common_headers(
        self.client.delete(uri.as_ref()),
        Some(&session_id),
        auth_header,
        custom_headers,
      )
      .send()
      .await
      .map_err(StreamableHttpError::Client)?;
    if response.status() == StatusCode::METHOD_NOT_ALLOWED {
      return Ok(());
    }
    response.error_for_status().map_err(StreamableHttpError::Client)?;
    Ok(())
  }

  async fn get_stream(
    &self,
    uri: Arc<str>,
    session_id: Option<Arc<str>>,
    last_event_id: Option<String>,
    auth_header: Option<String>,
    custom_headers: HashMap<HeaderName, HeaderValue>,
  ) -> Result<BoxStream<'static, Result<Sse, SseError>>, Failure> {
    let mut request = self.with_common_headers(
      self.client.get(uri.as_ref()),
      session_id.as_deref(),
      auth_header,
      custom_headers,
    );
    if let Some(id) = last_event_id {
      request = request.header(LAST_EVENT_ID, id);
    }
    let response = request.send().await.map_err(StreamableHttpError::Client)?;
    if response.status() == StatusCode::METHOD_NOT_ALLOWED {
      return Err(StreamableHttpError::ServerDoesNotSupportSse);
    }
    let response = response.error_for_status().map_err(StreamableHttpError::Client)?;
    let content_type = read_header(&response, CONTENT_TYPE);
    if !content_type.as_deref().is_some_and(|value| value.starts_with(EVENT_STREAM)) {
      return Err(StreamableHttpError::UnexpectedContentType(content_type));
    }
    Ok(read_events(response))
  }
}

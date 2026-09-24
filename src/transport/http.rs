//! The real HTTP transport: one bounded attempt per call.
//!
//! Bounded everywhere (time and bytes); policy stays outside. Non-2xx replies are handed back like
//! any other reply, with their body capped by `Limits::max_error_body_bytes`, so the layer that
//! knows the protocol can classify them: the transport reports facts, it does not interpret them. An
//! error body that cannot be read degrades to an empty body, because the status code is already the
//! conclusion and an unreadable extra must not hide it. Exactly one request leaves this layer per
//! `execute`: no redirect following, and no retry policy of any kind.

use futures_util::StreamExt;

use crate::protocol::error::Error;
use crate::protocol::wire::ReplyStream;
use crate::protocol::wire::{Call, Limits, Method, Reply, Transport, parse_retry_after};

use super::{Proxy, TransportError, build_payload_error, build_read_error, truncate};

/// Attempts HTTP calls with `reqwest`.
#[derive(Clone)]
pub struct ReqwestTransport {
  client: reqwest::Client,
  limits: Limits,
  stream_total: Option<std::time::Duration>,
}

impl ReqwestTransport {
  /// Builds the client: connect timeout, no redirects, and the requested proxy policy.
  pub fn new(limits: Limits, proxy: Proxy) -> Result<Self, Error> {
    let mut builder = reqwest::Client::builder()
      .connect_timeout(limits.connect)
      .redirect(reqwest::redirect::Policy::none())
      // One call means at most one request on the wire. The default policy silently resends on
      // HTTP/2 protocol nacks (`REFUSED_STREAM`, `GOAWAY(NO_ERROR)`): safe by the spec, because the
      // server states it did not process the stream, but invisible here. Retrying belongs above
      // this layer, where it can be seen, measured and switched off per caller.
      .retry(reqwest::retry::never());
    builder = match proxy {
      Proxy::Environment => builder,
      Proxy::Disabled => builder.no_proxy(),
      Proxy::Manual { url, basic_auth } => {
        let mut rule = reqwest::Proxy::all(&url)
          .map_err(|error| Error::Build(format!("proxy {url}: {error}")))?;
        if let Some((username, password)) = &basic_auth {
          rule = rule.basic_auth(username, password);
        }
        builder.proxy(rule)
      }
    };
    let client =
      builder.build().map_err(|error| Error::Build(format!("client build failed: {error}")))?;
    Ok(Self { client, limits, stream_total: None })
  }

  /// Override the wall-clock limit only for streamed responses. Buffered calls keep `limits.total`.
  pub fn with_stream_total(mut self, total: std::time::Duration) -> Self {
    self.stream_total = Some(total);
    self
  }

  /// Renders one request: the caller's header list verbatim, nothing added behind its back.
  fn build_request(&self, call: &Call) -> Result<reqwest::RequestBuilder, Error> {
    let method = match call.method {
      Method::Get => reqwest::Method::GET,
      Method::Post => reqwest::Method::POST,
    };
    let mut headers = reqwest::header::HeaderMap::new();
    for (name, value) in &call.headers {
      let header = reqwest::header::HeaderName::from_bytes(name.as_bytes())
        .map_err(|error| Error::Build(format!("invalid header name `{name}`: {error}")))?;
      let value = reqwest::header::HeaderValue::from_str(value)
        .map_err(|error| Error::Build(format!("invalid `{name}` header value: {error}")))?;
      headers.append(header, value);
    }
    Ok(self.client.request(method, &call.url).headers(headers).body(call.body.clone()))
  }
}

impl Transport for ReqwestTransport {
  type Stream = HttpBodyStream;

  async fn execute(&self, call: &Call) -> Result<Reply, Error> {
    let total_deadline = tokio::time::Instant::now() + self.limits.total;
    let headers_deadline =
      (tokio::time::Instant::now() + self.limits.first_byte).min(total_deadline);
    let response =
      match tokio::time::timeout_at(headers_deadline, self.build_request(call)?.send()).await {
        Ok(Ok(response)) => response,
        Ok(Err(error)) => return Err(decode_request_error(&error)),
        Err(_) => {
          return Err(
            TransportError::AwaitHeaders("no response head within the first-byte limit".to_owned())
              .into(),
          );
        }
      };
    let status = response.status().as_u16();
    let headers = response
      .headers()
      .iter()
      .map(|(name, value)| {
        (name.as_str().to_owned(), value.to_str().unwrap_or_default().to_owned())
      })
      .collect();
    let success = (200..300).contains(&status);
    let limit =
      if success { self.limits.max_response_bytes } else { self.limits.max_error_body_bytes };
    let body = match read_bounded_until(response, limit, total_deadline).await {
      Ok(body) => body,
      // An error body only exists to be classified, so losing it must not hide the status code.
      Err(_) if !success => Vec::new(),
      Err(error) => return Err(error),
    };
    Ok(Reply { status, headers, body })
  }

  async fn execute_stream(&self, call: &Call) -> Result<HttpBodyStream, Error> {
    let mut limits = self.limits;
    limits.total = self.stream_total.unwrap_or(limits.total);
    let total_deadline = tokio::time::Instant::now() + limits.total;
    let headers_deadline =
      (tokio::time::Instant::now() + limits.first_byte).min(total_deadline);
    let response =
      match tokio::time::timeout_at(headers_deadline, self.build_request(call)?.send()).await {
        Ok(Ok(response)) => response,
        Ok(Err(error)) => return Err(decode_request_error(&error)),
        Err(_) => {
          return Err(
            TransportError::AwaitHeaders("no response head within the first-byte limit".to_owned())
              .into(),
          );
        }
      };
    Ok(HttpBodyStream::from_http_response(response, limits))
  }
}

/// One upstream body as it arrives.
///
/// Unlike [`Reply`], nothing is buffered: chunks are handed over as the upstream produced them,
/// which is the whole point of streaming. The streaming half of [`Limits`] applies here: a gap
/// longer than `idle`, an attempt longer than `total`, or a body larger than `max_response_bytes`
/// ends it with a transport error. The reply head is kept as it arrived: a service reports what a
/// call left in its account only there, on a call it really served.
pub struct HttpBodyStream {
  status: u16,
  headers: Vec<(String, String)>,
  retry_after_ms: Option<u64>,
  limits: Limits,
  total_deadline: tokio::time::Instant,
  received: usize,
  response: reqwest::Response,
}

impl HttpBodyStream {
  fn from_http_response(response: reqwest::Response, limits: Limits) -> Self {
    let status = response.status().as_u16();
    let headers: Vec<(String, String)> = response
      .headers()
      .iter()
      .map(|(name, value)| {
        (name.as_str().to_owned(), value.to_str().unwrap_or_default().to_owned())
      })
      .collect();
    let retry_after_ms = response
      .headers()
      .get("retry-after")
      .and_then(|value| value.to_str().ok())
      .and_then(parse_retry_after);
    let total_deadline = tokio::time::Instant::now() + limits.total;
    Self { status, headers, retry_after_ms, limits, total_deadline, received: 0, response }
  }
}

impl ReplyStream for HttpBodyStream {
  fn get_status(&self) -> u16 {
    self.status
  }

  fn get_retry_after_ms(&self) -> Option<u64> {
    self.retry_after_ms
  }

  fn get_headers(&self) -> &[(String, String)] {
    &self.headers
  }

  fn get_limits(&self) -> Limits {
    self.limits
  }

  /// The next chunk of the body, or `None` at its end.
  ///
  /// Each wait is bounded by the idle limit and by what is left of the total limit; whichever one
  /// runs out is the error reported.
  async fn next(&mut self) -> Result<Option<Vec<u8>>, Error> {
    let now = tokio::time::Instant::now();
    if now >= self.total_deadline {
      return Err(TransportError::ReadBody("stream exceeded the total timeout".to_owned()).into());
    }
    let idle_deadline = (now + self.limits.idle).min(self.total_deadline);
    let chunk = match tokio::time::timeout_at(idle_deadline, self.response.chunk()).await {
      Ok(chunk) => {
        let chunk = chunk.map_err(|error| build_read_error(&error.to_string()))?;
        chunk.map(|chunk| chunk.to_vec())
      }
      Err(_) if idle_deadline >= self.total_deadline => {
        return Err(
          TransportError::ReadBody("stream exceeded the total timeout".to_owned()).into(),
        );
      }
      Err(_) => {
        return Err(
          TransportError::ReadBody("no body chunk within the idle limit".to_owned()).into(),
        );
      }
    };
    let Some(chunk) = chunk else { return Ok(None) };
    self.received += chunk.len();
    if self.received > self.limits.max_response_bytes {
      return Err(build_payload_error(&format!(
        "stream exceeds {} bytes",
        self.limits.max_response_bytes
      )));
    }
    Ok(Some(chunk))
  }
}

/// Maps a `reqwest` failure onto one attempt phase. The message is capped: an upstream must never
/// be able to flood a log line with text of its own choosing.
fn decode_request_error(error: &reqwest::Error) -> Error {
  let message = truncate(&error.to_string());
  if error.is_connect() {
    TransportError::Connect(message).into()
  } else if error.is_timeout() {
    TransportError::AwaitHeaders(message).into()
  } else {
    TransportError::ReadBody(message).into()
  }
}

async fn read_bounded(response: reqwest::Response, max_bytes: usize) -> Result<Vec<u8>, Error> {
  let mut body = Vec::new();
  let mut stream = response.bytes_stream();
  while let Some(chunk) = stream.next().await {
    let chunk = chunk.map_err(|error| build_read_error(&error.to_string()))?;
    body.extend_from_slice(&chunk);
    if body.len() > max_bytes {
      return Err(build_payload_error(&format!("response body exceeds {max_bytes} bytes")));
    }
  }
  Ok(body)
}

async fn read_bounded_until(
  response: reqwest::Response,
  max_bytes: usize,
  deadline: tokio::time::Instant,
) -> Result<Vec<u8>, Error> {
  match tokio::time::timeout_at(deadline, read_bounded(response, max_bytes)).await {
    Ok(body) => body,
    Err(_) => Err(build_read_error("response body exceeded the total timeout")),
  }
}

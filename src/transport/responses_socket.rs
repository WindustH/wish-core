//! A Responses WebSocket belongs to one accepted chain. Moving the live socket into
//! the response reader means cancellation, a bad frame or an abandoned turn closes it.
use super::{build_read_error, http::ReqwestTransport, record::Record};
use crate::protocol::{
  attempt::{Call, Limits},
  error::Error,
};
use futures_util::{FutureExt, SinkExt, StreamExt};
use std::sync::{Arc, Mutex};
use tokio::time::{Instant, timeout, timeout_at};
use tokio_tungstenite::{
  WebSocketStream,
  tungstenite::{
    Message,
    handshake::{client::generate_key, derive_accept_key},
    protocol::{Role, WebSocketConfig},
  },
};

type Wire = WebSocketStream<reqwest::Upgraded>;

struct Connection {
  wire: Wire,
  expiry: Option<tokio_util::task::AbortOnDropHandle<()>>,
  headers: Vec<(String, String)>,
}
#[derive(Clone)]
pub(crate) struct ResponsesSocket {
  client: reqwest::Client,
  limits: Limits,
  idle: Arc<Mutex<Option<Connection>>>,
}
pub(crate) struct ResponseFrames {
  slot: ResponsesSocket,
  connection: Option<Connection>,
  deadline: Instant,
  bytes: usize,
  events: u64,
}
impl ResponsesSocket {
  pub fn new(transport: &ReqwestTransport) -> Self {
    let mut limits = transport.limits;
    limits.total = transport.stream_total.unwrap_or(limits.total);
    Self { client: transport.client.clone(), limits, idle: Default::default() }
  }
  async fn connect(&self, call: &Call) -> Result<Connection, Error> {
    let key = generate_key();
    let mut request = self.client.get(&call.url).version(reqwest::Version::HTTP_11);
    for (name, value) in &call.headers {
      if !matches!(
        name.to_ascii_lowercase().as_str(),
        "content-type"
          | "content-length"
          | "connection"
          | "upgrade"
          | "sec-websocket-key"
          | "sec-websocket-version"
      ) {
        request = request.header(name, value);
      }
    }
    if !call.headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("openai-beta")) {
      request = request.header("OpenAI-Beta", "responses_websockets=2026-02-06");
    }
    let response = request
      .header("Connection", "Upgrade")
      .header("Upgrade", "websocket")
      .header("Sec-WebSocket-Version", "13")
      .header("Sec-WebSocket-Key", &key)
      .send()
      .await
      .map_err(|_| Error::Upstream {
        status: None,
        code: Some("websocket_handshake".into()),
        message: "Responses WebSocket handshake unavailable".into(),
        retry_after_ms: None,
      })?;
    if response.status() != 101 {
      return Err(Error::Upstream {
        status: Some(response.status().as_u16()),
        code: Some("websocket_handshake".into()),
        message: "Responses WebSocket upgrade declined".into(),
        retry_after_ms: None,
      });
    }
    if response.headers().get("sec-websocket-accept").and_then(|v| v.to_str().ok())
      != Some(derive_accept_key(key.as_bytes()).as_str())
    {
      return Err(Error::Malformed("invalid WebSocket handshake".into()));
    }
    let headers = response
      .headers()
      .iter()
      .filter_map(|(k, v)| Some((k.to_string(), v.to_str().ok()?.to_owned())))
      .collect();
    let upgraded = response.upgrade().await.map_err(|e| build_read_error(&e.to_string()))?;
    let config = WebSocketConfig::default()
      .max_frame_size(Some(self.limits.max_event_bytes))
      .max_message_size(Some(self.limits.max_event_bytes));
    let wire = WebSocketStream::from_raw_socket(upgraded, Role::Client, Some(config)).await;
    Ok(Connection { wire, headers, expiry: None })
  }
  pub async fn start(&self, call: &Call, continuing: bool) -> Result<ResponseFrames, Error> {
    let idle = self.idle.lock().unwrap().take();
    let mut connection = match idle {
      Some(c) => c,
      None if continuing => {
        return Err(Error::Upstream {
          status: Some(410),
          code: Some("previous_response_not_found".into()),
          message: "previous response socket expired".into(),
          retry_after_ms: None,
        });
      }
      None => timeout(self.limits.first_byte, self.connect(call)).await.map_err(|_| {
        super::TransportError::Connect("Responses WebSocket handshake timed out".into())
      })??,
    };
    connection.expiry.take();
    // An idle peer may already have sent its close frame. Discover that before
    // writing a new generation, when a fresh full replay is still unambiguous.
    if continuing {
      while let Some(frame) = connection.wire.next().now_or_never() {
        match frame {
          Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
          _ => {
            return Err(Error::Upstream {
              status: Some(410),
              code: Some("previous_response_not_found".into()),
              message: "previous response socket closed".into(),
              retry_after_ms: None,
            });
          }
        }
      }
    }
    let frame = call
      .body
      .clone()
      .try_into()
      .map_err(|_| Error::Build("invalid Responses frame encoding".into()))?;
    timeout(self.limits.first_byte, connection.wire.send(Message::Text(frame)))
      .await
      .map_err(|_| build_read_error("WebSocket send timed out"))?
      .map_err(|e| build_read_error(&e.to_string()))?;
    Ok(ResponseFrames {
      slot: self.clone(),
      connection: Some(connection),
      deadline: Instant::now() + self.limits.total,
      bytes: 0,
      events: 0,
    })
  }
}
impl ResponseFrames {
  pub fn headers(&self) -> &[(String, String)] {
    self.connection.as_ref().map_or(&[], |c| c.headers.as_slice())
  }
  pub fn release(&mut self) {
    if let Some(mut connection) = self.connection.take() {
      let slot = Arc::downgrade(&self.slot.idle);
      connection.expiry =
        Some(tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
          tokio::time::sleep(std::time::Duration::from_secs(120)).await;
          if let Some(slot) = slot.upgrade() {
            slot.lock().unwrap().take();
          }
        })));
      *self.slot.idle.lock().unwrap() = Some(connection);
    }
  }
  pub async fn next(&mut self) -> Result<Option<Record>, Error> {
    let Some(connection) = self.connection.as_mut() else { return Ok(None) };
    loop {
      let deadline = self.deadline.min(Instant::now() + self.slot.limits.idle);
      let frame = timeout_at(deadline, connection.wire.next())
        .await
        .map_err(|_| build_read_error("Responses WebSocket timed out"))?
        .ok_or_else(|| build_read_error("Responses WebSocket closed before completion"))?
        .map_err(|e| build_read_error(&e.to_string()))?;
      let data = match frame {
        Message::Text(text) => text.to_string(),
        Message::Ping(_) | Message::Pong(_) => continue,
        _ => return Err(build_read_error("expected a WebSocket response event")),
      };
      self.events += 1;
      self.bytes = self.bytes.saturating_add(data.len());
      if self.events > self.slot.limits.max_stream_events
        || self.bytes > self.slot.limits.max_response_bytes
      {
        return Err(Error::Malformed("Responses WebSocket exceeded stream limits".into()));
      }
      return Ok(Some(Record { event: None, data }));
    }
  }
}

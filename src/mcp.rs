//! Clients of MCP servers: local ones over stdio, remote ones over Streamable HTTP.
//!
//! The protocol is rmcp's. What lives here is how wish reaches a server - the program it starts in a
//! process group of its own, or the URL it posts to through wish's HTTP client - and the narrow
//! surface the application uses: list the tools, call one, close.

mod http;

use crate::transport::Proxy;
use reqwest::header::{HeaderName, HeaderValue};
use rmcp::{
  ClientHandler, RoleClient, ServiceExt,
  model::{
    CallToolRequest, CallToolRequestParams, CancelledNotificationParam, ClientRequest, RequestId,
    ServerResult,
  },
  service::{NotificationContext, Peer, PeerRequestOptions, RunningService, ServiceError},
  transport::{
    StreamableHttpClientTransport, TokioChildProcess,
    streamable_http_client::StreamableHttpClientTransportConfig,
  },
};
use serde_json::{Map, Value, json};
use std::{
  collections::{BTreeMap, HashMap},
  future::Future,
  path::{Path, PathBuf},
  process::Stdio,
  sync::{Arc, Mutex},
  time::Duration,
};
use tokio::{io::AsyncReadExt, process::ChildStderr};

/// How long a server has to start and finish the handshake. Generous, because a first `npx` run
/// downloads the server before it starts.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(120);
/// The end of a local server's standard error kept for telling why it failed.
const STDERR_TAIL: usize = 8 * 1024;

/// Where a server is and how to reach it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Endpoint {
  /// A local program speaking MCP on its standard input and output.
  Stdio { command: String, args: Vec<String>, env: BTreeMap<String, String>, cwd: PathBuf },
  /// A remote server speaking Streamable HTTP.
  Http { url: String, headers: BTreeMap<String, String>, proxy: Proxy },
}

#[derive(Debug, thiserror::Error)]
pub enum McpError {
  /// The server could not be started or reached, or failed the handshake.
  #[error("{0}")]
  Connect(String),
  /// The server answered with an error, or with something that is not an answer.
  #[error("{0}")]
  Rejected(String),
  /// No answer came: the connection closed or the time ran out, so a call may have taken effect.
  #[error("{0}")]
  Unknown(String),
}

fn to_mcp_error(error: ServiceError) -> McpError {
  match error {
    ServiceError::McpError(error) => McpError::Rejected(error.message.into_owned()),
    ServiceError::Timeout { timeout } => McpError::Unknown(format!(
      "the server did not answer within {} seconds; the call may still take effect",
      timeout.as_secs()
    )),
    ServiceError::TransportClosed
    | ServiceError::TransportSend(_)
    | ServiceError::Cancelled { .. } => {
      McpError::Unknown(format!("the connection to the server closed: {error}"))
    }
    error => McpError::Rejected(error.to_string()),
  }
}

/// What wish tells a server about itself, and hears back from it.
struct Handler {
  tools_changed: Box<dyn Fn() + Send + Sync>,
}
impl ClientHandler for Handler {
  async fn on_tool_list_changed(&self, _context: NotificationContext<RoleClient>) {
    (self.tools_changed)();
  }
  fn get_info(&self) -> rmcp::model::ClientConfig {
    rmcp::model::ClientConfig::new(
      Default::default(),
      rmcp::model::Implementation::new("wish", env!("CARGO_PKG_VERSION")),
    )
  }
}

/// One live connection to a server.
pub struct Connection {
  service: RunningService<RoleClient, Handler>,
  stderr: Option<Arc<Mutex<String>>>,
}

impl Connection {
  /// Starts or reaches the server and completes the handshake. `tools_changed` runs whenever the
  /// server says its tools changed.
  pub async fn open(
    endpoint: &Endpoint,
    tools_changed: impl Fn() + Send + Sync + 'static,
  ) -> Result<Self, McpError> {
    let handler = Handler { tools_changed: Box::new(tools_changed) };
    match endpoint {
      Endpoint::Stdio { command, args, env, cwd } => {
        Self::open_stdio(handler, command, args, env, cwd).await
      }
      Endpoint::Http { url, headers, proxy } => Self::open_http(handler, url, headers, proxy).await,
    }
  }

  /// Starts a local server in a process group of its own, keeping the end of its standard error.
  async fn open_stdio(
    handler: Handler,
    command: &str,
    args: &[String],
    env: &BTreeMap<String, String>,
    cwd: &Path,
  ) -> Result<Self, McpError> {
    let mut program = tokio::process::Command::new(command);
    program.args(args).envs(env).current_dir(cwd);
    let mut wrapped = process_wrap::tokio::CommandWrap::from(program);
    #[cfg(unix)]
    wrapped.wrap(process_wrap::tokio::ProcessGroup::leader());
    #[cfg(windows)]
    wrapped.wrap(process_wrap::tokio::JobObject);
    wrapped.wrap(process_wrap::tokio::KillOnDrop);
    let (transport, stderr) = TokioChildProcess::builder(wrapped)
      .stderr(Stdio::piped())
      .spawn()
      .map_err(|error| McpError::Connect(format!("could not start `{command}`: {error}")))?;
    let tail = spawn_stderr_tail(stderr);
    match handshake(handler.serve(transport)).await {
      Ok(service) => Ok(Self { service, stderr: Some(tail) }),
      Err(Some(error)) => {
        // The server's own words usually say more than the handshake's failure.
        tokio::time::sleep(Duration::from_millis(100)).await;
        Err(McpError::Connect(with_stderr(format!("the server failed to start: {error}"), &tail)))
      }
      Err(None) => Err(McpError::Connect(with_stderr(
        format!("the server did not finish starting within {} seconds", STARTUP_TIMEOUT.as_secs()),
        &tail,
      ))),
    }
  }

  /// Reaches a remote server through wish's HTTP client.
  async fn open_http(
    handler: Handler,
    url: &str,
    headers: &BTreeMap<String, String>,
    proxy: &Proxy,
  ) -> Result<Self, McpError> {
    let client =
      http::HttpClient::new(proxy.clone()).map_err(|error| McpError::Connect(error.to_string()))?;
    let mut custom = HashMap::new();
    for (name, value) in headers {
      let name = HeaderName::from_bytes(name.as_bytes())
        .map_err(|error| McpError::Connect(format!("header `{name}`: {error}")))?;
      let value = HeaderValue::from_str(value)
        .map_err(|error| McpError::Connect(format!("header `{name}`: {error}")))?;
      custom.insert(name, value);
    }
    let mut config = StreamableHttpClientTransportConfig::with_uri(url);
    config.custom_headers = custom;
    let transport = StreamableHttpClientTransport::with_client(client, config);
    match handshake(handler.serve(transport)).await {
      Ok(service) => Ok(Self { service, stderr: None }),
      Err(Some(error)) => Err(McpError::Connect(format!("could not connect: {error}"))),
      Err(None) => Err(McpError::Connect(format!(
        "the server did not answer the handshake within {} seconds",
        STARTUP_TIMEOUT.as_secs()
      ))),
    }
  }

  /// Whether the connection has ended, so the next use needs a new one.
  pub fn is_closed(&self) -> bool {
    self.service.is_closed()
  }

  /// The server's name and version, as it gave them in the handshake.
  pub fn describe_server(&self) -> Value {
    let info = self.service.peer().peer_info();
    let server = info.as_ref().and_then(|info| info.server_info.as_ref());
    json!({"name": server.map(|s| &s.name), "version": server.map(|s| &s.version)})
  }

  /// What the server wrote to its standard error lately; empty for a remote server.
  pub fn get_stderr(&self) -> String {
    self.stderr.as_ref().map(|tail| tail.lock().unwrap().clone()).unwrap_or_default()
  }

  /// Every tool the server offers, as the protocol describes them.
  pub async fn list_tools(&self) -> Result<Vec<Value>, McpError> {
    let tools = self.service.peer().list_all_tools().await.map_err(to_mcp_error)?;
    Ok(tools.iter().filter_map(|tool| serde_json::to_value(tool).ok()).collect())
  }

  /// Calls a tool and returns its result as the protocol describes it. A call dropped before its
  /// answer tells the server to stop; one that runs past `timeout` without a progress report is
  /// abandoned the same way.
  pub async fn call_tool(
    &self,
    name: &str,
    arguments: Map<String, Value>,
    timeout: Duration,
  ) -> Result<Value, McpError> {
    let request = ClientRequest::CallToolRequest(CallToolRequest::new(
      CallToolRequestParams::new(name.to_owned()).with_arguments(arguments),
    ));
    let mut options = PeerRequestOptions::with_timeout(timeout);
    options.reset_timeout_on_progress = true;
    let handle =
      self.service.peer().send_cancellable_request(request, options).await.map_err(to_mcp_error)?;
    let mut pending = PendingCall { peer: handle.peer.clone(), id: Some(handle.id.clone()) };
    let result = handle.await_response().await;
    pending.id = None;
    match result.map_err(to_mcp_error)? {
      ServerResult::CallToolResult(result) => {
        serde_json::to_value(result).map_err(|error| McpError::Rejected(error.to_string()))
      }
      ServerResult::InputRequiredResult(_) => Err(McpError::Rejected(
        "the server asked for more input during the call, which wish does not support yet".into(),
      )),
      ServerResult::CreateTaskResult(_) => Err(McpError::Rejected(
        "the server turned the call into a task, which wish does not support yet".into(),
      )),
      _ => Err(McpError::Rejected("the server answered the call with something else".into())),
    }
  }

  /// Ends the connection; a local server's whole process group goes with it.
  pub async fn close(self) {
    let _ = self.service.cancel().await;
  }
}

/// Tells the server to stop a call nobody is waiting for any more.
struct PendingCall {
  peer: Peer<RoleClient>,
  id: Option<RequestId>,
}
impl Drop for PendingCall {
  fn drop(&mut self) {
    if let Some(id) = self.id.take() {
      let peer = self.peer.clone();
      tokio::spawn(async move {
        let reason = Some("the caller stopped waiting".to_owned());
        let _ = peer.notify_cancelled(CancelledNotificationParam::new(Some(id), reason)).await;
      });
    }
  }
}

/// Awaits a server's handshake for at most [`STARTUP_TIMEOUT`]: its failure, or `None` when the
/// time ran out.
async fn handshake<S, E>(serving: impl Future<Output = Result<S, E>>) -> Result<S, Option<E>> {
  match tokio::time::timeout(STARTUP_TIMEOUT, serving).await {
    Ok(result) => result.map_err(Some),
    Err(_) => Err(None),
  }
}

/// Keeps the last [`STDERR_TAIL`] bytes a local server writes to its standard error.
fn spawn_stderr_tail(stderr: Option<ChildStderr>) -> Arc<Mutex<String>> {
  let tail = Arc::new(Mutex::new(String::new()));
  if let Some(mut stderr) = stderr {
    let tail = tail.clone();
    tokio::spawn(async move {
      let mut buffer = [0; 4096];
      while let Ok(read) = stderr.read(&mut buffer).await {
        if read == 0 {
          break;
        }
        let mut tail = tail.lock().unwrap();
        tail.push_str(&String::from_utf8_lossy(&buffer[..read]));
        if tail.len() > STDERR_TAIL {
          let mut cut = tail.len() - STDERR_TAIL;
          while !tail.is_char_boundary(cut) {
            cut += 1;
          }
          tail.drain(..cut);
        }
      }
    });
  }
  tail
}

fn with_stderr(message: String, tail: &Mutex<String>) -> String {
  let tail = tail.lock().unwrap();
  let tail = tail.trim();
  if tail.is_empty() { message } else { format!("{message}\n{tail}") }
}

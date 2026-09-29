//! MCP servers for sessions: their configuration, the live connections, and what a session's shell
//! reaches them through.
//!
//! Nothing here enters the model's tool list. Every session with a shell gets a fixed note in its
//! shell tool's description and a few variables in its shell's environment; the model finds the
//! servers and their tools by running `wish mcp`, and the answers arrive as ordinary command output.
//! Servers and their tools can change at any time without touching the request a conversation's
//! prompt cache is keyed on, and so can a session's MCP switch: it decides only what `wish mcp`
//! answers.
//!
//! A server runs once per session by default (`scope: "session"`), started in the session's working
//! directory the first time that session calls it. That is how servers are written to be used - one
//! client each - so it is right whether or not a server keeps state. `scope: "shared"` has every
//! session use one instance, for servers known to keep none. An instance nobody has called for
//! `idle_timeout` seconds is closed, and started again on the next call.

pub mod cli;

use crate::mcp::{Connection, Endpoint, McpError};
use crate::server::{config::ProxyConfig, error::ApiError, provider::Provider};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::{
  collections::{BTreeMap, HashMap},
  path::{Path, PathBuf},
  sync::{
    Arc, Mutex, RwLock,
    atomic::{AtomicUsize, Ordering},
  },
  time::{Duration, Instant},
};

/// Appended to `shell_start`'s description in every session with a shell. It names no server and no
/// tool, and does not depend on the session's MCP switch, so it never changes.
pub const SHELL_NOTE: &str = if cfg!(windows) {
  // cmd.exe has no single quotes, and quoting JSON for it is fragile: the arguments go on standard
  // input instead, which `wish mcp call` reads when they are left out.
  " MCP servers are available through the `wish mcp` command: `wish mcp list` shows the servers and their tools, `wish mcp describe <server>/<tool>` shows a tool's parameters, and `wish mcp call <server>/<tool>` calls it with the JSON arguments read from standard input (give them in shell_start's data) and prints the result. Scripts can run it too, to chain calls and filter results before printing them."
} else {
  " MCP servers are available through the `wish mcp` command: `wish mcp list` shows the servers and their tools, `wish mcp describe <server>/<tool>` shows a tool's parameters, and `wish mcp call <server>/<tool> '<json arguments>'` calls it and prints the result. Scripts can run it too, to chain calls and filter results before printing them."
};

/// Headers a server's configuration may not set: the transport sends them itself.
const RESERVED_HEADERS: &[&str] =
  &["accept", "content-type", "mcp-session-id", "mcp-protocol-version", "last-event-id"];

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct McpConfig {
  pub servers: BTreeMap<String, McpServerConfig>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum McpTransport {
  /// A local program speaking MCP on its standard input and output.
  Stdio,
  /// A remote server speaking Streamable HTTP.
  Http,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum McpScope {
  /// One instance per session, in the session's working directory.
  #[default]
  Session,
  /// One instance every session uses.
  Shared,
}

fn yes() -> bool {
  true
}
fn default_idle_timeout() -> u64 {
  30 * 60
}
fn default_timeout() -> u64 {
  5 * 60
}

/// One configured server.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct McpServerConfig {
  #[serde(default = "yes")]
  pub enabled: bool,
  pub transport: McpTransport,
  /// stdio: the program, found on `PATH` when not a path.
  #[serde(default, skip_serializing_if = "String::is_empty")]
  pub command: String,
  #[serde(default, skip_serializing_if = "Vec::is_empty")]
  pub args: Vec<String>,
  /// stdio: variables added to the environment wish runs in.
  #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
  pub env: BTreeMap<String, String>,
  /// stdio: where the program starts. Absent, a session's instance starts in the session's working
  /// directory and a shared one in the default working directory.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub cwd: Option<PathBuf>,
  /// http: the server's endpoint.
  #[serde(default, skip_serializing_if = "String::is_empty")]
  pub url: String,
  #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
  pub headers: BTreeMap<String, String>,
  /// http: a provider whose key is sent as `Authorization: Bearer`, for a server that comes with a
  /// subscription the provider already holds.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub auth_provider: Option<String>,
  #[serde(default = "yes")]
  pub proxy_enabled: bool,
  #[serde(default)]
  pub scope: McpScope,
  /// Seconds without a call after which an instance is closed; 0 keeps it open.
  #[serde(default = "default_idle_timeout")]
  pub idle_timeout: u64,
  /// Seconds a call may go without an answer or a progress report.
  #[serde(default = "default_timeout")]
  pub timeout: u64,
}

impl McpConfig {
  pub fn validate(
    &self,
    providers: &BTreeMap<String, crate::server::provider::ProviderConfig>,
  ) -> Result<(), ApiError> {
    for (id, server) in &self.servers {
      let invalid =
        |message: String| ApiError::bad_request(format!("MCP server `{id}`: {message}"));
      if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(ApiError::bad_request(format!(
          "MCP server name `{id}` may only use letters, digits, `-` and `_`"
        )));
      }
      if server.timeout == 0 {
        return Err(invalid("timeout must be at least one second".into()));
      }
      match server.transport {
        McpTransport::Stdio => {
          if server.command.trim().is_empty() {
            return Err(invalid("a stdio server needs a command".into()));
          }
          if server.cwd.as_ref().is_some_and(|cwd| !cwd.is_absolute()) {
            return Err(invalid("cwd must be an absolute path".into()));
          }
        }
        McpTransport::Http => {
          let url =
            reqwest::Url::parse(&server.url).map_err(|_| invalid("url must be valid".into()))?;
          if !matches!(url.scheme(), "http" | "https") {
            return Err(invalid("url must use http:// or https://".into()));
          }
          for name in server.headers.keys() {
            let lower = name.to_ascii_lowercase();
            if RESERVED_HEADERS.contains(&lower.as_str()) {
              return Err(invalid(format!("the transport sets `{name}` itself")));
            }
            if lower == "authorization" && server.auth_provider.is_some() {
              return Err(invalid("set either an Authorization header or auth_provider".into()));
            }
          }
          if let Some(provider) = &server.auth_provider
            && !providers.contains_key(provider)
          {
            return Err(invalid(format!("auth_provider `{provider}` does not exist")));
          }
        }
      }
    }
    Ok(())
  }
}

/// Why an MCP request could not be served.
#[derive(Debug)]
pub enum McpFailure {
  /// No such server or tool, or the server is switched off.
  NotFound(String),
  /// The server could not be started or reached.
  Connect(String),
  /// The server refused, or answered with something else.
  Rejected(String),
  /// No answer came; a call may have taken effect.
  Unknown(String),
}
impl From<McpError> for McpFailure {
  fn from(error: McpError) -> Self {
    match error {
      McpError::Connect(message) => Self::Connect(message),
      McpError::Rejected(message) => Self::Rejected(message),
      McpError::Unknown(message) => Self::Unknown(message),
    }
  }
}
impl From<McpFailure> for ApiError {
  fn from(failure: McpFailure) -> Self {
    use axum::http::StatusCode;
    let (status, kind, message) = match failure {
      McpFailure::NotFound(message) => (StatusCode::NOT_FOUND, "not_found", message),
      McpFailure::Connect(message) => (StatusCode::BAD_GATEWAY, "connect", message),
      McpFailure::Rejected(message) => (StatusCode::UNPROCESSABLE_ENTITY, "rejected", message),
      McpFailure::Unknown(message) => (StatusCode::GATEWAY_TIMEOUT, "unknown", message),
    };
    ApiError { status, message, details: Some(json!({"kind": kind})) }
  }
}

/// The session a request comes from.
pub struct Caller<'a> {
  pub session: &'a str,
  pub cwd: &'a Path,
}

/// What the application holds about MCP apart from the connections themselves.
struct Settings {
  config: McpConfig,
  proxy: ProxyConfig,
  default_cwd: PathBuf,
}

#[derive(Clone, Hash, PartialEq, Eq)]
struct InstanceKey {
  server: String,
  /// The session an instance belongs to; `None` for a shared one.
  session: Option<String>,
}

struct Instance {
  endpoint: Endpoint,
  cwd: PathBuf,
  connection: tokio::sync::OnceCell<Connection>,
  last_used: Mutex<Instant>,
  active: AtomicUsize,
}

/// What is known of a server's tools, kept across its instances: the same configuration offers the
/// same tools, so listing them never needs a session's own instance.
#[derive(Clone, Default)]
struct Catalog {
  tools: Option<Vec<Value>>,
  server: Value,
  error: Option<String>,
  checked_at: Option<u64>,
}

pub struct McpHub {
  settings: RwLock<Settings>,
  instances: Mutex<HashMap<InstanceKey, Arc<Instance>>>,
  catalogs: Arc<Mutex<HashMap<String, Catalog>>>,
}

impl McpHub {
  pub fn new(config: McpConfig, proxy: ProxyConfig, default_cwd: PathBuf) -> Self {
    Self {
      settings: RwLock::new(Settings { config, proxy, default_cwd }),
      instances: Mutex::new(HashMap::new()),
      catalogs: Arc::new(Mutex::new(HashMap::new())),
    }
  }

  fn resolve(
    &self,
    server: &McpServerConfig,
    cwd: &Path,
    providers: &BTreeMap<String, Arc<Provider>>,
  ) -> Result<Endpoint, McpFailure> {
    let settings = self.settings.read().unwrap();
    Ok(match server.transport {
      McpTransport::Stdio => Endpoint::Stdio {
        command: server.command.clone(),
        args: server.args.clone(),
        env: server.env.clone(),
        cwd: cwd.to_owned(),
      },
      McpTransport::Http => {
        let mut headers = server.headers.clone();
        if let Some(id) = &server.auth_provider {
          let provider = providers
            .get(id)
            .filter(|provider| provider.config.enabled)
            .ok_or_else(|| McpFailure::Connect(format!("provider `{id}` is not available")))?;
          headers
            .insert("authorization".into(), format!("Bearer {}", provider.client.get_api_key()));
        }
        Endpoint::Http {
          url: server.url.clone(),
          headers,
          proxy: settings.proxy.policy(server.proxy_enabled),
        }
      }
    })
  }

  /// The directory a server's instance starts in.
  fn start_directory(&self, server: &McpServerConfig, caller: Option<&Caller>) -> PathBuf {
    if let Some(cwd) = &server.cwd {
      return cwd.clone();
    }
    match (server.scope, caller) {
      (McpScope::Session, Some(caller)) => caller.cwd.to_owned(),
      _ => self.settings.read().unwrap().default_cwd.clone(),
    }
  }

  fn find_server(&self, id: &str) -> Result<McpServerConfig, McpFailure> {
    self
      .settings
      .read()
      .unwrap()
      .config
      .servers
      .get(id)
      .filter(|server| server.enabled)
      .cloned()
      .ok_or_else(|| McpFailure::NotFound(format!("no MCP server `{id}`")))
  }

  /// The instance a caller uses for a server, created when it has none yet. A closed one is replaced.
  fn get_instance(
    &self,
    id: &str,
    server: &McpServerConfig,
    caller: &Caller,
    providers: &BTreeMap<String, Arc<Provider>>,
  ) -> Result<Arc<Instance>, McpFailure> {
    let key = InstanceKey {
      server: id.to_owned(),
      session: (server.scope == McpScope::Session).then(|| caller.session.to_owned()),
    };
    let mut instances = self.instances.lock().unwrap();
    if let Some(instance) = instances.get(&key)
      && !instance.connection.get().is_some_and(Connection::is_closed)
    {
      return Ok(instance.clone());
    }
    let cwd = self.start_directory(server, Some(caller));
    let endpoint = self.resolve(server, &cwd, providers)?;
    let instance = Arc::new(Instance {
      endpoint,
      cwd,
      connection: tokio::sync::OnceCell::new(),
      last_used: Mutex::new(Instant::now()),
      active: AtomicUsize::new(0),
    });
    if let Some(previous) = instances.insert(key, instance.clone()) {
      close_later(previous);
    }
    Ok(instance)
  }

  /// The instance's connection, opened on first use. Opening lists the tools into the catalog.
  async fn connect<'a>(
    &self,
    id: &str,
    instance: &'a Instance,
  ) -> Result<&'a Connection, McpFailure> {
    let catalogs = self.catalogs.clone();
    let result = instance
      .connection
      .get_or_try_init(|| async {
        let stale = {
          let (catalogs, server) = (Arc::downgrade(&catalogs), id.to_owned());
          move || {
            if let Some(catalogs) = catalogs.upgrade()
              && let Some(catalog) = catalogs.lock().unwrap().get_mut(&server)
            {
              catalog.tools = None;
            }
          }
        };
        let connection = Connection::open(&instance.endpoint, stale).await?;
        let tools = connection.list_tools().await;
        let mut catalogs = catalogs.lock().unwrap();
        let catalog = catalogs.entry(id.to_owned()).or_default();
        catalog.server = connection.describe_server();
        catalog.checked_at = Some(crate::session::statistics::Timestamp::now().0);
        match tools {
          Ok(tools) => {
            catalog.tools = Some(tools);
            catalog.error = None;
          }
          Err(error) => catalog.error = Some(format!("listing the tools failed: {error}")),
        }
        Ok::<_, McpError>(connection)
      })
      .await;
    match result {
      Ok(connection) => Ok(connection),
      Err(error) => {
        let mut catalogs = self.catalogs.lock().unwrap();
        let catalog = catalogs.entry(id.to_owned()).or_default();
        catalog.error = Some(error.to_string());
        catalog.checked_at = Some(crate::session::statistics::Timestamp::now().0);
        Err(error.into())
      }
    }
  }

  /// A server's tools: from the catalog, or read through the caller's instance when unknown.
  async fn get_tools(
    &self,
    id: &str,
    caller: &Caller<'_>,
    providers: &BTreeMap<String, Arc<Provider>>,
  ) -> Result<Vec<Value>, McpFailure> {
    if let Some(tools) = self.catalogs.lock().unwrap().get(id).and_then(|c| c.tools.clone()) {
      return Ok(tools);
    }
    let server = self.find_server(id)?;
    let instance = self.get_instance(id, &server, caller, providers)?;
    let connection = self.connect(id, &instance).await?;
    let tools = connection.list_tools().await?;
    self.catalogs.lock().unwrap().entry(id.to_owned()).or_default().tools = Some(tools.clone());
    Ok(tools)
  }

  /// Every enabled server with its tools, each summarized by the first line of its description.
  pub async fn list(
    &self,
    caller: &Caller<'_>,
    providers: &BTreeMap<String, Arc<Provider>>,
    only: Option<&str>,
  ) -> Result<Vec<Value>, McpFailure> {
    let ids: Vec<String> = {
      let settings = self.settings.read().unwrap();
      settings
        .config
        .servers
        .iter()
        .filter(|(id, server)| server.enabled && only.is_none_or(|only| only == id.as_str()))
        .map(|(id, _)| id.clone())
        .collect()
    };
    if let Some(only) = only
      && ids.is_empty()
    {
      return Err(McpFailure::NotFound(format!("no MCP server `{only}`")));
    }
    let listed = futures_util::future::join_all(ids.iter().map(|id| async move {
      match self.get_tools(id, caller, providers).await {
        Ok(tools) => json!({"server": id, "tools": tools.iter().map(|tool| json!({
          "name": tool["name"],
          "description": tool["description"].as_str().and_then(|d| d.lines().map(str::trim).find(|l| !l.is_empty())),
        })).collect::<Vec<_>>()}),
        Err(error) => json!({"server": id, "error": ApiError::from(error).message}),
      }
    }))
    .await;
    Ok(listed)
  }

  /// One tool's full definition.
  pub async fn describe_tool(
    &self,
    id: &str,
    tool: &str,
    caller: &Caller<'_>,
    providers: &BTreeMap<String, Arc<Provider>>,
  ) -> Result<Value, McpFailure> {
    self.find_server(id)?;
    self
      .get_tools(id, caller, providers)
      .await?
      .into_iter()
      .find(|candidate| candidate["name"] == tool)
      .ok_or_else(|| McpFailure::NotFound(format!("MCP server `{id}` has no tool `{tool}`")))
  }

  /// Calls a tool through the caller's instance of the server.
  pub async fn call(
    &self,
    id: &str,
    tool: &str,
    arguments: Map<String, Value>,
    caller: &Caller<'_>,
    providers: &BTreeMap<String, Arc<Provider>>,
  ) -> Result<Value, McpFailure> {
    let server = self.find_server(id)?;
    let instance = self.get_instance(id, &server, caller, providers)?;
    let connection = self.connect(id, &instance).await?;
    instance.active.fetch_add(1, Ordering::AcqRel);
    let _busy = Busy(&instance);
    Ok(connection.call_tool(tool, arguments, Duration::from_secs(server.timeout)).await?)
  }

  /// Opens a connection of its own to a server, reads its tools into the catalog and closes it: the
  /// check behind the settings page's button.
  pub async fn check(
    &self,
    id: &str,
    providers: &BTreeMap<String, Arc<Provider>>,
  ) -> Result<Value, McpFailure> {
    let server = self
      .settings
      .read()
      .unwrap()
      .config
      .servers
      .get(id)
      .cloned()
      .ok_or_else(|| McpFailure::NotFound(format!("no MCP server `{id}`")))?;
    let cwd = self.start_directory(&server, None);
    let endpoint = self.resolve(&server, &cwd, providers)?;
    let opened = Connection::open(&endpoint, || {}).await;
    let mut catalog = Catalog {
      checked_at: Some(crate::session::statistics::Timestamp::now().0),
      ..Default::default()
    };
    let result = match opened {
      Ok(connection) => {
        catalog.server = connection.describe_server();
        let tools = connection.list_tools().await;
        let stderr = connection.get_stderr();
        connection.close().await;
        match tools {
          Ok(tools) => {
            catalog.tools = Some(tools.clone());
            Ok(json!({"server": catalog.server, "tools": tools, "stderr": stderr}))
          }
          Err(error) => {
            catalog.error = Some(error.to_string());
            Err(McpFailure::from(error))
          }
        }
      }
      Err(error) => {
        catalog.error = Some(error.to_string());
        Err(McpFailure::from(error))
      }
    };
    self.catalogs.lock().unwrap().insert(id.to_owned(), catalog);
    result
  }

  /// Every configured server, with what is known of it: for the settings page.
  pub fn describe(&self) -> Vec<Value> {
    // One lock at a time: `get_instance` takes the instances before the settings.
    let ids: Vec<String> = self.settings.read().unwrap().config.servers.keys().cloned().collect();
    let catalogs = self.catalogs.lock().unwrap().clone();
    let instances: Vec<_> = self
      .instances
      .lock()
      .unwrap()
      .iter()
      .map(|(key, instance)| (key.server.clone(), instance.clone()))
      .collect();
    ids
      .iter()
      .map(|id| {
        let catalog = catalogs.get(id).cloned().unwrap_or_default();
        let live: Vec<_> = instances
          .iter()
          .filter(|(server, instance)| {
            server == id && instance.connection.get().is_some_and(|c| !c.is_closed())
          })
          .map(|(_, instance)| instance.clone())
          .collect();
        let stderr = live
          .iter()
          .filter_map(|i| i.connection.get())
          .map(Connection::get_stderr)
          .find(|s| !s.is_empty());
        json!({
          "id": id,
          "instances": live.len(),
          "server": catalog.server,
          "tools": catalog.tools,
          "error": catalog.error,
          "stderr": stderr,
          "checked_at": catalog.checked_at,
        })
      })
      .collect()
  }

  /// Takes a new configuration. An instance whose server changed, went away, or would now be reached
  /// differently (another key, another proxy) is closed; the next call starts a new one.
  pub fn apply(
    &self,
    config: McpConfig,
    proxy: ProxyConfig,
    default_cwd: PathBuf,
    providers: &BTreeMap<String, Arc<Provider>>,
  ) {
    let previous = {
      let mut settings = self.settings.write().unwrap();
      let previous = std::mem::replace(&mut settings.config, config);
      settings.proxy = proxy;
      settings.default_cwd = default_cwd;
      previous
    };
    let current = self.settings.read().unwrap().config.clone();
    {
      let mut catalogs = self.catalogs.lock().unwrap();
      catalogs.retain(|id, _| {
        current.servers.get(id) == previous.servers.get(id) && current.servers.contains_key(id)
      });
    }
    let mut closing = Vec::new();
    self.instances.lock().unwrap().retain(|key, instance| {
      let keep =
        current.servers.get(&key.server).filter(|server| server.enabled).is_some_and(|server| {
          previous.servers.get(&key.server) == Some(server)
            && self.resolve(server, &instance.cwd, providers).is_ok_and(|e| e == instance.endpoint)
        });
      if !keep {
        closing.push(instance.clone());
      }
      keep
    });
    closing.into_iter().for_each(close_later);
  }

  /// Closes instances nobody has called for their server's idle time, and ones already closed.
  pub fn reap(&self) {
    let timeouts: HashMap<String, u64> = {
      let settings = self.settings.read().unwrap();
      settings.config.servers.iter().map(|(id, s)| (id.clone(), s.idle_timeout)).collect()
    };
    let mut closing = Vec::new();
    self.instances.lock().unwrap().retain(|key, instance| {
      let closed = instance.connection.get().is_some_and(Connection::is_closed);
      let idle = timeouts.get(&key.server).copied().unwrap_or(0);
      let expired = idle > 0
        && instance.active.load(Ordering::Acquire) == 0
        && instance.last_used.lock().unwrap().elapsed() >= Duration::from_secs(idle);
      if closed || expired {
        closing.push(instance.clone());
      }
      !(closed || expired)
    });
    closing.into_iter().for_each(close_later);
  }

  /// Closes a session's own instances, when it is deleted or switches MCP off.
  pub fn close_session(&self, session: &str) {
    let mut closing = Vec::new();
    self.instances.lock().unwrap().retain(|key, instance| {
      let mine = key.session.as_deref() == Some(session);
      if mine {
        closing.push(instance.clone());
      }
      !mine
    });
    closing.into_iter().for_each(close_later);
  }

  /// Closes every instance and waits for them to go.
  pub async fn close_all(&self) {
    let instances: Vec<_> = self.instances.lock().unwrap().drain().map(|(_, i)| i).collect();
    for instance in instances {
      close(instance).await;
    }
  }
}

/// Marks an instance in use for as long as a call through it runs.
struct Busy<'a>(&'a Instance);
impl Drop for Busy<'_> {
  fn drop(&mut self) {
    *self.0.last_used.lock().unwrap() = Instant::now();
    self.0.active.fetch_sub(1, Ordering::AcqRel);
  }
}

async fn close(instance: Arc<Instance>) {
  // A call still running holds its own reference; the connection goes when the last one does.
  if let Ok(instance) = Arc::try_unwrap(instance)
    && let Some(connection) = instance.connection.into_inner()
  {
    connection.close().await;
  }
}

fn close_later(instance: Arc<Instance>) {
  tokio::spawn(close(instance));
}

/// Saves a binary content block of a result - an image, audio, a resource's bytes - as a file, and
/// puts its path where the data was, so neither the bridge nor the output carries base64.
pub fn store_binary_content(result: &mut Value, directory: &Path) {
  let save = |item: &mut Map<String, Value>, field: &str| {
    let Some(data) = item.get(field).and_then(Value::as_str) else { return };
    use base64::Engine;
    let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(data) else { return };
    let mime = item.get("mimeType").and_then(Value::as_str).unwrap_or("");
    let extension = match mime {
      "image/png" => ".png",
      "image/jpeg" => ".jpg",
      "image/gif" => ".gif",
      "image/webp" => ".webp",
      "image/svg+xml" => ".svg",
      "audio/wav" | "audio/x-wav" => ".wav",
      "audio/mpeg" => ".mp3",
      "audio/ogg" => ".ogg",
      "application/pdf" => ".pdf",
      "application/json" => ".json",
      _ => "",
    };
    use sha2::Digest;
    let path = directory.join(format!("{:x}{extension}", sha2::Sha256::digest(&bytes)));
    if std::fs::create_dir_all(directory).and_then(|_| std::fs::write(&path, &bytes)).is_ok() {
      item.remove(field);
      item.insert("path".into(), json!(path));
    }
  };
  let Some(content) = result.get_mut("content").and_then(Value::as_array_mut) else { return };
  for block in content {
    let Some(item) = block.as_object_mut() else { continue };
    match item.get("type").and_then(Value::as_str) {
      Some("image" | "audio") => save(item, "data"),
      Some("resource") => {
        if let Some(resource) = item.get_mut("resource").and_then(Value::as_object_mut) {
          save(resource, "blob");
        }
      }
      _ => {}
    }
  }
}

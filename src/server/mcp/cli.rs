//! `wish mcp`: the command a session's shell reaches its MCP servers by.
//!
//! A client of the bridge the server offers each session. The address, the session and its token
//! come from the environment the session's shell runs in, so the command works only there.

use serde_json::{Map, Value, json};
use std::io::{IsTerminal, Read};

const USAGE: &str = "usage:
  wish mcp list [server]                  servers and their tools
  wish mcp describe <server>/<tool>       a tool's description and parameters
  wish mcp call [--json] <server>/<tool> [arguments]
                                          call a tool; arguments are a JSON object, read from
                                          standard input when left out; --json prints the whole
                                          result as the protocol gives it

exit status: 0 done, 1 the tool reported an error, 2 nothing was called, 3 the call's outcome is
unknown (it may have taken effect)";

type Failure = (i32, String);

/// Runs the command and returns its exit status.
pub async fn run(args: Vec<String>) -> i32 {
  match execute(args).await {
    Ok(status) => status,
    Err((status, message)) => {
      eprintln!("wish mcp: {message}");
      status
    }
  }
}

async fn execute(args: Vec<String>) -> Result<i32, Failure> {
  let mut args = args.into_iter();
  let command = args.next().unwrap_or_default();
  let rest: Vec<String> = args.collect();
  match (command.as_str(), rest.as_slice()) {
    ("list", []) => list(None).await,
    ("list", [server]) => list(Some(server)).await,
    ("describe", [target]) => describe(target).await,
    ("call", _) => call(rest).await,
    ("help" | "--help" | "-h" | "", _) => {
      println!("{USAGE}");
      Ok(0)
    }
    _ => Err((2, format!("unknown command\n{USAGE}"))),
  }
}

/// `server/tool`, split at the first slash: server names never hold one.
fn parse_target(target: &str) -> Result<(&str, &str), Failure> {
  target
    .split_once('/')
    .filter(|(server, tool)| !server.is_empty() && !tool.is_empty())
    .ok_or_else(|| (2, format!("expected <server>/<tool>, got `{target}`")))
}

struct Bridge {
  client: reqwest::Client,
  base: String,
  token: String,
}

impl Bridge {
  fn from_environment() -> Result<Self, Failure> {
    let read = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
    let (Some(url), Some(session), Some(token)) =
      (read("WISH_URL"), read("WISH_SESSION"), read("WISH_MCP_TOKEN"))
    else {
      return Err((
        2,
        "this command runs in the shell of a wish session with MCP switched on".into(),
      ));
    };
    // The bridge is on this machine: a proxy from the environment must not stand in between.
    let client = reqwest::Client::builder()
      .no_proxy()
      .build()
      .map_err(|error| (2, format!("could not set up a client: {error}")))?;
    Ok(Self { client, base: format!("{url}/sessions/{session}/mcp"), token })
  }

  fn url(&self, path: &str, query: &[(&str, &str)]) -> Result<reqwest::Url, Failure> {
    let mut url = reqwest::Url::parse(&format!("{}/{path}", self.base))
      .map_err(|error| (2, format!("WISH_URL is not a valid address: {error}")))?;
    if !query.is_empty() {
      url.query_pairs_mut().extend_pairs(query);
    }
    Ok(url)
  }

  async fn send(&self, request: reqwest::RequestBuilder) -> Result<Value, Failure> {
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

async fn list(server: Option<&str>) -> Result<i32, Failure> {
  let bridge = Bridge::from_environment()?;
  let query: Vec<(&str, &str)> = server.map(|server| ("server", server)).into_iter().collect();
  let servers = bridge.send(bridge.client.get(bridge.url("servers", &query)?)).await?;
  let servers = servers.as_array().cloned().unwrap_or_default();
  if servers.is_empty() {
    println!("No MCP servers are configured.");
    return Ok(0);
  }
  for (index, entry) in servers.iter().enumerate() {
    if index > 0 {
      println!();
    }
    let name = entry["server"].as_str().unwrap_or("?");
    if let Some(error) = entry["error"].as_str() {
      println!("{name}: unavailable: {error}");
      continue;
    }
    let tools = entry["tools"].as_array().cloned().unwrap_or_default();
    println!("{name} ({} tool{})", tools.len(), if tools.len() == 1 { "" } else { "s" });
    for tool in tools {
      let tool_name = tool["name"].as_str().unwrap_or("?");
      match tool["description"].as_str() {
        Some(summary) => println!("  {tool_name} - {summary}"),
        None => println!("  {tool_name}"),
      }
    }
  }
  Ok(0)
}

async fn describe(target: &str) -> Result<i32, Failure> {
  let (server, tool) = parse_target(target)?;
  let bridge = Bridge::from_environment()?;
  let definition = bridge
    .send(bridge.client.get(bridge.url("tool", &[("server", server), ("name", tool)])?))
    .await?;
  println!("{server}/{tool}");
  if let Some(title) = definition["title"].as_str() {
    println!("{title}");
  }
  if let Some(description) = definition["description"].as_str() {
    println!("\n{}", description.trim());
  }
  let pretty = |value: &Value| serde_json::to_string_pretty(value).unwrap_or_default();
  println!("\nParameters (JSON schema):\n{}", pretty(&definition["inputSchema"]));
  if !definition["outputSchema"].is_null() {
    println!("\nStructured result (JSON schema):\n{}", pretty(&definition["outputSchema"]));
  }
  if let Some(annotations) = definition["annotations"].as_object().filter(|a| !a.is_empty()) {
    println!("\nAnnotations: {}", Value::Object(annotations.clone()));
  }
  Ok(0)
}

async fn call(args: Vec<String>) -> Result<i32, Failure> {
  let raw = args.iter().any(|arg| arg == "--json");
  let positional: Vec<&String> = args.iter().filter(|arg| *arg != "--json").collect();
  let (target, text) = match positional.as_slice() {
    [target] => (target.as_str(), None),
    [target, arguments] => (target.as_str(), Some((*arguments).clone())),
    _ => return Err((2, format!("expected <server>/<tool> [arguments]\n{USAGE}"))),
  };
  let (server, tool) = parse_target(target)?;
  let text = match text {
    Some(text) => text,
    None if !std::io::stdin().is_terminal() => {
      let mut text = String::new();
      std::io::stdin()
        .read_to_string(&mut text)
        .map_err(|error| (2, format!("could not read the arguments: {error}")))?;
      text
    }
    None => String::new(),
  };
  let arguments: Map<String, Value> = if text.trim().is_empty() {
    Map::new()
  } else {
    match serde_json::from_str(&text) {
      Ok(Value::Object(arguments)) => arguments,
      Ok(_) => return Err((2, "the arguments must be a JSON object".into())),
      Err(error) => return Err((2, format!("the arguments are not valid JSON: {error}"))),
    }
  };
  let bridge = Bridge::from_environment()?;
  let result = bridge
    .send(
      bridge
        .client
        .post(bridge.url("call", &[])?)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(json!({"server": server, "tool": tool, "arguments": arguments}).to_string()),
    )
    .await?;
  if raw {
    println!("{}", serde_json::to_string_pretty(&result).unwrap_or_default());
  } else {
    print_result(&result);
  }
  Ok(if result["isError"] == true { 1 } else { 0 })
}

/// Prints a result the way a reader wants it: text as text, files as their paths, and structured
/// content as JSON when nothing else says it.
fn print_result(result: &Value) {
  let mut printed = false;
  for block in result["content"].as_array().into_iter().flatten() {
    printed = true;
    let mime = block["mimeType"].as_str().unwrap_or("");
    match block["type"].as_str() {
      Some("text") => println!("{}", block["text"].as_str().unwrap_or("")),
      Some(kind @ ("image" | "audio")) => match block["path"].as_str() {
        Some(path) => println!("[{kind} {mime} saved to {path}]"),
        None => println!("[{kind} {mime}]"),
      },
      Some("resource") => {
        let resource = &block["resource"];
        let uri = resource["uri"].as_str().unwrap_or("");
        match (resource["text"].as_str(), resource["path"].as_str()) {
          (Some(text), _) => println!("{text}"),
          (None, Some(path)) => println!("[resource {uri} saved to {path}]"),
          (None, None) => println!("[resource {uri}]"),
        }
      }
      Some("resource_link") => {
        let uri = block["uri"].as_str().unwrap_or("");
        match block["name"].as_str() {
          Some(name) => println!("[resource link {uri} ({name})]"),
          None => println!("[resource link {uri}]"),
        }
      }
      _ => println!("{block}"),
    }
  }
  if !printed && !result["structuredContent"].is_null() {
    println!("{}", serde_json::to_string_pretty(&result["structuredContent"]).unwrap_or_default());
  }
}

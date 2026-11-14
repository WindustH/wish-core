//! `wish session`: how a session's model sees, makes and messages other sessions, from
//! its shell.
//!
//! A client of the bridge, like `wish mcp`; the session it speaks for comes from the shell's
//! environment, so the command works only there.
use super::client::{BridgeClient, Failure};
use serde_json::{Map, Value, json};

const USAGE: &str = "usage:
  wish session list
  wish session show <session>
  wish session create <name> [--instructions TEXT] [--model MODEL] [--provider PROVIDER] [--cwd DIR] [--no-shell]
  wish session config <session> [--name NAME] [--model MODEL] [--provider PROVIDER] [--instructions TEXT]
  wish session delete <session>
  wish session send <session> <text>

Targets are session names, IDs, or $WISH_SESSION_ID_* aliases; _0 is yourself.
Examples:
  wish session show '$WISH_SESSION_ID_0'
  wish session send '$WISH_SESSION_ID_1' 'Please review the latest changes.'

send puts the message directly into the target's queue
and wakes it. Reply with send; ordinary conversation replies are not forwarded.
Messages live in session histories only, with no separate conversation object.
Only configure yourself or sessions you created; only delete sessions you created.
Commands use the identity supplied by the shell bridge. Help works outside a session.
Exit status: 0 done, 1 refused or not found, 2 invalid arguments.";

/// Runs the command and returns its exit status.
pub async fn run(args: Vec<String>) -> i32 {
  match execute(args).await {
    Ok(status) => status,
    Err((status, message)) => {
      eprintln!("wish session: {message}");
      status
    }
  }
}

async fn execute(args: Vec<String>) -> Result<i32, Failure> {
  let mut args = args.into_iter();
  let command = args.next().unwrap_or_default();
  let rest: Vec<String> = args.collect();
  match (command.as_str(), rest.as_slice()) {
    ("list", []) => list().await,
    ("show", [target]) => show(target).await,
    ("create", [name, options @ ..]) => create(name, options).await,
    ("config", [target, options @ ..]) if !options.is_empty() => config(target, options).await,
    ("delete", [target]) => delete(target).await,
    ("send", [target, rest @ ..]) if !rest.is_empty() => send(target, rest).await,
    ("help" | "--help" | "-h" | "", _) => {
      println!("{USAGE}");
      Ok(0)
    }
    _ => Err((2, format!("unknown command\n{USAGE}"))),
  }
}

/// `--key value` pairs, and what was left over. A key named in `flags` takes no value.
fn options(
  args: &[String],
  keys: &[&str],
  flags: &[&str],
) -> Result<(Map<String, Value>, Vec<String>), Failure> {
  let mut map = Map::new();
  let mut rest = Vec::new();
  let mut args = args.iter();
  while let Some(arg) = args.next() {
    let Some(key) = arg.strip_prefix("--") else {
      rest.push(arg.clone());
      continue;
    };
    if flags.contains(&key) {
      map.insert(key.to_owned(), Value::Bool(true));
    } else if keys.contains(&key) {
      let value = args.next().ok_or_else(|| (2, format!("--{key} needs a value")))?;
      map.insert(key.to_owned(), Value::String(value.clone()));
    } else {
      return Err((2, format!("unknown option --{key}\n{USAGE}")));
    }
  }
  Ok((map, rest))
}

/// A refusal is reported with status 1, since it is the model's request that was wrong.
fn refused(failure: Failure) -> Failure {
  match failure {
    (2, message) => (1, message),
    other => other,
  }
}

fn name_of(entry: &Value) -> &str {
  entry["name"].as_str().filter(|name| !name.is_empty()).unwrap_or("?")
}

async fn list() -> Result<i32, Failure> {
  let bridge = BridgeClient::from_environment("session")?;
  let peers = bridge.send(bridge.client.get(bridge.url("peers", &[])?)).await?;
  let me = peers["me"].as_str().unwrap_or_default();
  println!("Sessions:");
  for session in peers["sessions"].as_array().into_iter().flatten() {
    let id = session["id"].as_str().unwrap_or("?");
    let mut facts = vec![format!(
      "{}/{}",
      session["provider"].as_str().unwrap_or("?"),
      session["model"].as_str().unwrap_or("?")
    )];
    facts.push(session["phase"].as_str().unwrap_or("?").to_lowercase());
    if let Some(queued) = session["queue"].as_u64().filter(|count| *count > 0) {
      facts.push(format!("{queued} queued"));
    }
    if session["created_by"].as_str() == Some(me) {
      facts.push("made by you".into());
    }
    let me_mark = if id == me { " (you)" } else { "" };
    println!("  {}{me_mark} [{}] - {}", name_of(session), short(id), facts.join(", "));
  }
  println!("\n`wish session show <name>` tells more.");
  Ok(0)
}

fn short(id: &str) -> &str {
  &id[..id.len().min(8)]
}

async fn show(target: &str) -> Result<i32, Failure> {
  let bridge = BridgeClient::from_environment("session")?;
  let path = format!("peers/{}", encode(target));
  let peer = bridge.send(bridge.client.get(bridge.url(&path, &[])?)).await.map_err(refused)?;
  println!("Session {} [{}]", name_of(&peer), peer["id"].as_str().unwrap_or("?"));
  println!(
    "Model: {}/{}",
    peer["provider"].as_str().unwrap_or("?"),
    peer["model"].as_str().unwrap_or("?")
  );
  println!("Directory: {}", peer["cwd"].as_str().unwrap_or("?"));
  println!(
    "State: {}{}",
    peer["phase"].as_str().unwrap_or("?").to_lowercase(),
    match peer["queue"].as_u64().unwrap_or(0) {
      0 => String::new(),
      queued => format!(", {queued} message{} queued", if queued == 1 { "" } else { "s" }),
    }
  );
  if let Some(instructions) = peer["instructions"].as_str().filter(|text| !text.is_empty()) {
    println!("Instructions: {}", instructions.replace('\n', "\n  "));
  }
  if peer["created_by"].as_str() == Some(peer["me"].as_str().unwrap_or_default()) {
    println!("Made by you; `wish session config` and `wish session delete` may change it.");
  }
  Ok(0)
}

async fn create(name: &str, args: &[String]) -> Result<i32, Failure> {
  let (mut body, rest) =
    options(args, &["instructions", "model", "provider", "cwd"], &["no-shell"])?;
  if !rest.is_empty() {
    return Err((2, format!("unexpected arguments: {}", rest.join(" "))));
  }
  if body.remove("no-shell").is_some() {
    body.insert("shell".into(), Value::Bool(false));
  }
  body.insert("name".into(), Value::String(name.to_owned()));
  let bridge = BridgeClient::from_environment("session")?;
  let session = bridge
    .send(with_json(bridge.client.post(bridge.url("peers", &[])?), Value::Object(body)))
    .await
    .map_err(refused)?;
  println!(
    "Created session {} [{}] on {}/{}. `wish session send {}` messages it.",
    name_of(&session),
    session["id"].as_str().unwrap_or("?"),
    session["provider"].as_str().unwrap_or("?"),
    session["model"].as_str().unwrap_or("?"),
    name_of(&session)
  );
  Ok(0)
}

async fn config(target: &str, args: &[String]) -> Result<i32, Failure> {
  let (body, rest) = options(args, &["name", "model", "provider", "instructions"], &[])?;
  if !rest.is_empty() {
    return Err((2, format!("unexpected arguments: {}", rest.join(" "))));
  }
  let bridge = BridgeClient::from_environment("session")?;
  let path = format!("peers/{}", encode(target));
  let changed = bridge
    .send(with_json(bridge.client.patch(bridge.url(&path, &[])?), Value::Object(body)))
    .await
    .map_err(refused)?;
  let peer = &changed["session"];
  println!("Configured session {} [{}].", name_of(peer), peer["id"].as_str().unwrap_or("?"));
  Ok(0)
}

async fn delete(target: &str) -> Result<i32, Failure> {
  let bridge = BridgeClient::from_environment("session")?;
  let path = format!("peers/{}", encode(target));
  bridge.send(bridge.client.delete(bridge.url(&path, &[])?)).await.map_err(refused)?;
  println!("Deleted.");
  Ok(0)
}

async fn send(target: &str, args: &[String]) -> Result<i32, Failure> {
  let text = args.join(" ");
  if text.trim().is_empty() {
    return Err((2, "nothing to send".into()));
  }
  let bridge = BridgeClient::from_environment("session")?;
  let path = format!("peers/{}/send", encode(target));
  let posted = bridge
    .send(with_json(bridge.client.post(bridge.url(&path, &[])?), json!({"text": text})))
    .await
    .map_err(refused)?;
  println!(
    "Sent to session {} [{}].",
    name_of(&posted["to"]),
    posted["to"]["id"].as_str().unwrap_or("?")
  );
  Ok(0)
}

/// The request with `value` as its JSON body.
fn with_json(request: reqwest::RequestBuilder, value: Value) -> reqwest::RequestBuilder {
  request.header(reqwest::header::CONTENT_TYPE, "application/json").body(value.to_string())
}

/// A path segment, with what is not allowed there escaped.
fn encode(name: &str) -> String {
  let mut out = String::new();
  for byte in name.bytes() {
    match byte {
      b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(byte as char),
      _ => out.push_str(&format!("%{byte:02X}")),
    }
  }
  out
}

//! `wish session`: how a session's model sees, makes and messages other sessions and groups, from
//! its shell.
//!
//! A client of the bridge, like `wish mcp`; the session it speaks for comes from the shell's
//! environment, so the command works only there.
use crate::server::bridge::client::{BridgeClient, Failure};
use serde_json::{Map, Value, json};

const USAGE: &str = "usage:
  wish session list                              the sessions and groups
  wish session show <session|group>              one in detail; a group with its recent messages
  wish session create <name> [options]           a new session
      --instructions <text>   what it is for and how to work, given to it as instructions
      --model <model>  --provider <provider>  --cwd <dir>  --no-shell
  wish session config <session> [options]        change one you made, or yourself; a name or
                                                 model even while it runs
      --name <name>  --model <model>  --provider <provider>  --instructions <text>
  wish session delete <session>                  remove one you made
  wish session send <session|group> <text>       a message, which wakes the other members; to a
                                                 session, in a group of the two of you and the user
  wish session group create <name> <session>...  a group of you, them and the user
  wish session group add <group> <session>...    more members
  wish session group rename <group> <name>       a group you are in
  wish session group leave <group>

Sessions and groups are named by name or id. Exit status: 0 done, 1 refused or not found, 2
nothing was asked.";

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
    ("group", [action, rest @ ..]) => group(action, rest).await,
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
    (2, message) if message != "not found" => (1, message),
    (2, _) => (1, "no such session or group; `wish session list` shows them".into()),
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
  let groups = peers["groups"].as_array().cloned().unwrap_or_default();
  if !groups.is_empty() {
    println!("Groups:");
    for group in &groups {
      let members: Vec<&str> =
        group["members"].as_array().into_iter().flatten().map(name_of).collect();
      println!(
        "  {} [{}] - {}",
        name_of(group),
        short(group["id"].as_str().unwrap_or("?")),
        members.join(", ")
      );
    }
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
  if peer["kind"] == "group" {
    let members: Vec<&str> =
      peer["members"].as_array().into_iter().flatten().map(name_of).collect();
    println!(
      "Group {} [{}]\nMembers: the user, {}",
      name_of(&peer),
      peer["id"].as_str().unwrap_or("?"),
      members.join(", ")
    );
    let messages = peer["messages"].as_array().cloned().unwrap_or_default();
    if messages.is_empty() {
      println!("No messages yet.");
    } else {
      println!("Recent messages:");
      for message in &messages {
        let from = match message["author"]["kind"].as_str() {
          Some("user") => "the user".to_owned(),
          Some("session") => message["author"]["name"].as_str().unwrap_or("?").to_owned(),
          _ => "Wish".to_owned(),
        };
        println!(
          "  {from}: {}",
          message["text"].as_str().unwrap_or_default().replace('\n', "\n    ")
        );
      }
    }
    return Ok(0);
  }
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
  let groups: Vec<&str> = peer["groups"].as_array().into_iter().flatten().map(name_of).collect();
  if !groups.is_empty() {
    println!("Groups: {}", groups.join(", "));
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
  // A group comes back as `list` shows it; a session as the user's API does, under `session`.
  let (what, peer) = match changed["kind"].as_str() {
    Some("group") => ("Renamed group", &changed),
    _ => ("Configured session", &changed["session"]),
  };
  println!("{what} {} [{}].", name_of(peer), peer["id"].as_str().unwrap_or("?"));
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
  let woken: Vec<&str> = posted["woken"].as_array().into_iter().flatten().map(name_of).collect();
  let group = name_of(&posted["group"]);
  if woken.is_empty() {
    println!("Sent to group {group}, where no other session is.");
  } else {
    println!("Sent to group {group}; woke {}.", woken.join(", "));
  }
  Ok(0)
}

async fn group(action: &str, args: &[String]) -> Result<i32, Failure> {
  let bridge = BridgeClient::from_environment("session")?;
  match (action, args) {
    ("create", [name, members @ ..]) if !members.is_empty() => {
      let group = bridge
        .send(with_json(
          bridge.client.post(bridge.url("peers/groups", &[])?),
          json!({"name": name, "members": members}),
        ))
        .await
        .map_err(refused)?;
      let members: Vec<&str> =
        group["members"].as_array().into_iter().flatten().map(name_of).collect();
      println!(
        "Created group {} [{}] with the user, {}.",
        name_of(&group),
        group["id"].as_str().unwrap_or("?"),
        members.join(", ")
      );
      Ok(0)
    }
    ("add", [target, members @ ..]) if !members.is_empty() => {
      let path = format!("peers/{}/members", encode(target));
      let group = bridge
        .send(with_json(bridge.client.post(bridge.url(&path, &[])?), json!({"add": members})))
        .await
        .map_err(refused)?;
      let members: Vec<&str> =
        group["members"].as_array().into_iter().flatten().map(name_of).collect();
      println!("Group {} now has the user, {}.", name_of(&group), members.join(", "));
      Ok(0)
    }
    ("rename", [target, name]) => {
      let path = format!("peers/{}", encode(target));
      let group = bridge
        .send(with_json(bridge.client.patch(bridge.url(&path, &[])?), json!({"name": name})))
        .await
        .map_err(refused)?;
      println!("Renamed group {} [{}].", name_of(&group), group["id"].as_str().unwrap_or("?"));
      Ok(0)
    }
    ("leave", [target]) => {
      let path = format!("peers/{}/leave", encode(target));
      bridge.send(bridge.client.post(bridge.url(&path, &[])?)).await.map_err(refused)?;
      println!("Left.");
      Ok(0)
    }
    _ => Err((2, format!("unknown group command\n{USAGE}"))),
  }
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

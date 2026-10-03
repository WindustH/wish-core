//! `wish skill`: how a session's model finds and reads skills, from its shell.
//!
//! A client of the bridge, like `wish mcp`; the session it speaks for comes from the shell's
//! environment, so the command works only there.
use crate::server::bridge::client::{BridgeClient, Failure};
use serde_json::Value;

const USAGE: &str = "usage:
  wish skill find <words>    the skills that fit a task, best first
  wish skill list            every skill, by folder
  wish skill show <name>     a skill's instructions, and the files beside them

exit status: 0 done, 1 no skill matched or has that name, 2 nothing was asked";

/// Runs the command and returns its exit status.
pub async fn run(args: Vec<String>) -> i32 {
  match execute(args).await {
    Ok(status) => status,
    Err((status, message)) => {
      eprintln!("wish skill: {message}");
      status
    }
  }
}

async fn execute(args: Vec<String>) -> Result<i32, Failure> {
  let mut args = args.into_iter();
  let command = args.next().unwrap_or_default();
  let rest: Vec<String> = args.collect();
  match (command.as_str(), rest.as_slice()) {
    ("find", [_, ..]) => find(&rest.join(" ")).await,
    ("list", []) => list().await,
    ("show", [name]) => show(name).await,
    ("help" | "--help" | "-h" | "", _) => {
      println!("{USAGE}");
      Ok(0)
    }
    _ => Err((2, format!("unknown command\n{USAGE}"))),
  }
}

async fn skills(query: Option<&str>) -> Result<Vec<Value>, Failure> {
  let bridge = BridgeClient::from_environment("skill")?;
  let query: Vec<(&str, &str)> = query.map(|query| ("query", query)).into_iter().collect();
  let answer = bridge.send(bridge.client.get(bridge.url("skills", &query)?)).await?;
  Ok(answer["skills"].as_array().cloned().unwrap_or_default())
}

fn line(skill: &Value) -> String {
  let name = skill["name"].as_str().unwrap_or("?");
  match skill["description"].as_str().map(str::trim).filter(|text| !text.is_empty()) {
    Some(description) => {
      format!("{name} - {}", description.split_whitespace().collect::<Vec<_>>().join(" "))
    }
    None => name.to_owned(),
  }
}

async fn find(query: &str) -> Result<i32, Failure> {
  let found = skills(Some(query)).await?;
  if found.is_empty() {
    println!("No skill fits \"{query}\". `wish skill list` shows them all.");
    return Ok(1);
  }
  for skill in &found {
    println!("{}", line(skill));
  }
  println!("\n`wish skill show <name>` reads one.");
  Ok(0)
}

async fn list() -> Result<i32, Failure> {
  let all = skills(None).await?;
  if all.is_empty() {
    println!("No skills are installed.");
    return Ok(0);
  }
  // Skills at the top of their folders first, then each folder's under its name.
  let folder = |skill: &Value| skill["category"].as_str().unwrap_or_default().to_owned();
  let mut folders: Vec<String> = all.iter().map(folder).collect();
  folders.sort();
  folders.dedup();
  for name in folders {
    let indent = if name.is_empty() { "" } else { "  " };
    if !name.is_empty() {
      println!("{name}/");
    }
    for skill in all.iter().filter(|skill| folder(skill) == name) {
      println!("{indent}{}", line(skill));
    }
  }
  println!(
    "\n{} skill{}. `wish skill show <name>` reads one.",
    all.len(),
    if all.len() == 1 { "" } else { "s" }
  );
  Ok(0)
}

async fn show(name: &str) -> Result<i32, Failure> {
  let bridge = BridgeClient::from_environment("skill")?;
  let path = format!("skills/{}", encode(name));
  let skill = match bridge.send(bridge.client.get(bridge.url(&path, &[])?)).await {
    Ok(skill) => skill,
    Err((2, message)) if message == "not found" => {
      println!("No skill is named \"{name}\". `wish skill find <words>` looks for one.");
      return Ok(1);
    }
    Err(failure) => return Err(failure),
  };
  println!("{}", line(&skill));
  println!("Directory: {}", skill["dir"].as_str().unwrap_or_default());
  let files: Vec<&str> =
    skill["files"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
  if !files.is_empty() {
    let more = skill["more_files"].as_u64().unwrap_or(0);
    let more = if more > 0 { format!(", and {more} more") } else { String::new() };
    println!("Files: {}{more}", files.join(", "));
  }
  println!("Paths in the instructions are relative to the directory.\n");
  println!("{}", skill["body"].as_str().unwrap_or_default());
  Ok(0)
}

/// A name as one segment of a URL path.
fn encode(name: &str) -> String {
  name
    .bytes()
    .map(|byte| match byte {
      b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
        (byte as char).to_string()
      }
      _ => format!("%{byte:02X}"),
    })
    .collect()
}

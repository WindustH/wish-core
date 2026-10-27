//! `wish task`: a session-authenticated client of task operations.
use super::client::{BridgeClient, Failure};
use serde_json::{Value, json};
const USAGE: &str = include_str!("help/task.txt");

pub async fn run(args: Vec<String>) -> i32 {
  match execute(args).await {
    Ok(code) => code,
    Err((code, message)) => {
      eprintln!("wish task: {message}");
      code
    }
  }
}
fn invalid(message: impl Into<String>) -> Failure {
  (2, message.into())
}
fn task_id(id: &str) -> Result<String, Failure> {
  if id == "self" {
    std::env::var("WISH_TASK_ID").map_err(|_| invalid("no current task"))
  } else {
    Ok(id.into())
  }
}
fn print(value: &Value) {
  println!("{}", serde_json::to_string_pretty(value).unwrap());
}
async fn send(bridge: &BridgeClient, request: Value) -> Result<Value, Failure> {
  bridge
    .send(
      bridge
        .client
        .post(bridge.url("task", &[])?)
        .header("Content-Type", "application/json")
        .body(request.to_string()),
    )
    .await
}
async fn execute(mut args: Vec<String>) -> Result<i32, Failure> {
  let structured = args.first().is_some_and(|arg| arg == "--output-json");
  if structured {
    args.remove(0);
  }
  let display = |value: &Value| {
    if structured {
      print(value);
    } else {
      println!("{}", crate::tool::task_output::render(value, false));
    }
  };
  let Some(command) = args.first().map(String::as_str) else {
    println!("{USAGE}");
    return Ok(0);
  };
  if matches!(command, "help" | "--help" | "-h") {
    println!("{USAGE}");
    return Ok(0);
  }
  let bridge = BridgeClient::from_environment("task")?;
  let mut rest = args[1..].iter();
  if command == "list" {
    if rest.clone().any(|arg| !matches!(arg.as_str(), "--running" | "--json")) {
      return Err(invalid(USAGE));
    }
    let mut result = send(&bridge, json!({"action":"list"})).await?;
    if args.iter().any(|arg| arg == "--running")
      && let Some(tasks) = result["tasks"].as_array_mut()
    {
      tasks.retain(|task| task["process"]["status"] == "running");
    }
    if structured || args.iter().any(|arg| arg == "--json") {
      print(&result);
    } else {
      for task in result["tasks"].as_array().into_iter().flatten() {
        println!(
          "{}\t{}\t{}\t{}",
          task["task_id"].as_str().unwrap_or("?"),
          task["process"]["status"].as_str().unwrap_or("?"),
          if task["background"] == true { "background" } else { "foreground" },
          task["command"].as_str().unwrap_or_default().replace('\n', " ")
        );
      }
    }
    return Ok(0);
  }
  if matches!(command, "show" | "background" | "bg" | "wait" | "remove") {
    let id = task_id(rest.next().ok_or_else(|| invalid("task id is required"))?)?;
    if rest.next().is_some() {
      return Err(invalid(USAGE));
    }
    if command == "wait" {
      if std::env::var("WISH_TASK_ID").ok().as_deref() == Some(&id) {
        return Err(invalid("a task cannot wait for itself"));
      }
      loop {
        let result=send(&bridge,json!({"action":"execute","operation":{"operation":"poll","execution_id":id,"max_bytes":0,"wait_ms":1000}})).await?;
        if result["process"]["status"] != "running" {
          let success = result["process"]["exit_code"] == 0;
          display(&result);
          return Ok(if success { 0 } else { 1 });
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
      }
    }
    display(
      &send(&bridge, json!({"action":if command=="bg"{"background"}else{command},"task_id":id}))
        .await?,
    );
    return Ok(0);
  }
  let mut operation = json!({"operation":command});
  match command {
    "start" | "run" => {
      operation["operation"] = "start".into();
      operation["timeout"] = 0.into();
      let mut script = None;
      while let Some(arg) = rest.next() {
        match arg.as_str() {
          "--" => {
            script = Some(rest.by_ref().map(String::as_str).collect::<Vec<_>>().join(" "));
            break;
          }
          "--stdin" => {
            let mut data = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut data)
              .map_err(|error| invalid(error.to_string()))?;
            operation["data"] = data.into();
          }
          "--interactive" => operation["interactive"] = true.into(),
          "--data" | "--encoding" => {
            operation[&arg[2..]] =
              rest.next().ok_or_else(|| invalid("option requires a value"))?.clone().into();
          }
          "--wait" => {
            operation["timeout"] =
              serde_json::from_str(rest.next().ok_or_else(|| invalid("--wait needs seconds"))?)
                .map_err(|_| invalid("invalid wait"))?;
          }
          _ => return Err(invalid(format!("unknown option {arg}; place the command after --"))),
        }
      }
      let script = script
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| invalid("provide a command after --"))?;
      operation["command"] = script.into();
    }
    "poll" | "write" | "kill" => {
      operation["execution_id"] =
        task_id(rest.next().ok_or_else(|| invalid("task id is required"))?)?.into();
      if command == "write" {
        operation["data"] = "".into();
      }
      if command == "poll" && !structured {
        operation["max_bytes"] = (32 * 1024).into();
      }
      let mut data_seen = false;
      while let Some(arg) = rest.next() {
        match arg.as_str() {
          "--stdin" if command == "write" => {
            let mut data = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut data)
              .map_err(|error| invalid(error.to_string()))?;
            operation["data"] = data.into();
            data_seen = true;
          }
          "--force" if command == "kill" => operation["mode"] = "force".into(),
          "--close" if command == "write" => operation["close"] = true.into(),
          "--encoding" if command != "kill" => {
            operation["encoding"] =
              rest.next().ok_or_else(|| invalid("encoding is required"))?.clone().into()
          }
          "--offset" | "--max-bytes" | "--wait-ms" if command == "poll" => {
            let name = arg[2..].replace('-', "_");
            let value = rest
              .next()
              .ok_or_else(|| invalid("option value is required"))?
              .parse::<u64>()
              .map_err(|_| invalid("expected a non-negative integer"))?;
            operation[&name] = value.into();
          }
          "--" if command == "write" => {
            operation["data"] =
              rest.by_ref().map(String::as_str).collect::<Vec<_>>().join(" ").into();
            break;
          }
          _ if command == "write" && !arg.starts_with("--") && !data_seen => {
            operation["data"] = arg.clone().into();
            data_seen = true;
          }
          _ => return Err(invalid(format!("unknown option {arg}"))),
        }
      }
    }
    _ => return Err(invalid(USAGE)),
  }
  display(&send(&bridge, json!({"action":"execute","operation":operation})).await?);
  Ok(0)
}

//! Small command-line adapters over the session capability bridge.
use super::client::{BridgeClient, Failure};
use serde_json::{Value, json};
pub async fn run(name: &str, args: Vec<String>) -> i32 {
  match execute(name, args).await {
    Ok(()) => 0,
    Err((code, message)) => {
      eprintln!("wish {name}: {message}");
      code
    }
  }
}
async fn execute(name: &str, mut args: Vec<String>) -> Result<(), Failure> {
  let structured = args.first().is_some_and(|arg| arg == "--output-json");
  if structured {
    args.remove(0);
  }
  if args.is_empty() || args.iter().any(|arg| matches!(arg.as_str(), "--help" | "-h")) {
    println!(
      "{}",
      if name.starts_with("history_") {
        include_str!("help/history.txt")
      } else if crate::extensions::catalog::is_tool(name) {
        "Use wish extension search, wish extension inspect TOOL, or wish extension call TOOL --json '<arguments>'."
      } else {
        include_str!("help/askuser.txt")
      }
    );
    return Ok(());
  }
  let bridge = BridgeClient::from_environment(name)?;
  let url = bridge.url(&format!("commands/{name}"), &[])?;
  let mut requested_arguments = Value::Null;
  let output = if args == ["--schema"] {
    bridge.send(bridge.client.get(url)).await?
  } else {
    let arguments: Value = match args.first().map(String::as_str) {
      Some("--stdin") if args.len() == 1 => {
        let mut text = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut text)
          .map_err(|e| (2, e.to_string()))?;
        serde_json::from_str(&text).map_err(|e| (2, e.to_string()))?
      }
      Some("--json") if args.len() == 2 => {
        serde_json::from_str(&args[1]).map_err(|e| (2, e.to_string()))?
      }
      Some(value) if !value.starts_with("--") => match name {
        "history_search" => json!({"text":args.join(" ")}),
        "history_read" if args.len() == 1 => {
          json!({"sequence":value.parse::<u64>().map_err(|_| (2, "sequence must be a non-negative integer; use wish history --help".into()))?})
        }
        "askuser" if args.len() == 1 => {
          serde_json::from_str(value).map_err(|e| (2, e.to_string()))?
        }
        _ => return Err((2, "use --json for additional options".into())),
      },
      _ => return Err((2, "invalid arguments; use --help".into())),
    };
    requested_arguments = arguments.clone();
    bridge
      .send(
        bridge
          .client
          .post(url)
          .header("Content-Type", "application/json")
          .body(arguments.to_string()),
      )
      .await?
  };
  if name.starts_with("history_") && output.get("items").is_some() && !structured {
    println!("{}", crate::tool::history::render(&output, &requested_arguments));
  } else if name == "askuser" && output.get("status").is_some() && !structured {
    println!("{}", crate::tool::ask_user::render(&output));
  } else if let Some(text) = output.as_str().filter(|_| !structured) {
    println!("{text}");
  } else {
    println!("{}", serde_json::to_string_pretty(&output).unwrap());
  }
  Ok(())
}

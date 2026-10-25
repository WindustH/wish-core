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
async fn execute(name: &str, args: Vec<String>) -> Result<(), Failure> {
  if args.is_empty() || args.iter().any(|arg| matches!(arg.as_str(), "--help" | "-h")) {
    println!(
      "{}",
      match name {
        "webfetch" => include_str!("help/webfetch.txt"),
        "websearch" => include_str!("help/websearch.txt"),
        _ => include_str!("help/askuser.txt"),
      }
    );
    return Ok(());
  }
  let bridge = BridgeClient::from_environment(name)?;
  let url = bridge.url(&format!("commands/{name}"), &[])?;
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
        "webfetch" if args.len() == 1 => json!({"url":value}),
        "websearch" => json!({"query":args.join(" ")}),
        "askuser" if args.len() == 1 => {
          serde_json::from_str(value).map_err(|e| (2, e.to_string()))?
        }
        _ => return Err((2, "use --json for additional options".into())),
      },
      _ => return Err((2, "invalid arguments; use --help".into())),
    };
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
  if let Some(text) = output.as_str() {
    println!("{text}");
  } else {
    println!("{}", serde_json::to_string_pretty(&output).unwrap());
  }
  Ok(())
}

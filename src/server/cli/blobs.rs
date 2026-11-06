//! Positional convenience syntax over the same schema/executor as the blob tool.
use serde_json::{Value, json};
const HELP: &str = include_str!("help/blob.txt");
pub async fn run(args: Vec<String>) -> i32 {
  if args.is_empty() || args.iter().any(|arg| matches!(arg.as_str(), "--help" | "-h" | "help")) {
    println!("{HELP}");
    return 0;
  }
  if args[0].starts_with("--") {
    return super::tool_commands::run("blob", args).await;
  }
  match arguments(&args) {
    Ok(value) => super::tool_commands::run("blob", vec!["--json".into(), value.to_string()]).await,
    Err(error) => {
      eprintln!("wish blob: {error}\n{HELP}");
      2
    }
  }
}
fn arguments(args: &[String]) -> Result<Value, &'static str> {
  let action = args[0].as_str();
  let mut value = json!({"action":action});
  let mut rest = args[1..].iter();
  if action != "list" {
    let source = rest.next().ok_or("file path or blob ID is required")?;
    match action {
      "add" | "link" | "symlink" => value["path"] = source.clone().into(),
      "mention" => value["source"] = source.clone().into(),
      "delete" | "update" => value["id"] = source.clone().into(),
      "show" | "hide" | "pin" | "unpin" => {
        value = json!({"action":"update","id":source});
        match action {
          "show" | "hide" => value["visible"] = (action == "show").into(),
          _ => value["pinned"] = (action == "pin").into(),
        }
      }
      _ => return Err("unknown action"),
    }
  }
  while let Some(option) = rest.next() {
    match option.as_str() {
      "--all" => value["all"] = true.into(),
      "--displayed" => value["all"] = false.into(),
      "--visible" => value["visible"] = true.into(),
      "--hidden" => value["visible"] = false.into(),
      "--pinned" => value["pinned"] = true.into(),
      "--unpinned" => value["pinned"] = false.into(),
      "--page" | "--start" | "--limit" => {
        value[&option[2..]] =
          rest.next().ok_or("missing number")?.parse::<u64>().map_err(|_| "invalid integer")?.into()
      }
      "--time" => {
        let time =
          rest.next().ok_or("missing seconds")?.parse::<f64>().map_err(|_| "invalid seconds")?;
        if !time.is_finite() || time < 0. {
          return Err("time must be finite non-negative seconds");
        }
        value["time"] = time.into();
      }
      "--label" | "--name" => {
        value[&option[2..]] = rest.next().ok_or("missing label/name")?.clone().into()
      }
      _ => return Err("unknown option"),
    }
  }
  Ok(value)
}

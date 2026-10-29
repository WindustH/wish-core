//! One CLI entry for extension discovery, inspection and invocation.
use serde_json::json;
const HELP: &str = "wish extension search [TAG ...]\n  Find enabled tools; all supplied tags must match.\nwish extension inspect TOOL\n  Show the tool's schema, examples and available services.\nwish extension call TOOL --json '<arguments>'\nwish extension call TOOL --stdin\n  Invoke the selected extension tool. JSON arguments belong to that tool.\n\nThese commands run inside a Wish session shell and obey its permissions.\nInspect before calling. Select a configured service with service; omit it for the default priority.";
pub async fn run(mut args: Vec<String>) -> i32 {
  if args.is_empty() || args.iter().any(|arg| matches!(arg.as_str(), "--help" | "-h")) {
    println!("{HELP}");
    return 0;
  }
  let operation = args.remove(0);
  let (tool, arguments) = match operation.as_str() {
    "search" if args.iter().all(|arg| !arg.starts_with('-')) => (
      "extension_tools".to_owned(),
      vec!["--json".into(), json!({"action":"search","tags":args}).to_string()],
    ),
    "inspect" if args.len() == 1 => (
      "extension_tools".to_owned(),
      vec!["--json".into(), json!({"action":"inspect","tool":args[0]}).to_string()],
    ),
    "call" if !args.is_empty() => {
      let tool = args.remove(0);
      if crate::extensions::catalog::find(&tool).is_none()
        || !(args == ["--stdin"] || (args.len() == 2 && args[0] == "--json"))
      {
        eprintln!(
          "wish extension: use call TOOL --json '<arguments>' or --stdin; inspect the tool first"
        );
        return 2;
      }
      (tool, args)
    }
    _ => {
      eprintln!("wish extension: invalid arguments\n{HELP}");
      return 2;
    }
  };
  let mut forwarded = vec!["--output-json".into()];
  forwarded.extend(arguments);
  super::tool_commands::run(&tool, forwarded).await
}

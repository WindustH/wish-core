//! History CLI grammar; execution uses the existing session tool bridge.
pub async fn run(mut args: Vec<String>) -> i32 {
  if args.is_empty() || matches!(args[0].as_str(), "--help" | "-h") {
    println!("{}", include_str!("help/history.txt"));
    return 0;
  }
  let operation = args.remove(0);
  let name = match operation.as_str() {
    "search" => "history_search",
    "read" => "history_read",
    "query" => "history_query",
    _ => {
      eprintln!("wish history: expected search, read or query; use --help");
      return 2;
    }
  };
  if operation == "query" && args.is_empty() {
    args = vec!["--json".into(), "{}".into()];
  }
  super::tool_commands::run(name, args).await
}

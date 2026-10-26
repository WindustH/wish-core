//! `history_search`, `history_read` and `history_query`: the current session's permanent history,
//! sealed generations included, searched, read around a record and queried.
use crate::{
  executor::{ExecutionControl, tool::ToolExecutor},
  protocol::Tool,
  session::{
    Session, SessionReader, ToolCall, ToolOutcome,
    history::query::{
      HistoryCursor, HistoryFilter, HistoryKind, HistoryOrder, HistoryPageRequest, HistorySearch,
      HistorySearchMode,
    },
  },
};
use serde::Deserialize;
use serde_json::{Value, json};

/// The three history tools, bound to one session's history.
#[derive(Clone)]
pub struct HistoryTools {
  reader: SessionReader,
}

enum Operation {
  Search { query: HistorySearch, filter: HistoryFilter },
  Query { page: HistoryPageRequest, filter: HistoryFilter },
  Read(ReadArgs),
}
fn message_filter() -> HistoryFilter {
  HistoryFilter { kind: Some(HistoryKind::Message), ..Default::default() }
}

/// `history_search`'s arguments. `query` is a legacy form outside the schema, still read.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchArgs {
  #[serde(default)]
  text: Option<String>,
  #[serde(default)]
  mode: Option<HistorySearchMode>,
  #[serde(default)]
  limit: Option<usize>,
  #[serde(default)]
  query: Option<HistorySearch>,
  #[serde(default = "message_filter")]
  filter: HistoryFilter,
}
impl SearchArgs {
  fn into_operation(self) -> Result<Operation, String> {
    if let Some(query) = self.query {
      return Ok(Operation::Search { query, filter: self.filter });
    }
    let text = self.text.ok_or_else(|| "missing required field `text`".to_string())?;
    let mode = self.mode.unwrap_or(HistorySearchMode::Terms);
    let limit = self.limit.unwrap_or(20);
    Ok(Operation::Search { query: HistorySearch { text, mode, limit }, filter: self.filter })
  }
}

/// `history_read`'s arguments.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadArgs {
  sequence: u64,
  #[serde(default)]
  before: usize,
  #[serde(default)]
  after: usize,
}

/// `history_query`'s arguments. `page` is a legacy form outside the schema, still read.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct QueryArgs {
  #[serde(default)]
  page: Option<HistoryPageRequest>,
  #[serde(default)]
  limit: Option<usize>,
  #[serde(default)]
  order: Option<HistoryOrder>,
  #[serde(default)]
  cursor: Option<HistoryCursor>,
  #[serde(default = "message_filter")]
  filter: HistoryFilter,
}
impl QueryArgs {
  fn into_operation(self) -> Operation {
    let page = if let Some(page) = self.page {
      page
    } else {
      HistoryPageRequest {
        limit: self.limit.unwrap_or(50),
        order: self.order.unwrap_or_default(),
        cursor: self.cursor,
      }
    };
    Operation::Query { page, filter: self.filter }
  }
}

fn filter_schema() -> Value {
  json!({
    "type":"object","additionalProperties":false,"description":"All provided conditions are combined with AND. Omitted filter defaults to messages; kind=event selects events.","properties":{
      "kind":{"type":"string","enum":["message","event"]},
      "message_types":{"type":"array","items":{"type":"string","enum":["system","developer","user","assistant","reasoning","tool_use","tool_result","upstream_compaction"]}},
      "event_types":{"type":"array","items":{"type":"string"},"description":"SessionEvent names, e.g. Finished or ToolFinished."},
      "origins":{"type":"array","items":{"type":"string","enum":["Imported","Input","Model","Tool","Interrupted","Context","Summary"]}},
      "since":{"type":"integer","minimum":0,"description":"Inclusive history timestamp in Unix milliseconds."},
      "until":{"type":"integer","minimum":0,"description":"Exclusive history timestamp in Unix milliseconds."},
      "generation":{"type":"integer","minimum":0,"description":"Generation at the time the history fact was recorded."},
      "model_call_id":{"type":"integer","minimum":0},"tool_name":{"type":"string"},
      "metadata":{"type":"array","items":{"type":"object","required":["operation","path"],"properties":{
        "operation":{"type":"string","enum":["equals","exists"]},"path":{"type":"string","description":"SQLite JSON path, e.g. $.project_id."},"value":{"description":"Scalar JSON value for equals."}}}}
    }
  })
}

impl HistoryTools {
  /// The binding is fixed at construction; tool arguments cannot select another session.
  pub fn new(session: &Session) -> Self {
    Self { reader: session.reader().clone() }
  }

  pub fn get_specifications(&self) -> Vec<Tool> {
    vec![
      Tool {
        name: "history_search".into(),
        description: "Search this session's permanent history using keyword or substring full-text search. Returns matching sequences, snippets and relevance scores. Use history_read on a sequence to retrieve its original message or nearby events.".into(),
        input_schema: json!({
          "type": "object", "additionalProperties": false,
          "required": ["text"],
          "properties": {
            "text": {"type": "string", "description": "Search text. terms requires all whitespace-separated terms; substring matches contiguous text, useful for Chinese and paths."},
            "mode": {"type": "string", "enum": ["terms", "substring"], "default": "terms"},
            "limit": {"type": "integer", "minimum": 1, "default": 20},
            "filter": filter_schema()
          }
        }),
      },
      Tool {
        name: "history_read".into(),
        description: "Read the original message or event at a specific history sequence, optionally including nearby records for context.".into(),
        input_schema: json!({
          "type": "object", "additionalProperties": false,
          "required": ["sequence"],
          "properties": {
            "sequence": {"type": "integer", "minimum": 0, "description": "History sequence number to read."},
            "before": {"type": "integer", "minimum": 0, "default": 0, "description": "Number of preceding history records to include."},
            "after": {"type": "integer", "minimum": 0, "default": 0, "description": "Number of following history records to include."}
          }
        }),
      },
      Tool {
        name: "history_query".into(),
        description: "Query and filter this session's history records in order, with cursor-based pagination. Does not perform full-text scoring.".into(),
        input_schema: json!({
          "type": "object", "additionalProperties": false,
          "properties": {
            "filter": filter_schema(),
            "limit": {"type": "integer", "minimum": 1, "default": 50},
            "order": {"type": "string", "enum": ["oldest_first", "newest_first"], "default": "oldest_first"},
            "cursor": {
              "type": "object",
              "description": "Pass next from the previous query unchanged, retaining its filter and order.",
              "required": ["end_sequence", "after_sequence", "order"],
              "properties": {
                "end_sequence": {"type": "integer", "minimum": 0},
                "after_sequence": {"type": "integer", "minimum": 0},
                "order": {"type": "string", "enum": ["oldest_first", "newest_first"]}
              }
            }
          }
        }),
      },
    ]
  }

  fn run(&self, operation: Operation) -> Result<Value, String> {
    match operation {
      Operation::Search { query, filter } => {
        self.reader.search_history(query, filter).map(|result| json!(result))
      }
      Operation::Query { page, filter } => {
        self.reader.query_history(filter, page).map(|result| json!(result))
      }
      Operation::Read(ReadArgs { sequence, before, after }) => {
        self.reader.read_history_around(sequence, before, after).map(|items| json!({"items":items}))
      }
    }
    .map_err(|error| error.to_string())
  }

  /// The operation a call asks for, or why its arguments do not fit.
  fn parse_operation(call: &ToolCall) -> Result<Operation, String> {
    match call.name.as_str() {
      "history_search" => call.parse_arguments::<SearchArgs>()?.into_operation(),
      "history_read" => call.parse_arguments().map(Operation::Read),
      "history_query" => call.parse_arguments::<QueryArgs>().map(QueryArgs::into_operation),
      _ => Err(format!("unknown tool: {}", call.name)),
    }
  }
}
impl ToolExecutor for HistoryTools {
  async fn execute(&self, call: &ToolCall, control: &ExecutionControl) -> ToolOutcome {
    if control.is_cancelled() {
      return ToolOutcome::Cancelled;
    }
    let mut normalized = call.clone();
    if let Some(specification) =
      self.get_specifications().into_iter().find(|tool| tool.name == call.name)
    {
      normalized.arguments =
        match super::arguments::normalize(&call.arguments, &specification.input_schema) {
          Ok(arguments) => arguments,
          Err(error) => return ToolOutcome::Failed(error),
        };
    }
    let operation = match Self::parse_operation(&normalized) {
      Ok(operation) => operation,
      Err(error) => {
        return ToolOutcome::Failed(super::arguments::error(
          &call.name,
          error,
          match call.name.as_str() {
            "history_search" => r#"{"text":"keyword"}"#,
            "history_read" => r#"{"sequence":0}"#,
            _ => {
              "{} (omit cursor to restart; otherwise copy the previous result's next cursor exactly)"
            }
          },
        ));
      }
    };
    let tool = self.clone();
    // SQLite dispatch is synchronous; keep it off the executor's async thread. Await completion
    // even after cancellation, so a started read is not left running behind its result.
    let result = tokio::task::spawn_blocking(move || tool.run(operation)).await;
    if control.is_cancelled() {
      return ToolOutcome::Cancelled;
    }
    match result {
      Ok(Ok(result)) => ToolOutcome::Success(result),
      Ok(Err(error)) => ToolOutcome::Failed(error),
      Err(error) => ToolOutcome::Failed(error.to_string()),
    }
  }
}

/// Readable CLI output: keep record addresses and continuation without JSON-escaping prose.
pub fn render(output: &Value, arguments: &Value) -> String {
  fn content(value: &Value, indent: usize, lines: &mut Vec<String>) {
    let pad = "  ".repeat(indent);
    match value {
      Value::Object(fields) => {
        for (key, value) in fields {
          if value.is_null() {
            continue;
          }
          if key == "data_base64" {
            lines.push(format!("{pad}{key}: [image bytes; use view_image on the attachment file]"));
          } else if let Some(text) = value.as_str() {
            lines.push(format!("{pad}{key}: {text}"));
          } else {
            lines.push(format!("{pad}{key}:"));
            content(value, indent + 1, lines);
          }
        }
      }
      Value::Array(items) => {
        for item in items {
          content(item, indent, lines);
        }
      }
      Value::String(text) => lines.push(format!("{pad}{text}")),
      value => lines.push(format!("{pad}{value}")),
    }
  }
  let mut lines = Vec::new();
  if let Some(items) = output["items"].as_array() {
    for item in items {
      let kind = item["message_type"].as_str().or(item["event_type"].as_str()).unwrap_or("record");
      lines.push(format!("#{} {kind}", item["record"]["sequence"]));
      if let Some(snippet) = item["snippet"].as_str() {
        lines.push(snippet.to_owned());
      }
      if let Some(value) = item.get("content") {
        content(value, 0, &mut lines);
      }
      lines.push(String::new());
    }
    if items.is_empty() {
      lines.push("No matching history records.".into());
    }
  }
  if let Some(cursor) = output.get("next").filter(|value| !value.is_null()) {
    let mut next = arguments.clone();
    if !next.is_object() {
      next = json!({});
    }
    if next["page"].is_object() {
      next["page"]["cursor"] = cursor.clone();
    } else {
      next["cursor"] = cursor.clone();
    }
    // JSON can contain apostrophes in user filters; quote the complete argument for POSIX shells.
    let quoted = next.to_string().replace('\'', "'\"'\"'");
    lines.push(format!("Continue: wish history query --json '{quoted}'"));
  }
  if output["has_more"] == true {
    lines.push("More matches exist; narrow the search or increase limit.".into());
  }
  lines.join("\n")
}

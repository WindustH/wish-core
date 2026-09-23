//! Search and expand the current session's permanent history, including sealed generations.
use crate::{
  executor::{
    ExecutionControl,
    tool::{ToolCall, ToolExecutor, ToolOutcome},
  },
  protocol::Tool,
  session::{
    HistoryReader, Session,
    history::query::{
      HistoryCursor, HistoryFilter, HistoryKind, HistoryOrder, HistoryPageRequest, HistorySearch,
      HistorySearchMode,
    },
  },
};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Clone)]
pub struct SearchHistoryTool {
  reader: HistoryReader,
}
#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum Operation {
  Search {
    query: HistorySearch,
    #[serde(default = "message_filter")]
    filter: HistoryFilter,
  },
  Query {
    #[serde(default)]
    page: HistoryPageRequest,
    #[serde(default = "message_filter")]
    filter: HistoryFilter,
  },
  Read {
    sequence: u64,
    #[serde(default)]
    before: usize,
    #[serde(default)]
    after: usize,
  },
}
fn message_filter() -> HistoryFilter {
  HistoryFilter { kind: Some(HistoryKind::Message), ..Default::default() }
}

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
    Ok(Operation::Search {
      query: HistorySearch { text, mode, limit },
      filter: self.filter,
    })
  }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadArgs {
  sequence: u64,
  #[serde(default)]
  before: usize,
  #[serde(default)]
  after: usize,
}

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

impl SearchHistoryTool {
  /// The binding is fixed at construction; tool arguments cannot select another session.
  pub fn new(session: &Session) -> Self {
    Self { reader: session.create_history_reader() }
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
      Operation::Read { sequence, before, after } => {
        self.reader.read_history_around(sequence, before, after).map(|items| json!({"items":items}))
      }
    }
    .map_err(|error| error.to_string())
  }
}
impl ToolExecutor for SearchHistoryTool {
  async fn execute(&self, call: &ToolCall, control: &ExecutionControl) -> ToolOutcome {
    if control.is_cancelled() {
      return ToolOutcome::Cancelled;
    }
    let operation = match call.name.as_str() {
      "history_search" => match serde_json::from_value::<SearchArgs>(call.arguments.clone()) {
        Ok(args) => match args.into_operation() {
          Ok(op) => op,
          Err(err) => return ToolOutcome::Failed(err),
        },
        Err(err) => return ToolOutcome::Failed(err.to_string()),
      },
      "history_read" => match serde_json::from_value::<ReadArgs>(call.arguments.clone()) {
        Ok(args) => Operation::Read {
          sequence: args.sequence,
          before: args.before,
          after: args.after,
        },
        Err(err) => return ToolOutcome::Failed(err.to_string()),
      },
      "history_query" => match serde_json::from_value::<QueryArgs>(call.arguments.clone()) {
        Ok(args) => args.into_operation(),
        Err(err) => return ToolOutcome::Failed(err.to_string()),
      },
      _ => return ToolOutcome::Failed(format!("unknown tool: {}", call.name)),
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

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_atomic_history_specifications_and_args() {
    let dummy_filter = filter_schema();
    assert!(dummy_filter.is_object());

    // Test SearchArgs flat parsing
    let args: SearchArgs = serde_json::from_value(json!({
      "text": "hello",
      "limit": 10
    })).unwrap();
    let op = args.into_operation().unwrap();
    assert!(matches!(op, Operation::Search { query, .. } if query.text == "hello" && query.limit == 10));

    // Test SearchArgs missing text
    let bad_args: SearchArgs = serde_json::from_value(json!({
      "limit": 10
    })).unwrap();
    assert!(bad_args.into_operation().is_err());

    // Test ReadArgs parsing
    let read_args: ReadArgs = serde_json::from_value(json!({
      "sequence": 42,
      "before": 2
    })).unwrap();
    assert_eq!(read_args.sequence, 42);
    assert_eq!(read_args.before, 2);
    assert_eq!(read_args.after, 0);

    // Test ReadArgs missing sequence
    assert!(serde_json::from_value::<ReadArgs>(json!({})).is_err());

    // Test QueryArgs parsing
    let query_args: QueryArgs = serde_json::from_value(json!({
      "limit": 15,
      "order": "newest_first"
    })).unwrap();
    let op = query_args.into_operation();
    assert!(matches!(op, Operation::Query { page, .. } if page.limit == 15 && page.order == HistoryOrder::NewestFirst));
  }
}

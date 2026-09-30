use crate::storage::{ListId, StorageError, Transaction};
use rusqlite::{params_from_iter, types::Value};

#[derive(Default)]
pub(crate) struct IndexFilter {
  pub item_kind: Option<String>,
  pub message_types: Vec<String>,
  pub event_types: Vec<String>,
  pub origins: Vec<String>,
  pub since: Option<u64>,
  pub until: Option<u64>,
  pub generation: Option<u64>,
  pub model_call: Option<u64>,
  pub tool_name: Option<String>,
  pub metadata: Vec<MetadataCondition>,
}
pub(crate) enum MetadataCondition {
  Equals { path: String, value: String },
  Exists { path: String },
}
pub(crate) enum SearchText {
  Terms(String),
  Substring(String),
}
pub(crate) struct IndexPage {
  pub end: u64,
  pub after: Option<u64>,
  pub newest_first: bool,
  pub limit: usize,
}
pub(crate) struct IndexHit {
  pub sequence: u64,
  pub message_type: Option<String>,
  pub event_type: Option<String>,
  pub tool_name: Option<String>,
  pub snippet: Option<String>,
  pub score: Option<f64>,
}
fn quote_literal(text: &str) -> String {
  format!("'{}'", text.replace('\'', "''"))
}
fn quote_fts(text: &str) -> String {
  format!("\"{}\"", text.replace('"', "\"\""))
}
fn integer(value: u64) -> Result<Value, StorageError> {
  Ok(Value::Integer(super::to_integer(value)?))
}
impl Transaction<'_> {
  pub(crate) fn query_history_index(
    &self,
    list: &ListId,
    filter: IndexFilter,
    page: IndexPage,
    search: Option<SearchText>,
  ) -> Result<Vec<IndexHit>, StorageError> {
    let mut predicates = vec![
      "h.list=(SELECT id FROM wish_list_keys WHERE name=?)".to_string(),
      "h.sequence<?".to_string(),
    ];
    let mut values = vec![Value::Text(list.0.clone()), integer(page.end)?];
    push_filters(filter, &mut predicates, &mut values)?;
    let source = match search {
      Some(search) => search_source(search, &mut predicates, &mut values),
      None => {
        if let Some(after) = page.after {
          predicates.push(format!("h.sequence{}?", if page.newest_first { "<" } else { ">" }));
          values.push(integer(after)?);
        }
        Source::default()
      }
    };
    let order = source
      .order
      .unwrap_or_else(|| format!("h.sequence {}", if page.newest_first { "DESC" } else { "ASC" }));
    let parameters = source
      .snippet_values
      .into_iter()
      .chain(values)
      .chain([Value::Integer(page.limit.try_into().map_err(|_| StorageError::InvalidRange)?)]);
    let sql = format!(
      "SELECT h.sequence,{},{},h.message_type,h.event_type,h.tool_name FROM {} WHERE {} ORDER BY {order} LIMIT ?",
      source.snippet,
      source.score,
      source.from,
      predicates.join(" AND ")
    );
    Ok(
      self
        .sql
        .prepare(&sql)?
        .query_map(params_from_iter(parameters), |row| {
          Ok(IndexHit {
            sequence: row.get::<_, i64>(0)? as u64,
            snippet: row.get(1)?,
            score: row.get(2)?,
            message_type: row.get(3)?,
            event_type: row.get(4)?,
            tool_name: row.get(5)?,
          })
        })?
        .collect::<Result<_, _>>()?,
    )
  }
}

/// Adds the conditions `filter` sets, and the values they bind.
fn push_filters(
  filter: IndexFilter,
  predicates: &mut Vec<String>,
  values: &mut Vec<Value>,
) -> Result<(), StorageError> {
  for (column, value) in [("item_kind", filter.item_kind), ("tool_name", filter.tool_name)] {
    if let Some(value) = value {
      predicates.push(format!("h.{column}=?"));
      values.push(Value::Text(value));
    }
  }
  for (column, choices) in [
    ("message_type", filter.message_types),
    ("event_type", filter.event_types),
    ("origin", filter.origins),
  ] {
    if !choices.is_empty() {
      predicates.push(format!("h.{column} IN ({})", vec!["?"; choices.len()].join(",")));
      values.extend(choices.into_iter().map(Value::Text));
    }
  }
  for (column, op, value) in [
    ("recorded_at", ">=", filter.since),
    ("recorded_at", "<", filter.until),
    ("generation", "=", filter.generation),
    ("model_call", "=", filter.model_call),
  ] {
    if let Some(value) = value {
      predicates.push(format!("h.{column}{op}?"));
      values.push(integer(value)?);
    }
  }
  if !filter.metadata.is_empty() {
    predicates.push("h.item_kind='message'".into());
  }
  for condition in filter.metadata {
    match condition {
      MetadataCondition::Exists { path } => {
        predicates.push(format!("json_type(h.metadata,{}) IS NOT NULL", quote_literal(&path)))
      }
      MetadataCondition::Equals { path, value } => {
        let path = quote_literal(&path);
        predicates.push(format!("json_extract(h.metadata,{path}) IS json_extract(?,'$') AND json_type(h.metadata,{path})=json_type(?,'$')"));
        values.extend([Value::Text(value.clone()), Value::Text(value)]);
      }
    }
  }
  Ok(())
}

/// Where a query reads its rows from and what it reports of each, which the search text decides.
struct Source {
  from: String,
  snippet: String,
  /// Values the snippet binds, ahead of the predicates' values.
  snippet_values: Vec<Value>,
  score: String,
  /// Replaces ordering by sequence.
  order: Option<String>,
}
impl Default for Source {
  fn default() -> Self {
    Self {
      from: "wish_history_index h".into(),
      snippet: "NULL".into(),
      snippet_values: Vec::new(),
      score: "NULL".into(),
      order: None,
    }
  }
}

/// The source that finds `search`, with the condition it adds. A full-text search reads the
/// matching table, best matches first.
fn search_source(
  search: SearchText,
  predicates: &mut Vec<String>,
  values: &mut Vec<Value>,
) -> Source {
  let (table, query) = match search {
    SearchText::Substring(text) if text.chars().count() < 3 => {
      // Trigram indexes cannot answer shorter needles. Scan only rows surviving the filters.
      predicates.push("instr(lower(h.text),lower(?))>0".into());
      values.push(Value::Text(text.clone()));
      return Source {
        snippet: "substr(h.text,max(1,instr(lower(h.text),lower(?))-80),240)".into(),
        snippet_values: vec![Value::Text(text)],
        ..Source::default()
      };
    }
    SearchText::Terms(text) => (
      "wish_history_words",
      text.split_whitespace().map(quote_fts).collect::<Vec<_>>().join(" AND "),
    ),
    SearchText::Substring(text) => ("wish_history_substrings", quote_fts(&text)),
  };
  predicates.push(format!("{table} MATCH ?"));
  values.push(Value::Text(query));
  let score = format!("bm25({table})");
  Source {
    from: format!("{table} JOIN wish_history_index h ON h.id={table}.rowid"),
    snippet: format!("snippet({table},0,'[',']',' ... ',32)"),
    snippet_values: Vec::new(),
    order: Some(format!("{score},h.sequence DESC")),
    score,
  }
}

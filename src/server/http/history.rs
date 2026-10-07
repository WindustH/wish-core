//! A session's history: the timeline the chat page pages through - its messages, with run outcomes
//! if asked - and the history queries, full-text search and reads around one record.
use crate::server::{
  app::App,
  error::{ApiError, blocking},
  session::web_event,
};
use crate::session::history::query::{
  HistoryContent, HistoryCursor, HistoryFilter, HistoryKind, HistoryOrder, HistoryPageRequest,
  HistorySearch,
};
use axum::{
  Json,
  extract::{Path, Query, State},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Timeline {
  include_outcomes: bool,
  limit: usize,
  before: Option<u64>,
  after: Option<u64>,
  order: String,
}
impl Default for Timeline {
  fn default() -> Self {
    Self { include_outcomes: false, limit: 40, before: None, after: None, order: "desc".into() }
  }
}
pub async fn timeline(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Query(query): Query<Timeline>,
) -> Result<Json<Value>, ApiError> {
  let reader = app.get_session(&id).await?.reader.clone();
  blocking(move || {
    let order = match query.order.as_str() {
      "asc" => HistoryOrder::OldestFirst,
      "desc" => HistoryOrder::NewestFirst,
      _ => return Err(ApiError::bad_request("invalid order")),
    };
    let messages = || HistoryFilter { kind: Some(HistoryKind::Message), ..Default::default() };
    let base =
      reader.query_history(messages(), HistoryPageRequest { limit: 1, ..Default::default() })?;
    let cursor = if let Some(before) = query.before {
      Some(HistoryCursor {
        end_sequence: before.min(base.end_sequence),
        after_sequence: before,
        order,
      })
    } else {
      query.after.map(|after| HistoryCursor {
        end_sequence: base.end_sequence,
        after_sequence: after,
        order,
      })
    };
    let page = reader.query_history(
      messages(),
      HistoryPageRequest { limit: query.limit, order, cursor: cursor.clone() },
    )?;
    let mut matches = page.items;
    let mut has_more = page.next.is_some();
    // Notes of the application's - a message sent to a group - belong in the conversation; each
    // run's outcome only when asked for.
    let mut event_types = vec!["Application".to_owned()];
    if query.include_outcomes {
      event_types.push("Finished".into());
    }
    let events = reader.query_history(
      HistoryFilter { kind: Some(HistoryKind::Event), event_types, ..Default::default() },
      HistoryPageRequest { limit: query.limit, order, cursor },
    )?;
    if !events.items.is_empty() {
      has_more |= events.next.is_some();
      matches.extend(events.items);
      matches.sort_by_key(|item| item.record.sequence);
      if order == HistoryOrder::NewestFirst {
        matches.reverse();
      }
      has_more |= matches.len() > query.limit;
      matches.truncate(query.limit);
    }
    let next = if has_more {
      matches.last().map(|item| HistoryCursor {
        end_sequence: page.end_sequence,
        after_sequence: item.record.sequence,
        order,
      })
    } else {
      None
    };
    let mut items = Vec::new();
    for item in &matches {
      let Some(item) = reader.read_history_item(item.record.sequence)? else { continue };
      if let HistoryContent::Event(event) = &item.content {
        if let Some(event) = web_event(event) {
          items.push(json!({"record":item.record,"content":{"kind":"event","value":event}}));
        }
      } else {
        items.push(json!(item));
      }
    }
    Ok(Json(json!({"items":items,"next":next,"end_sequence":page.end_sequence})))
  })
  .await
}

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct HistoryQuery {
  filter: HistoryFilter,
  page: HistoryPageRequest,
}
pub async fn query(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Json(query): Json<HistoryQuery>,
) -> Result<Json<Value>, ApiError> {
  let reader = app.get_session(&id).await?.reader.clone();
  blocking(move || Ok(Json(json!(reader.query_history(query.filter, query.page)?)))).await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Search {
  #[serde(default)]
  filter: HistoryFilter,
  query: HistorySearch,
}
pub async fn search(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Json(query): Json<Search>,
) -> Result<Json<Value>, ApiError> {
  let reader = app.get_session(&id).await?.reader.clone();
  blocking(move || Ok(Json(json!(reader.search_history(query.query, query.filter)?)))).await
}
#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Around {
  before: usize,
  after: usize,
}
pub async fn read(
  State(app): State<Arc<App>>,
  Path((id, sequence)): Path<(String, u64)>,
  Query(around): Query<Around>,
) -> Result<Json<Value>, ApiError> {
  let reader = app.get_session(&id).await?.reader.clone();
  blocking(move || {
    if reader.read_history_item(sequence)?.is_none() {
      return Err(ApiError::not_found());
    }
    Ok(Json(json!(reader.read_history_around(sequence, around.before, around.after)?)))
  })
  .await
}

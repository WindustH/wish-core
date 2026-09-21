use super::{Entry, HistoryItem, HistoryRecord, SessionEvent, index, query::*};
use crate::{
  session::{Session, SessionError, persistence::SessionRecord},
  storage::{
    ListId, PAGE_SIZE, Storage, StorageError, Transaction,
    search::{IndexFilter, IndexHit, IndexPage, MetadataCondition, SearchText},
  },
};

/// Cloneable session-scoped reader. It never claims the running owner or changes session facts.
#[derive(Clone)]
pub struct HistoryReader {
  storage: Storage,
  history: ListId,
  entries: ListId,
  events: ListId,
}
impl Session {
  pub fn create_history_reader(&self) -> HistoryReader {
    HistoryReader::from_record(self.storage.clone(), &self.record)
  }
  /// Add an expression index for a frequently filtered SQLite JSON path, e.g. $.project_id.
  pub fn index_history_metadata(&self, path: String) -> Result<(), SessionError> {
    validate_path(&path)?;
    self.storage.transaction(move |tx| tx.index_history_metadata(&path).map_err(SessionError::from))
  }
  pub fn rebuild_history_index(&self) -> Result<(), SessionError> {
    self.create_history_reader().rebuild_index()
  }
}
impl HistoryReader {
  /// Open without claiming session ownership, including while an executor owns the session.
  pub fn open(storage: Storage, id: &str) -> Result<Self, SessionError> {
    let key = Session::build_key(id);
    let record = storage.transaction(move |tx| -> Result<_, SessionError> {
      Ok(tx.load_object::<SessionRecord>(&key)?)
    })?;
    Ok(Self::from_record(storage, &record))
  }
  fn from_record(storage: Storage, record: &SessionRecord) -> Self {
    Self {
      storage,
      history: record.history.clone(),
      entries: record.entries.clone(),
      events: record.events.clone(),
    }
  }
  fn rebuild_index(&self) -> Result<(), SessionError> {
    let reader = self.clone();
    self.storage.transaction(move |tx| {
      tx.reset_history_index(&reader.history.0)?;
      let end = tx.list_len::<HistoryRecord>(&reader.history)?;
      let mut start = 0;
      while start < end {
        let page = tx.read_page::<HistoryRecord>(
          &reader.history,
          start,
          (end - start).min(PAGE_SIZE) as usize,
        )?;
        for record in &page.items {
          index::index_record(tx, &reader.history, &reader.entries, &reader.events, record)?;
        }
        start += page.items.len() as u64;
      }
      Ok(())
    })
  }
  pub fn query_history(
    &self,
    filter: HistoryFilter,
    page: HistoryPageRequest,
  ) -> Result<HistoryPage, SessionError> {
    validate_limit(page.limit)?;
    let filter = convert_filter(filter)?;
    if page.cursor.as_ref().is_some_and(|cursor| cursor.order != page.order) {
      return Err(SessionError::InvalidHistoryQuery("cursor order differs from page order".into()));
    }
    let reader = self.clone();
    self.storage.transaction(move |tx| {
      let end = match &page.cursor {
        Some(cursor) => cursor.end_sequence,
        None => tx.list_len::<HistoryRecord>(&reader.history)?,
      };
      let hits = tx.query_history_index(
        &reader.history.0,
        filter,
        IndexPage {
          end,
          after: page.cursor.map(|cursor| cursor.after_sequence),
          newest_first: page.order == HistoryOrder::NewestFirst,
          limit: page.limit + 1,
        },
        None,
      )?;
      let has_more = hits.len() > page.limit;
      let items = reader.load_matches(tx, hits.into_iter().take(page.limit))?;
      let next = if has_more {
        items.last().map(|item| HistoryCursor {
          end_sequence: end,
          after_sequence: item.record.sequence,
          order: page.order,
        })
      } else {
        None
      };
      Ok(HistoryPage { items, next, end_sequence: end })
    })
  }
  pub fn search_history(
    &self,
    query: HistorySearch,
    filter: HistoryFilter,
  ) -> Result<HistorySearchResults, SessionError> {
    validate_limit(query.limit)?;
    if query.text.trim().is_empty() {
      return Err(SessionError::InvalidHistoryQuery("search text is empty".into()));
    }
    let filter = convert_filter(filter)?;
    let used_text_index =
      !matches!(query.mode, HistorySearchMode::Substring) || query.text.chars().count() >= 3;
    let search = match query.mode {
      HistorySearchMode::Terms => SearchText::Terms(query.text),
      HistorySearchMode::Substring => SearchText::Substring(query.text),
    };
    let reader = self.clone();
    self.storage.transaction(move |tx| {
      let end = tx.list_len::<HistoryRecord>(&reader.history)?;
      let hits = tx.query_history_index(
        &reader.history.0,
        filter,
        IndexPage { end, after: None, newest_first: true, limit: query.limit + 1 },
        Some(search),
      )?;
      let has_more = hits.len() > query.limit;
      let items = reader.load_matches(tx, hits.into_iter().take(query.limit))?;
      Ok(HistorySearchResults { items, end_sequence: end, has_more, used_text_index })
    })
  }
  pub fn read_history_item(&self, sequence: u64) -> Result<Option<HistoryEntry>, SessionError> {
    let reader = self.clone();
    self.storage.transaction(move |tx| {
      tx.get_item::<HistoryRecord>(&reader.history, sequence)?
        .map(|record| reader.load_entry(tx, record))
        .transpose()
    })
  }
  /// Expand timeline neighbors, not a protocol-ready replay fragment.
  pub fn read_history_around(
    &self,
    sequence: u64,
    before: usize,
    after: usize,
  ) -> Result<Vec<HistoryEntry>, SessionError> {
    let count =
      before.checked_add(after).and_then(|n| n.checked_add(1)).ok_or(StorageError::InvalidRange)?;
    let reader = self.clone();
    self.storage.transaction(move |tx| {
      if sequence >= tx.list_len::<HistoryRecord>(&reader.history)? {
        return Ok(vec![]);
      }
      let start = sequence.saturating_sub(before as u64);
      let count = count - (before as u64 - (sequence - start)) as usize;
      let page = tx.read_page::<HistoryRecord>(&reader.history, start, count)?;
      page.items.into_iter().map(|record| reader.load_entry(tx, record)).collect()
    })
  }
  fn load_entry(
    &self,
    tx: &mut Transaction<'_>,
    record: std::sync::Arc<HistoryRecord>,
  ) -> Result<HistoryEntry, SessionError> {
    let content = match record.item {
      HistoryItem::Message(id) => HistoryContent::Message(
        tx.get_item::<Entry>(&self.entries, id.0 as u64)?.ok_or(SessionError::InvalidEntry(id))?,
      ),
      HistoryItem::Event(id) => HistoryContent::Event(
        tx.get_item::<SessionEvent>(&self.events, id.0)?
          .ok_or_else(|| StorageError::Corrupt("missing history event".into()))?,
      ),
    };
    Ok(HistoryEntry { record, content })
  }
  fn load_matches(
    &self,
    tx: &mut Transaction<'_>,
    hits: impl Iterator<Item = IndexHit>,
  ) -> Result<Vec<HistoryMatch>, SessionError> {
    hits
      .map(|hit| {
        let record = tx
          .get_item::<HistoryRecord>(&self.history, hit.sequence)?
          .ok_or_else(|| StorageError::Corrupt("missing indexed history record".into()))?;
        Ok(HistoryMatch {
          record,
          snippet: hit.snippet,
          score: hit.score,
          event_type: hit.event_type,
          tool_name: hit.tool_name,
          message_type: hit
            .message_type
            .map(|name| serde_json::from_value(serde_json::Value::String(name)))
            .transpose()
            .map_err(StorageError::from)?,
        })
      })
      .collect()
  }
}
fn validate_limit(limit: usize) -> Result<(), SessionError> {
  if limit == 0 || limit >= i64::MAX as usize {
    return Err(SessionError::InvalidHistoryQuery(
      "limit must be positive and fit a SQLite range".into(),
    ));
  }
  Ok(())
}
fn validate_path(path: &str) -> Result<(), SessionError> {
  if !path.starts_with('$') {
    return Err(SessionError::InvalidHistoryQuery("metadata path must start with $".into()));
  }
  Ok(())
}
fn convert_filter(filter: HistoryFilter) -> Result<IndexFilter, SessionError> {
  if filter.since.zip(filter.until).is_some_and(|(start, end)| start > end) {
    return Err(SessionError::InvalidHistoryQuery("since is later than until".into()));
  }
  let mut metadata = vec![];
  for condition in filter.metadata {
    metadata.push(match condition {
      MetadataFilter::Exists { path } => {
        validate_path(&path)?;
        MetadataCondition::Exists { path }
      }
      MetadataFilter::Equals { path, value } => {
        validate_path(&path)?;
        if value.is_array() || value.is_object() {
          return Err(SessionError::InvalidHistoryQuery(
            "metadata equality requires a scalar JSON value".into(),
          ));
        }
        MetadataCondition::Equals { path, value: value.to_string() }
      }
    });
  }
  Ok(IndexFilter {
    item_kind: filter.kind.map(|kind| match kind {
      HistoryKind::Message => "message".into(),
      HistoryKind::Event => "event".into(),
    }),
    message_types: filter.message_types.into_iter().map(|kind| kind.as_str().into()).collect(),
    event_types: filter.event_types,
    origins: filter
      .origins
      .into_iter()
      .map(|origin| serde_json::to_value(origin).map(|value| value.as_str().unwrap().to_owned()))
      .collect::<Result<_, _>>()
      .map_err(StorageError::from)?,
    since: filter.since.map(|time| time.0),
    until: filter.until.map(|time| time.0),
    generation: filter.generation.map(|id| id.0 as u64),
    model_call: filter.model_call_id.map(|id| id.0),
    tool_name: filter.tool_name,
    metadata,
  })
}

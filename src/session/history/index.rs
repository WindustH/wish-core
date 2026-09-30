//! What the history index keeps of each record: the fields history is filtered by, and the text it
//! is searched by.
use super::query::{HistoryKind, MessageType};
use super::{Entry, Fact, HistoryRecord, SessionEvent};
use crate::{
  protocol::{ContentBlock, Message},
  session::{RunOutcome, SessionError},
  storage::{ListId, StorageError, Transaction, history_index::IndexRow},
};

/// Indexes `record`, about what `fact` holds, in the index of the history list `history`.
pub(super) fn index_record(
  store: &mut Transaction<'_>,
  history: &ListId,
  record: &HistoryRecord,
  fact: Fact<'_>,
) -> Result<(), SessionError> {
  let mut row = IndexRow {
    sequence: record.sequence,
    recorded_at: record.recorded_at.0,
    generation: record.generation.0 as u64,
    model_call: record.model_call_id.map(|id| id.0),
    metadata: "null".into(),
    ..Default::default()
  };
  match fact {
    Fact::Message(entry) => fill_message_fields(&mut row, entry)?,
    Fact::Event(_, event) => fill_event_fields(&mut row, event),
  }
  store.index_history_record(history, &row)?;
  Ok(())
}

fn fill_message_fields(row: &mut IndexRow, entry: &Entry) -> Result<(), SessionError> {
  row.item_kind = HistoryKind::Message.as_str();
  row.origin = Some(entry.origin.as_str());
  row.metadata = serde_json::to_string(entry.message.get_metadata()).map_err(StorageError::from)?;
  row.message_type = Some(MessageType::of(&entry.message).as_str());
  let text = match &entry.message {
    Message::System { content, .. }
    | Message::Developer { content, .. }
    | Message::User { content, .. }
    | Message::Assistant { content, .. } => extract_text(content),
    Message::Reasoning { plaintext, display, .. } => {
      if plaintext.is_empty() {
        display.clone()
      } else {
        plaintext.clone()
      }
    }
    Message::ToolUse { name, arguments, .. } => {
      row.tool_name = Some(name.clone());
      format!("{name}\n{arguments}")
    }
    Message::ToolResult { name, content, .. } => {
      row.tool_name = Some(name.clone());
      format!("{name}\n{content}")
    }
    Message::UpstreamCompaction { .. } => String::new(),
  };
  // History search output is a projection of existing history, and its arguments repeat the
  // query itself. Keep it filterable, but do not index these duplicate retrieval documents.
  row.text = if matches!(
    row.tool_name.as_deref(),
    Some("history_search" | "history_read" | "history_query")
  ) {
    String::new()
  } else {
    text
  };
  Ok(())
}
fn extract_text(blocks: &[ContentBlock]) -> String {
  blocks
    .iter()
    .filter_map(|block| match block {
      ContentBlock::Text { text } => Some(text.as_str()),
      ContentBlock::Image { .. } => None,
    })
    .collect::<Vec<_>>()
    .join("\n")
}

fn fill_event_fields(row: &mut IndexRow, event: &SessionEvent) {
  row.item_kind = HistoryKind::Event.as_str();
  row.event_type = Some(event.type_name());
  match event {
    SessionEvent::ToolStarted(call) | SessionEvent::ToolFinished { call, .. } => {
      row.tool_name = Some(call.name.clone())
    }
    SessionEvent::Finished(RunOutcome::Failed(error))
    | SessionEvent::CompactionSummaryFailed { outcome: RunOutcome::Failed(error) }
    | SessionEvent::CompactionTranslationFailed { outcome: RunOutcome::Failed(error) } => {
      row.text = error.to_string();
    }
    _ => {}
  }
}

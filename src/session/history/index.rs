use super::{Entry, HistoryItem, HistoryRecord, SessionEvent};
use crate::{
  protocol::{ContentBlock, Message},
  session::{RunOutcome, SessionError},
  storage::{ListId, Transaction, search::IndexRecord},
};

pub(super) fn index_record(
  tx: &mut Transaction<'_>,
  history: &ListId,
  entries: &ListId,
  events: &ListId,
  record: &HistoryRecord,
) -> Result<(), SessionError> {
  let mut row = create_row(record);
  match record.item {
    HistoryItem::Message(id) => {
      let entry =
        tx.get_item::<Entry>(entries, id.0 as u64)?.ok_or(SessionError::InvalidEntry(id))?;
      row.item_kind = "message";
      row.origin = serde_json::to_value(entry.origin)
        .map_err(crate::storage::StorageError::from)?
        .as_str()
        .map(str::to_owned);
      row.metadata = serde_json::to_string(entry.message.get_metadata())
        .map_err(crate::storage::StorageError::from)?;
      let (kind, text) = match &entry.message {
        Message::System { content, .. } => ("system", extract_text(content)),
        Message::Developer { content, .. } => ("developer", extract_text(content)),
        Message::User { content, .. } => ("user", extract_text(content)),
        Message::Assistant { content, .. } => ("assistant", extract_text(content)),
        Message::Reasoning { plaintext, display, .. } => {
          ("reasoning", if plaintext.is_empty() { display.clone() } else { plaintext.clone() })
        }
        Message::ToolUse { name, arguments, .. } => {
          row.tool_name = Some(name.clone());
          ("tool_use", format!("{name}\n{arguments}"))
        }
        Message::ToolResult { name, content, .. } => {
          row.tool_name = Some(name.clone());
          ("tool_result", format!("{name}\n{content}"))
        }
        Message::UpstreamCompaction { .. } => ("upstream_compaction", String::new()),
      };
      row.message_type = Some(kind);
      // History search output is a projection of existing history, and its arguments repeat the
      // query itself. Keep it filterable, but do not index these duplicate retrieval documents.
      row.text =
        if row.tool_name.as_deref() == Some("search_history") { String::new() } else { text };
    }
    HistoryItem::Event(id) => {
      let event = tx
        .get_item::<SessionEvent>(events, id.0)?
        .ok_or_else(|| crate::storage::StorageError::Corrupt("missing history event".into()))?;
      fill_event_fields(&mut row, &event);
    }
  }
  tx.index_history_record(&history.0, &row)?;
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
fn get_event_type(event: &SessionEvent) -> &'static str {
  match event {
    SessionEvent::UpstreamCompactionStarted { .. } => "UpstreamCompactionStarted",
    SessionEvent::UpstreamCompactionCompleted(..) => "UpstreamCompactionCompleted",
    SessionEvent::CompactionSummaryStarted { .. } => "CompactionSummaryStarted",
    SessionEvent::CompactionSummary { .. } => "CompactionSummary",
    SessionEvent::ContextCompacted { .. } => "ContextCompacted",
    SessionEvent::Created(..) => "Created",
    SessionEvent::ContextEntryCreated { .. } => "ContextEntryCreated",
    SessionEvent::ResponseRejected(..) => "ResponseRejected",
    SessionEvent::MetadataUpdated(..) => "MetadataUpdated",
    SessionEvent::ConfigUpdated(..) => "ConfigUpdated",
    SessionEvent::MessageQueued { .. } => "MessageQueued",
    SessionEvent::InputsConsumed { .. } => "InputsConsumed",
    SessionEvent::StateChanged { .. } => "StateChanged",
    SessionEvent::TurnStarted { .. } => "TurnStarted",
    SessionEvent::ModelStream(..) => "ModelStream",
    SessionEvent::ResponseAccepted { .. } => "ResponseAccepted",
    SessionEvent::ResponseInterrupted(..) => "ResponseInterrupted",
    SessionEvent::ToolStarted(..) => "ToolStarted",
    SessionEvent::ToolFinished { .. } => "ToolFinished",
    SessionEvent::StableBoundary { .. } => "StableBoundary",
    SessionEvent::Finished(..) => "Finished",
    SessionEvent::GenerationPrepared { .. } => "GenerationPrepared",
    SessionEvent::GenerationActivated { .. } => "GenerationActivated",
  }
}

fn create_row(record: &HistoryRecord) -> IndexRecord {
  IndexRecord {
    sequence: record.sequence,
    recorded_at: record.recorded_at.0,
    generation: record.generation.0 as u64,
    model_call: record.model_call_id.map(|id| id.0),
    metadata: "null".into(),
    ..Default::default()
  }
}

fn fill_event_fields(row: &mut IndexRecord, event: &SessionEvent) {
  row.item_kind = "event";
  row.event_type = Some(get_event_type(event));
  match event {
    SessionEvent::ToolStarted(call) | SessionEvent::ToolFinished { call, .. } => {
      row.tool_name = Some(call.name.clone())
    }
    SessionEvent::Finished(RunOutcome::Failed(error)) => row.text = error.to_string(),
    _ => {}
  }
}

pub(super) fn index_event_record(
  tx: &mut Transaction<'_>,
  history: &ListId,
  record: &HistoryRecord,
  event: &SessionEvent,
) -> Result<(), SessionError> {
  let mut row = create_row(record);
  fill_event_fields(&mut row, event);
  tx.index_history_record(&history.0, &row)?;
  Ok(())
}

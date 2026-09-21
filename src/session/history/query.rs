//! Public history filters and bounded query results.
use super::{Entry, EntryOrigin, HistoryRecord, SessionEvent};
use crate::session::{
  GenerationId,
  statistics::{ModelCallId, Timestamp},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageType {
  System,
  Developer,
  User,
  Assistant,
  Reasoning,
  ToolUse,
  ToolResult,
  UpstreamCompaction,
}
impl MessageType {
  pub(super) fn as_str(self) -> &'static str {
    match self {
      Self::System => "system",
      Self::Developer => "developer",
      Self::User => "user",
      Self::Assistant => "assistant",
      Self::Reasoning => "reasoning",
      Self::ToolUse => "tool_use",
      Self::ToolResult => "tool_result",
      Self::UpstreamCompaction => "upstream_compaction",
    }
  }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryKind {
  Message,
  Event,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum MetadataFilter {
  /// Scalar JSON equality. Missing is distinct from explicit null; booleans are distinct from numbers.
  Equals {
    path: String,
    value: serde_json::Value,
  },
  Exists {
    path: String,
  },
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HistoryFilter {
  pub kind: Option<HistoryKind>,
  pub message_types: Vec<MessageType>,
  /// SessionEvent variant names, e.g. "Finished", "ModelStream".
  pub event_types: Vec<String>,
  pub origins: Vec<EntryOrigin>,
  /// Inclusive history-record timestamp, in Unix milliseconds.
  pub since: Option<Timestamp>,
  /// Exclusive history-record timestamp.
  pub until: Option<Timestamp>,
  pub generation: Option<GenerationId>,
  pub model_call_id: Option<ModelCallId>,
  pub tool_name: Option<String>,
  pub metadata: Vec<MetadataFilter>,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryOrder {
  #[default]
  OldestFirst,
  NewestFirst,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoryCursor {
  /// Exclusive upper sequence captured on the first page.
  pub end_sequence: u64,
  pub after_sequence: u64,
  pub order: HistoryOrder,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HistoryPageRequest {
  pub limit: usize,
  pub order: HistoryOrder,
  pub cursor: Option<HistoryCursor>,
}
impl Default for HistoryPageRequest {
  fn default() -> Self {
    Self { limit: 50, order: HistoryOrder::OldestFirst, cursor: None }
  }
}
#[derive(Clone, Debug, Serialize)]
pub struct HistoryMatch {
  pub message_type: Option<MessageType>,
  pub event_type: Option<String>,
  pub tool_name: Option<String>,
  pub record: Arc<HistoryRecord>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub snippet: Option<String>,
  /// FTS5 BM25 score: smaller sorts first. Absent for short substring scans.
  #[serde(skip_serializing_if = "Option::is_none")]
  pub score: Option<f64>,
}
#[derive(Clone, Debug, Serialize)]
pub struct HistoryPage {
  pub items: Vec<HistoryMatch>,
  pub next: Option<HistoryCursor>,
  pub end_sequence: u64,
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistorySearchMode {
  #[default]
  Terms,
  Substring,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HistorySearch {
  pub text: String,
  pub mode: HistorySearchMode,
  pub limit: usize,
}
impl Default for HistorySearch {
  fn default() -> Self {
    Self { text: String::new(), mode: HistorySearchMode::Terms, limit: 20 }
  }
}
#[derive(Clone, Debug, Serialize)]
pub struct HistorySearchResults {
  pub items: Vec<HistoryMatch>,
  pub end_sequence: u64,
  pub has_more: bool,
  /// Short substring searches scan the filtered rows because trigram needs three characters.
  pub used_text_index: bool,
}
#[derive(Clone, Debug, Serialize)]
pub struct HistoryEntry {
  pub record: Arc<HistoryRecord>,
  pub content: HistoryContent,
}
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum HistoryContent {
  Message(Arc<Entry>),
  Event(Arc<SessionEvent>),
}

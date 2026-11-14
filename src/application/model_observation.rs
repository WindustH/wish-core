//! Records image delivery events and legacy instruction observations in durable history.
use crate::{
  protocol::error::Error,
  protocol::{ContentBlock, Message, Request},
  session::{SessionEvent, SessionReader, SessionSender},
};
use serde_json::Value;
use std::{
  collections::BTreeMap,
  sync::{Arc, Mutex},
};
pub struct InstructionObserver {
  sender: SessionSender,
  events: tokio::sync::broadcast::Sender<Value>,
  reader: SessionReader,
  state: Arc<Mutex<InstructionState>>,
}
/// Shared across model wrappers and generation changes in one open session. The cursor
/// tracks committed history, including observations made outside the current wrapper.
#[derive(Default)]
pub(super) struct InstructionState {
  recorded: BTreeMap<String, Value>,
  image_delivery: BTreeMap<String, Value>,
  next_sequence: u64,
}
impl InstructionObserver {
  pub(super) fn new(
    sender: SessionSender,
    reader: SessionReader,
    events: tokio::sync::broadcast::Sender<Value>,
    state: Arc<Mutex<InstructionState>>,
  ) -> Self {
    Self { sender, reader, events, state }
  }
  /// Commit the admission outcome before sending. Reuse the saved explanation verbatim;
  /// inspection/counting never call this, and generation membership does not reset it.
  pub async fn record_images(
    &self,
    deliveries: Vec<super::model_images::ImageDelivery>,
  ) -> Result<Vec<Message>, Error> {
    let sender = self.sender.clone();
    let reader = self.reader.clone();
    let state = self.state.clone();
    let (messages, changed) = crate::utils::blocking::io(move || {
      let mut state = state.lock().unwrap();
      state.refresh(&reader)?;
      let mut messages = Vec::new();
      let mut changed = false;
      let mut grouped = BTreeMap::<String, (super::model_images::ImageDelivery, bool)>::new();
      for delivery in deliveries {
        let path = delivery.path.to_string_lossy().into_owned();
        let key = if delivery.from_view_image { format!("view_image:{path}") } else { path.clone() };
        let included = delivery.reason.is_none();
        if let Some((previous, any_included)) = grouped.get_mut(&key) {
          *any_included |= included;
          if previous.reason.is_none() && delivery.reason.is_some() { *previous = delivery; }
        } else {
          grouped.insert(key, (delivery, included));
        }
      }
      for (key, (delivery, included)) in grouped {
        let path = delivery.path.to_string_lossy().into_owned();
        let reason = serde_json::json!(delivery.reason);
        if state.image_delivery.get(&key).is_none_or(|event| event["reason"] != reason || event["included"] != included || event["source_path"] != serde_json::json!(delivery.source_path) || event["content"][0]["Text"]["text"].as_str().is_none()) {
          let invalid = || crate::session::SessionError::Storage(crate::storage::StorageError::Corrupt("invalid image delivery path".into()));
          let directory = delivery.path.parent().ok_or_else(invalid)?;
          let id = delivery.path.file_name().and_then(|name| name.to_str()).ok_or_else(invalid)?;
          let aliases = super::env_aliases::Aliases::open(directory)
            .map_err(|error| crate::session::SessionError::Storage(crate::storage::StorageError::Corrupt(error.to_string())))?;
          aliases.blob(id).map_err(|error| crate::session::SessionError::Storage(crate::storage::StorageError::Corrupt(error.to_string())))?;
          let quote = |path: &str| serde_json::json!(aliases.shorten(path));
          let text = if included && let Some(reason) = delivery.reason {
            let source = delivery.source_path.as_deref().unwrap_or(&path);
            format!("Image {}: newer occurrence included; older occurrence omitted ({reason}).", quote(source))
          } else if delivery.from_view_image {
            match (&delivery.source_path, delivery.reason) {
              (Some(path), Some(reason)) => format!("Image {} not included: {reason}.", quote(path)),
              (None, Some(reason)) => format!("Image from view_image not included: {reason}."),
              (Some(path), None) => format!("Image read from {}.", quote(path)),
              (None, None) => "Image read by view_image.".to_owned(),
            }
          } else { match delivery.reason {
            Some(reason) => format!("Image stored at {}. Not included: {reason}.", quote(&path)),
            None => format!("Image included from {}.", quote(&path)),
          }};
          let event = serde_json::json!({"type":"image_delivery","key":key,"path":path,"source_path":delivery.source_path,"reason":reason,"included":included,
            "content":[{"Text":{"text":text}}]});
          sender.record_application_event(event.clone())?;
          state.image_delivery.insert(key.clone(), event);
          changed = true;
        }
        if delivery.reason.is_some() {
          let event = &state.image_delivery[&key];
          messages.push(Message::Developer {
            fixed: None,
            metadata: serde_json::json!({"source":"wish_image_delivery"}),
            content: vec![ContentBlock::Text {
              text: event["content"][0]["Text"]["text"].as_str().unwrap_or("Image not included.").to_owned(),
            }],
          });
        }
      }
      Ok::<_, crate::session::SessionError>((messages, changed))
    })
    .await
    .map_err(|error| Error::Build(error.to_string()))?
    .map_err(|error| Error::Build(error.to_string()))?;
    if changed {
      let _ = self.events.send(serde_json::json!({"type":"history_changed"}));
    }
    Ok(messages)
  }
  pub async fn record(&self, original: &Request, request: &Request) -> Result<(), Error> {
    let (sender, events) = (&self.sender, &self.events);
    let mut notes = Vec::new();
    for (index, message) in request.conversation.iter().enumerate() {
      match message {
        Message::System { content, .. } => {
          let encode = |message: &Message| {
            serde_json::to_value(message).map_err(|error| Error::Build(error.to_string()))
          };
          let before = original.conversation.get(index).map(encode).transpose()?;
          if before.as_ref() != Some(&encode(message)?) {
            notes.push(serde_json::json!({"title":"系统指令（请求时补入）","content":content}));
          }
        }
        Message::User { metadata, content }
          if metadata["source"]
            .as_str()
            .is_some_and(|source| source.starts_with("wish_request_")) =>
        {
          notes.push(serde_json::json!({"title":metadata["instruction_title"],"content":content,"position":index}));
        }
        _ => {}
      }
    }
    if !request.tools.is_empty() {
      notes.push(serde_json::json!({"title":"可用工具定义","content":[{"Text":{"text":serde_json::to_string_pretty(&request.tools).map_err(|error| Error::Build(error.to_string()))?}}]}));
    }
    let sender = sender.clone();
    let reader = self.reader.clone();
    let state = self.state.clone();
    let changed = crate::utils::blocking::io(move || {
      let mut state = state.lock().unwrap();
      // Another model wrapper (for compaction or a resumed run) may have observed context
      // since our previous call. History, rather than an instance-local cache, is authoritative.
      state.refresh(&reader)?;
      let recorded = &mut state.recorded;
      // Compare current state, not all values ever seen: A -> B -> A is two changes.
      // Message positions and generation IDs do not identify request instructions.
      let mut current: BTreeMap<String, Value> = BTreeMap::new();
      for note in notes {
        let key = note_key(&note);
        if let Some(previous) = current.get_mut(&key) {
          if !same_content(previous, &note) {
            previous["content"].as_array_mut().expect("instruction content")
              .extend(note["content"].as_array().expect("instruction content").iter().cloned());
          }
        } else {
          current.insert(key, note);
        }
      }
      let mut changed = false;
      for (key, note) in &current {
        if recorded.get(key).is_some_and(|previous| same_content(previous, note)) {
          continue;
        }
        sender.record_application_event(
          serde_json::json!({"type":"model_instruction","key":key,"operation":"set","instruction":note}),
        )?;
        recorded.insert(key.clone(), note.clone());
        changed = true;
      }
      let removed: Vec<_> = recorded.keys().filter(|key| !current.contains_key(*key)
        && recorded[*key]["title"] != "图片输入说明").cloned().collect();
      for key in removed {
        sender.record_application_event(
          serde_json::json!({"type":"model_instruction","key":key,"operation":"remove","instruction":recorded[&key]}),
        )?;
        recorded.remove(&key);
        changed = true;
      }
      Ok::<_, crate::session::SessionError>(changed)
    })
    .await
    .map_err(|error| Error::Build(error.to_string()))?
    .map_err(|error| Error::Build(error.to_string()))?;
    if changed {
      let _ = events.send(serde_json::json!({"type":"history_changed"}));
    }
    Ok(())
  }
}

fn same_content(left: &Value, right: &Value) -> bool {
  left["title"] == right["title"] && left["content"] == right["content"]
}

/// Each image remains in context independently; replacing a tool set or an instruction
/// changes one slot. Legacy observations have the same titles but no explicit keys.
fn note_key(note: &Value) -> String {
  let title = note["title"].as_str().unwrap_or_default();
  if title == "图片输入说明"
    && let Some(text) = note["content"][0]["Text"]["text"].as_str()
  {
    if let Some(path) = text
      .strip_prefix("Image read and stored at ")
      .or_else(|| text.strip_prefix("Image stored at "))
      && let Some(Ok(path)) = serde_json::Deserializer::from_str(path).into_iter::<String>().next()
    {
      return format!("image:{path}");
    }
    if text.starts_with("[Image sha256:")
      && let Some((identity, _)) = text.split_once(']')
    {
      return identity.to_owned();
    }
  }
  title.to_owned()
}

/// Replay context deltas in history order, including legacy observations as assignments.
/// A generation switch does not reset this state: unchanged context is inherited.
impl InstructionState {
  fn refresh(&mut self, reader: &SessionReader) -> Result<(), crate::session::SessionError> {
    use crate::session::history::{
      HistoryItem,
      query::{HistoryCursor, HistoryFilter, HistoryOrder, HistoryPageRequest},
    };
    let end_sequence = reader.get_history().len()?;
    if end_sequence < self.next_sequence {
      self.recorded.clear();
      self.image_delivery.clear();
      self.next_sequence = 0;
    }
    if end_sequence == self.next_sequence {
      return Ok(());
    }
    let mut cursor = (self.next_sequence > 0).then(|| HistoryCursor {
      end_sequence,
      after_sequence: self.next_sequence - 1,
      order: HistoryOrder::OldestFirst,
    });
    loop {
      let page = reader.query_history(
        HistoryFilter { event_types: vec!["Application".into()], ..Default::default() },
        HistoryPageRequest { limit: 100, cursor, ..Default::default() },
      )?;
      for item in page.items {
        if let HistoryItem::Event(id) = item.record.item
          && let Some(event) = reader.get_event(id)?
          && let SessionEvent::Application(value) = event.as_ref()
        {
          if value["type"] == "image_delivery" {
            if let Some(path) = value["path"].as_str() {
              self
                .image_delivery
                .insert(value["key"].as_str().unwrap_or(path).to_owned(), value.clone());
            }
            continue;
          }
          if value["type"] != "model_instruction" {
            continue;
          }
          let note = &value["instruction"];
          let key = value["key"].as_str().map(str::to_owned).unwrap_or_else(|| note_key(note));
          if value["operation"] == "remove" {
            self.recorded.remove(&key);
          } else {
            self.recorded.insert(key, note.clone());
          }
        }
      }
      cursor = page.next;
      if cursor.is_none() {
        self.next_sequence = page.end_sequence;
        return Ok(());
      }
    }
  }
}

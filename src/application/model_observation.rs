//! Records derived model-instruction observations and announces history changes.
use crate::{
  protocol::error::Error,
  protocol::{ContentBlock, Message, Request},
  session::{SessionEvent, SessionReader, SessionSender},
};
use serde_json::Value;
use std::{
  collections::HashSet,
  sync::{Arc, Mutex},
};
pub struct InstructionObserver {
  sender: SessionSender,
  events: tokio::sync::broadcast::Sender<Value>,
  reader: SessionReader,
  recorded: Arc<Mutex<Option<HashSet<[u8; 32]>>>>,
}
impl InstructionObserver {
  pub fn new(
    sender: SessionSender,
    reader: SessionReader,
    events: tokio::sync::broadcast::Sender<Value>,
  ) -> Self {
    Self { sender, reader, events, recorded: Arc::new(Mutex::new(None)) }
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
      if let Message::User { content, .. } | Message::Assistant { content, .. } = message {
        for block in content {
          if let ContentBlock::Text { text } = block
            && text.starts_with("[Image sha256:")
          {
            notes.push(serde_json::json!({"title":"图片输入说明","content":[block]}));
          }
        }
      }
    }
    if !request.tools.is_empty() {
      notes.push(serde_json::json!({"title":"可用工具定义","content":[{"Text":{"text":serde_json::to_string_pretty(&request.tools).map_err(|error| Error::Build(error.to_string()))?}}]}));
    }
    if notes.is_empty() {
      return Ok(());
    }
    let sender = sender.clone();
    let reader = self.reader.clone();
    let recorded = self.recorded.clone();
    let changed = crate::utils::blocking::io(move || {
      let mut cache = recorded.lock().unwrap();
      if cache.is_none() {
        *cache = Some(recorded_instructions(&reader)?);
      }
      let recorded = cache.as_mut().expect("loaded instruction observations");
      let mut changed = false;
      for note in notes {
        let key = fingerprint(&note);
        if recorded.contains(&key) {
          continue;
        }
        sender.record_application_event(
          serde_json::json!({"type":"model_instruction","instruction":note}),
        )?;
        recorded.insert(key);
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

fn fingerprint(note: &Value) -> [u8; 32] {
  use sha2::Digest;
  sha2::Sha256::digest(note.to_string()).into()
}

/// The model wrapper is rebuilt between runs. Seed its cache from durable observations so
/// resuming a session or restarting the server does not turn old request context into new activity.
fn recorded_instructions(
  reader: &SessionReader,
) -> Result<HashSet<[u8; 32]>, crate::session::SessionError> {
  use crate::session::history::{
    HistoryItem,
    query::{HistoryFilter, HistoryPageRequest},
  };
  let mut recorded = HashSet::new();
  let mut cursor = None;
  loop {
    let page = reader.query_history(
      HistoryFilter { event_types: vec!["Application".into()], ..Default::default() },
      HistoryPageRequest { limit: 100, cursor, ..Default::default() },
    )?;
    for item in page.items {
      if let HistoryItem::Event(id) = item.record.item
        && let Some(event) = reader.get_event(id)?
        && let SessionEvent::Application(value) = event.as_ref()
        && value["type"] == "model_instruction"
      {
        recorded.insert(fingerprint(&value["instruction"]));
      }
    }
    cursor = page.next;
    if cursor.is_none() {
      return Ok(recorded);
    }
  }
}

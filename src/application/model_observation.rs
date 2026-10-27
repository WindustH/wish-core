//! Records derived model-instruction observations and announces history changes.
use crate::{
  protocol::error::Error,
  protocol::{ContentBlock, Message, Request},
  session::SessionSender,
};
use serde_json::Value;
use std::{collections::HashSet, sync::Mutex};
pub struct InstructionObserver {
  sender: SessionSender,
  events: tokio::sync::broadcast::Sender<Value>,
  recorded: Mutex<HashSet<[u8; 32]>>,
}
impl InstructionObserver {
  pub fn new(sender: SessionSender, events: tokio::sync::broadcast::Sender<Value>) -> Self {
    Self { sender, events, recorded: Mutex::new(HashSet::new()) }
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
    let notes: Vec<_> = {
      let mut recorded = self.recorded.lock().unwrap();
      notes
        .into_iter()
        .filter(|note| {
          use sha2::Digest;
          recorded.insert(sha2::Sha256::digest(note.to_string()).into())
        })
        .collect()
    };
    if notes.is_empty() {
      return Ok(());
    }
    let sender = sender.clone();
    crate::utils::blocking::io(move || {
      for note in notes {
        sender.record_application_event(
          serde_json::json!({"type":"model_instruction","instruction":note}),
        )?;
      }
      Ok::<_, crate::session::SessionError>(())
    })
    .await
    .map_err(|error| Error::Build(error.to_string()))?
    .map_err(|error| Error::Build(error.to_string()))?;
    let _ = events.send(serde_json::json!({"type":"history_changed"}));
    Ok(())
  }
}

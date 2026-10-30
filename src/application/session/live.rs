//! Coalesced current-turn preview. No token/chunk log, and no disk persistence.
use crate::client::{RetryPhase, RetryStatus};
use crate::{protocol::StreamEvent, session::SessionEvent};
use serde_json::{Value, json};
use std::collections::HashMap;

#[derive(Default)]
pub(super) struct LivePreview {
  pub revision: u64,
  events: Vec<Value>,
  checkpoint: Option<(u64, Vec<Value>, HashMap<String, usize>)>,
  positions: HashMap<String, usize>,
  retries: HashMap<String, RetryStatus>,
}
impl LivePreview {
  pub fn snapshot(&self, data: Value) -> Value {
    let checkpoint = self
      .checkpoint
      .as_ref()
      .map(|(revision, events, _)| json!({"revision":revision, "events":events}));
    json!({"type":"snapshot", "data":data, "revision":self.revision, "live_events":self.events,
      "stream_checkpoint":checkpoint,
      "retries":self.retries.values().collect::<Vec<_>>(), "server_time":crate::utils::time::Timestamp::now().0})
  }
  pub fn observe_retry(&mut self, status: &RetryStatus) {
    self.revision += 1;
    if status.phase == RetryPhase::Finished {
      self.retries.remove(&status.request_id);
    } else {
      self.retries.insert(status.request_id.clone(), status.clone());
    }
  }
  fn clear(&mut self) {
    self.checkpoint = None;
    self.events.clear();
    self.positions.clear();
  }
  pub fn observe(&mut self, event: &SessionEvent) {
    self.revision += 1;
    match event {
      SessionEvent::TurnStarted { .. } => {
        self.clear();
        self.events.push(json!(event));
      }
      SessionEvent::ResponseAccepted { .. }
      | SessionEvent::ResponseInterrupted(_)
      | SessionEvent::Finished(_) => self.clear(),
      SessionEvent::ModelStream(stream) => {
        match stream {
          StreamEvent::Checkpoint { revision } => {
            self.checkpoint = Some((*revision, self.events.clone(), self.positions.clone()));
            return;
          }
          StreamEvent::Rewind { revision } => {
            if let Some((saved, events, positions)) = &self.checkpoint
              && saved == revision
            {
              self.events = events.clone();
              self.positions = positions.clone();
            }
            return;
          }
          StreamEvent::Recovering { .. } => {
            let key = "recovery".to_owned();
            if let Some(position) = self.positions.get(&key) {
              self.events[*position] = json!(event);
            } else {
              self.positions.insert(key, self.events.len());
              self.events.push(json!(event));
            }
            return;
          }
          _ => {}
        }
        let (kind, index, field, fragment) = match stream {
          StreamEvent::TextDelta { index, delta } => ("TextDelta", *index, "delta", delta),
          StreamEvent::ReasoningDelta { index, delta } => {
            ("ReasoningDelta", *index, "delta", delta)
          }
          StreamEvent::ReasoningDisplayDelta { index, delta } => {
            ("ReasoningDisplayDelta", *index, "delta", delta)
          }
          StreamEvent::ToolUseDelta { index, arguments, .. } => {
            ("ToolUseDelta", *index, "arguments", arguments)
          }
          StreamEvent::Usage(_) => {
            let value = json!(event);
            if let Some(position) = self.positions.get("usage") {
              self.events[*position] = value;
            } else {
              self.positions.insert("usage".into(), self.events.len());
              self.events.push(value);
            }
            return;
          }
          _ => return,
        };
        if let Some(position) = self.positions.remove("recovery") {
          self.events.remove(position);
          for index in self.positions.values_mut() {
            if *index > position {
              *index -= 1;
            }
          }
        }
        let key = format!("{kind}:{index}");
        if let Some(position) = self.positions.get(&key) {
          let value = &mut self.events[*position]["ModelStream"][kind];
          // Move the string out instead of copying the whole accumulated prefix per chunk.
          let mut text = match value[field].take() {
            Value::String(text) => text,
            _ => String::new(),
          };
          text.push_str(fragment);
          value[field] = json!(text);
          if let StreamEvent::ToolUseDelta { name, call_id, .. } = stream {
            if name.is_some() {
              value["name"] = json!(name);
            }
            if call_id.is_some() {
              value["call_id"] = json!(call_id);
            }
          }
        } else {
          self.positions.insert(key, self.events.len());
          self.events.push(json!(event));
        }
      }
      _ => {}
    }
  }
}

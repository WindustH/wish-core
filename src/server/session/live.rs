//! Coalesced current-turn preview. No token/chunk log, and no disk persistence.
use crate::{protocol::StreamEvent, session::SessionEvent};
use serde_json::{Value, json};
use std::collections::HashMap;

#[derive(Default)]
pub(super) struct LivePreview {
  pub revision: u64,
  events: Vec<Value>,
  positions: HashMap<String, usize>,
}
impl LivePreview {
  pub fn snapshot(&self, data: Value) -> Value {
    json!({"type":"snapshot", "data":data, "revision":self.revision, "live_events":self.events})
  }
  fn clear(&mut self) {
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

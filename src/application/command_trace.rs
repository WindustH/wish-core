//! Durable timeline observations for commands, without adding synthetic model messages.
use super::{
  error::{ApplicationError, blocking},
  session::SessionSlot,
};
use crate::session::{ToolCall, ToolOutcome};
use serde_json::{Value, json};
use std::sync::Arc;

pub struct CommandTrace {
  slot: Arc<SessionSlot>,
  call: ToolCall,
  finished: bool,
  tasks: tokio_util::task::TaskTracker,
}
async fn record(slot: &Arc<SessionSlot>, value: Value) -> Result<(), ApplicationError> {
  let sender = slot.sender.clone();
  blocking(move || Ok(sender.record_application_event(value)?)).await?;
  let _ = slot.events.send(json!({"type":"history_changed"}));
  Ok(())
}
impl CommandTrace {
  pub async fn start(
    app: &super::app::App,
    slot: Arc<SessionSlot>,
    call: ToolCall,
  ) -> Result<Self, ApplicationError> {
    record(&slot,json!({"type":"command_tool_use","call_id":call.call_id,"name":call.name,"arguments":call.arguments})).await?;
    Ok(Self { slot, call, finished: false, tasks: app.lifecycle.tasks.clone() })
  }
  pub async fn finish(mut self, outcome: &ToolOutcome) -> Result<(), ApplicationError> {
    let metadata = outcome.metadata();
    record(&self.slot,json!({"type":"command_tool_result","call_id":self.call.call_id,"name":self.call.name,"content":outcome.encode_content(),"metadata":metadata})).await?;
    self.finished = true;
    Ok(())
  }
}
impl Drop for CommandTrace {
  fn drop(&mut self) {
    if self.finished {
      return;
    }
    let slot = self.slot.clone();
    let value = json!({"type":"command_tool_result","call_id":self.call.call_id,"name":self.call.name,"content":{"status":"cancelled"}});
    self.tasks.spawn(async move {
      let _ = record(&slot, value).await;
    });
  }
}

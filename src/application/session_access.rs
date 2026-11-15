//! Shared session capability: direct model tool and traced shell bridge commands.
use super::{
  app::App,
  bridge_access,
  command_trace::CommandTrace,
  error::ApplicationError,
  peer_commands::{self as peers, Changes, NewSession, Send},
  session::SessionSlot,
};
use crate::{
  protocol::Tool,
  session::{ToolCall, ToolOutcome},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;

#[derive(Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum SessionRequest {
  List,
  Show { target: String },
  Create { input: NewSession },
  Configure { target: String, input: Changes },
  Delete { target: String },
  Send { target: String, input: Send },
}
pub fn specification() -> Tool {
  Tool {
    name: "session".into(),
    description: "Manage sessions: list, show, create, configure, delete, send. Targets are session names, IDs, or environment aliases such as $WISH_SESSION_ID_1; $WISH_SESSION_ID_0 is yourself. Only configure yourself or sessions you created; only delete sessions you created. send delivers text directly to the target session's input queue and wakes it; no group or separate transcript is created. Replies are not forwarded automatically: use send to reply. Use input for action-specific fields: create {name,instructions,model,provider,cwd,shell}; configure {name,model,provider,instructions}; send {text}.".into(),
    input_schema: json!({"type":"object","properties":{
      "action":{"type":"string","enum":["list","show","create","configure","delete","send"]},
      "target":{"type":"string","description":"Session name or ID"},
      "input":{"type":"object","properties":{
        "name":{"type":"string"},"instructions":{"type":"string"},"model":{"type":"string"},
        "provider":{"type":"string"},"cwd":{"type":"string"},"shell":{"type":"boolean"},
        "text":{"type":"string","x-preserve-whitespace":true}
      },"additionalProperties":false}
    },"required":["action"],"additionalProperties":false}),
  }
}
pub fn parse_request(arguments: &Value) -> Result<SessionRequest, ApplicationError> {
  let invalid = |reason: String| {
    ApplicationError::invalid(crate::tool::arguments::error(
      "session",
      reason,
      r#"{"action":"list"}"#,
    ))
  };
  let normalized =
    crate::tool::arguments::normalize(arguments, &specification().input_schema).map_err(invalid)?;
  serde_json::from_value(normalized).map_err(|error| invalid(error.to_string()))
}
pub async fn execute(
  app: Arc<App>,
  slot: Arc<SessionSlot>,
  mut request: SessionRequest,
) -> Result<Value, ApplicationError> {
  match &mut request {
    SessionRequest::Show { target }
    | SessionRequest::Configure { target, .. }
    | SessionRequest::Delete { target }
    | SessionRequest::Send { target, .. } => *target = slot.aliases.expand(target)?,
    _ => {}
  }
  app.lifecycle.require_open()?;
  slot.require_live()?;
  let id = slot.get_descriptor().id;
  match request {
    SessionRequest::List => peers::list_inner(app, id, slot).await,
    SessionRequest::Show { target } => peers::show_inner(app, (id, target), slot).await,
    SessionRequest::Create { input } => peers::create_inner(app, id, slot, input).await,
    SessionRequest::Configure { target, input } => {
      peers::configure_inner(app, (id, target), slot, input).await
    }
    SessionRequest::Delete { target } => {
      peers::delete_inner(app, (id, target), slot).await.map(|()| json!({"deleted":true}))
    }
    SessionRequest::Send { target, input } => {
      peers::send_inner(app, (id, target), slot, input).await
    }
  }
}
async fn command(
  app: Arc<App>,
  id: String,
  token: Option<&str>,
  request: SessionRequest,
) -> Result<Value, ApplicationError> {
  let slot = bridge_access::authenticate_session(&app, &id, token).await?;
  if !slot.get_descriptor().tools.shell {
    return Err(ApplicationError::conflict("shell is disabled for this session"));
  }
  let call = ToolCall {
    call_id: uuid::Uuid::new_v4().to_string(),
    name: "session".into(),
    arguments: serde_json::to_value(&request).map_err(ApplicationError::internal)?,
  };
  let trace = CommandTrace::start(&app, slot.clone(), call).await?;
  let result = app.lifecycle.until_shutdown(execute(app.clone(), slot.clone(), request)).await?;
  let outcome = match &result {
    Ok(value) => ToolOutcome::Success(value.clone()),
    Err(error) => ToolOutcome::Failed(error.to_string()),
  };
  let outcome = super::env_aliases::shorten_outcome(&slot, outcome).await;
  trace.finish(&outcome).await?;
  match result {
    Ok(value) => Ok(match outcome {
      ToolOutcome::SuccessWithProjection { model_output, .. } => model_output,
      _ => value,
    }),
    Err(error) => Err(error),
  }
}

pub async fn list(
  app: Arc<App>,
  id: String,
  token: Option<&str>,
) -> Result<Value, ApplicationError> {
  command(app, id, token, SessionRequest::List).await
}

pub async fn show(
  app: Arc<App>,
  (id, target): (String, String),
  token: Option<&str>,
) -> Result<Value, ApplicationError> {
  command(app, id, token, SessionRequest::Show { target }).await
}

pub async fn create(
  app: Arc<App>,
  id: String,
  token: Option<&str>,
  input: NewSession,
) -> Result<Value, ApplicationError> {
  command(app, id, token, SessionRequest::Create { input }).await
}

pub async fn configure(
  app: Arc<App>,
  (id, target): (String, String),
  token: Option<&str>,
  input: Changes,
) -> Result<Value, ApplicationError> {
  command(app, id, token, SessionRequest::Configure { target, input }).await
}

pub async fn delete(
  app: Arc<App>,
  (id, target): (String, String),
  token: Option<&str>,
) -> Result<(), ApplicationError> {
  command(app, id, token, SessionRequest::Delete { target }).await.map(|_| ())
}

pub async fn send(
  app: Arc<App>,
  (id, target): (String, String),
  token: Option<&str>,
  input: Send,
) -> Result<Value, ApplicationError> {
  command(app, id, token, SessionRequest::Send { target, input }).await
}

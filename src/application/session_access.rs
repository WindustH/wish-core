//! Shared session capability: direct model tool and traced shell bridge commands.
use super::{
  app::App,
  bridge_access,
  command_trace::CommandTrace,
  error::ApplicationError,
  peer_commands::{self as peers, AddMembers, Changes, NewGroup, NewSession, Send},
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
  CreateGroup { input: NewGroup },
  AddMembers { target: String, input: AddMembers },
  Leave { target: String },
}
pub fn specification() -> Tool {
  Tool {
    name: "session".into(),
    description: "Manage sessions and groups: list, show, create, configure, delete, send, create_group, add_members, leave. Targets are names or IDs. Only configure yourself or sessions you created; only delete sessions you created. Sending to a group requires membership and wakes its other members. A reply in your conversation is not a group message; use send. Use input for action-specific fields: create {name,instructions,model,provider,cwd,shell}; configure {name,model,provider,instructions}; send {text}; create_group {name,members}; add_members {add}.".into(),
    input_schema: json!({"type":"object","properties":{
      "action":{"type":"string","enum":["list","show","create","configure","delete","send","create_group","add_members","leave"]},
      "target":{"type":"string","description":"Session or group name or ID"},
      "input":{"type":"object","properties":{
        "name":{"type":"string"},"instructions":{"type":"string"},"model":{"type":"string"},
        "provider":{"type":"string"},"cwd":{"type":"string"},"shell":{"type":"boolean"},
        "text":{"type":"string","x-preserve-whitespace":true},
        "members":{"type":"array","items":{"type":"string"}},"add":{"type":"array","items":{"type":"string"}}
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
  request: SessionRequest,
) -> Result<Value, ApplicationError> {
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
    SessionRequest::CreateGroup { input } => peers::create_group_inner(app, id, slot, input).await,
    SessionRequest::AddMembers { target, input } => {
      peers::add_members_inner(app, (id, target), slot, input).await
    }
    SessionRequest::Leave { target } => {
      peers::leave_inner(app, (id, target), slot).await.map(|()| json!({"left":true}))
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
  let result = execute(app, slot, request).await;
  let outcome = match &result {
    Ok(value) => ToolOutcome::Success(value.clone()),
    Err(error) => ToolOutcome::Failed(error.to_string()),
  };
  trace.finish(&outcome).await?;
  result
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

pub async fn create_group(
  app: Arc<App>,
  id: String,
  token: Option<&str>,
  input: NewGroup,
) -> Result<Value, ApplicationError> {
  command(app, id, token, SessionRequest::CreateGroup { input }).await
}

pub async fn add_members(
  app: Arc<App>,
  (id, target): (String, String),
  token: Option<&str>,
  input: AddMembers,
) -> Result<Value, ApplicationError> {
  command(app, id, token, SessionRequest::AddMembers { target, input }).await
}

pub async fn leave(
  app: Arc<App>,
  (id, target): (String, String),
  token: Option<&str>,
) -> Result<(), ApplicationError> {
  command(app, id, token, SessionRequest::Leave { target }).await.map(|_| ())
}

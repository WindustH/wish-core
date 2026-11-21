//! Wire contracts that can refer to an earlier model response. Provider configuration
//! selects the contract explicitly because an endpoint accepting the same JSON schema
//! need not retain responses.
use super::{
  ModelUseProtocol,
  mode::{MessagesApiCompatMode, ResponsesDeployment},
};
use crate::protocol::error::Error;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IncrementalProtocol {
  Disabled,
  Responses,
  ResponsesWebsocket,
  OpenaiConversations,
  ArkResponses,
  AnthropicThreads,
  GoogleInteractions,
  MistralConversations,
}

impl IncrementalProtocol {
  pub fn validate(self, protocol: ModelUseProtocol) -> Result<(), Error> {
    if self == Self::Disabled {
      return Ok(());
    }
    let valid = match protocol {
      ModelUseProtocol::OpenAiResponses(mode) => {
        self == Self::ResponsesWebsocket
          || (mode.deployment != ResponsesDeployment::Codex
            && matches!(self, Self::Responses | Self::ArkResponses | Self::OpenaiConversations))
      }
      ModelUseProtocol::AnthropicMessages(MessagesApiCompatMode::Official) => {
        self == Self::AnthropicThreads
      }
      ModelUseProtocol::GoogleInteractions => self == Self::GoogleInteractions,
      ModelUseProtocol::MistralConversations => self == Self::MistralConversations,
      _ => false,
    };
    if valid {
      Ok(())
    } else {
      Err(Error::Build(format!("{self:?} cannot continue {}", protocol.get_name())))
    }
  }

  pub fn items(self) -> &'static str {
    match self {
      Self::MistralConversations => "inputs",
      Self::AnthropicThreads => "messages",
      _ => "input",
    }
  }

  pub fn apply(self, body: &mut Value, path: &mut String, previous: Option<&str>) {
    match self {
      Self::AnthropicThreads => {
        body["thread"] = match previous {
          Some(id) => {
            let object = body.as_object_mut().unwrap();
            object.remove("tools");
            object.remove("system");
            json!({"type":"continue","previous_message_id":id})
          }
          None => json!({"type":"create"}),
        };
      }
      Self::OpenaiConversations => {
        if let Some(id) = previous {
          body["conversation"] = json!(id);
        }
      }
      Self::MistralConversations => {
        body["store"] = json!(true);
        if let Some(id) = previous {
          path.push('/');
          path.push_str(id);
          let object = body.as_object_mut().unwrap();
          for key in ["model", "tools", "instructions"] {
            object.remove(key);
          }
        }
      }
      other => {
        body["store"] = json!(other != Self::ResponsesWebsocket);
        if other == Self::ArkResponses {
          body["caching"] = json!({"type":"enabled"});
        }
        if let Some(id) = previous {
          let field = if other == Self::GoogleInteractions {
            "previous_interaction_id"
          } else {
            "previous_response_id"
          };
          body[field] = json!(id);
        }
      }
    }
  }

  pub fn response_id(self, body: &Value, stream: bool) -> Option<&str> {
    let key = match (self, stream) {
      (Self::MistralConversations, _) => "/conversation_id",
      (Self::OpenaiConversations, false) => "/conversation/id",
      (Self::OpenaiConversations, true) => "/response/conversation/id",
      (_, false) => "/id",
      (Self::AnthropicThreads, true) => "/message/id",
      (Self::GoogleInteractions, true) => "/interaction/id",
      (_, true) => "/response/id",
    };
    body.pointer(key)?.as_str().filter(|id| {
      !id.is_empty()
        && id.len() <= 512
        && !id.chars().any(char::is_control)
        && (self != Self::MistralConversations
          || id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_')))
    })
  }

  pub fn stale(self, error: &Error) -> bool {
    let Error::Upstream { status, code, message, .. } = error else { return false };
    if matches!(self, Self::AnthropicThreads | Self::MistralConversations) && *status == Some(404) {
      return true;
    }
    if !matches!(status, None | Some(400 | 404 | 409 | 410)) {
      return false;
    }
    let text = format!("{code:?} {message}").to_ascii_lowercase();
    if self == Self::AnthropicThreads
      && ["thread_fingerprint_mismatch"].iter().any(|s| text.contains(s))
    {
      return true;
    }
    (text.contains("not found")
      || text.contains("not_found")
      || text.contains("expired")
      || text.contains("does not exist"))
      && ["previous", "conversation", "interaction", "thread", "message id", "response id"]
        .iter()
        .any(|key| text.contains(key))
  }
}

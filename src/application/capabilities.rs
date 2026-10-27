//! Session capability settings and policy; identities live in the neutral capability catalog.
use crate::capability::Capability;
use serde::{Deserialize, Serialize};

/// How an enabled capability is presented to the model.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolDelivery {
  // Compatibility for stored sessions without a delivery field; new-session defaults live in config.
  #[default]
  Tool,
  Command,
}
fn enabled_by_default() -> bool {
  true
}
fn command_delivery() -> ToolDelivery {
  ToolDelivery::Command
}

// One field declaration defines the durable settings, optional patch, and conversions.
// Preserve existing serde names/defaults: older session records use this exact shape.
macro_rules! tool_settings {
  ($( $(#[$attribute:meta])* pub $name:ident: $kind:ty, )*) => {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
    #[serde(deny_unknown_fields)]
    pub struct ToolSwitches { $( $(#[$attribute])* pub $name: $kind, )* }
    #[derive(Clone, Copy, Default, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct ToolChanges { $( pub $name: Option<$kind>, )* }
    impl ToolSwitches {
      pub fn apply_changes(self, changes: ToolChanges) -> Self {
        Self { $( $name: changes.$name.unwrap_or(self.$name), )* }
      }
    }
    impl From<ToolSwitches> for ToolChanges {
      fn from(switches: ToolSwitches) -> Self { Self { $( $name: Some(switches.$name), )* } }
    }
  }
}
tool_settings! {

  #[serde(default = "enabled_by_default")]
  pub history_search: bool,
  #[serde(default)]
  pub history_search_delivery: ToolDelivery,
  /// The simple `shell` tool and session task commands.
  pub shell: bool,
  #[serde(default = "command_delivery")]
  pub task_delivery: ToolDelivery,
  #[serde(default)]
  pub web_fetch_delivery: ToolDelivery,
  #[serde(default)]
  pub web_search_delivery: ToolDelivery,
  #[serde(default)]
  pub ask_user_delivery: ToolDelivery,
  /// Questions to the user: `ask_user`.
  pub ask_user: bool,
  /// Whether `wish mcp` in the session's shell may reach the MCP servers. The model's request is
  /// the same either way, so switching it keeps the prompt cache.
  pub mcp: bool,
  /// Searches of the web: `web_search`, answered by the configured search providers.
  pub web_search: bool,
  #[serde(default)]
  pub web_fetch: bool,
  /// Whether `wish skill` in the session's shell may read skills. Like `mcp`, it changes neither
  /// the tools nor the instructions, so switching it keeps the prompt cache.
  pub skills: bool,
  /// Whether `wish session` in the session's shell may see, make and message other sessions and
  /// groups, or use the equivalent direct session tool.
  pub sessions: bool,
  #[serde(default = "command_delivery")]
  pub sessions_delivery: ToolDelivery,
}
impl ToolSwitches {
  /// Whether a tool of this name runs under these switches.
  pub(crate) fn allow(self, name: &str) -> bool {
    Capability::from_tool(name).is_some_and(|capability| self.enabled(capability))
  }
  pub(crate) fn enabled(self, capability: Capability) -> bool {
    match capability {
      Capability::Image => true,
      Capability::History => self.history_search,
      Capability::Shell | Capability::Task => self.shell,
      Capability::Sessions => self.sessions,
      Capability::AskUser => self.ask_user,
      Capability::WebFetch => self.web_fetch,
      Capability::WebSearch => self.web_search,
    }
  }
  pub(crate) fn delivery(self, capability: Capability) -> ToolDelivery {
    match capability {
      Capability::History => self.history_search_delivery,
      Capability::Sessions => self.sessions_delivery,
      Capability::Task => self.task_delivery,
      Capability::AskUser => self.ask_user_delivery,
      Capability::WebFetch => self.web_fetch_delivery,
      Capability::WebSearch => self.web_search_delivery,
      Capability::Image | Capability::Shell => ToolDelivery::Tool,
    }
  }
  pub(crate) fn advertised(self, capability: Capability) -> bool {
    self.enabled(capability) && (!self.shell || self.delivery(capability) == ToolDelivery::Tool)
  }
}

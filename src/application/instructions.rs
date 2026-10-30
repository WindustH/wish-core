//! Builds and persists the application agent instructions for each generation.
use crate::protocol::{ContentBlock, Message};
const AGENT_INSTRUCTIONS: &str =
  "You are the agent operating this session. Use the provided tools to complete the user's task.";
/// Instructions follow delivery settings, so command-backed capabilities are discoverable
/// without advertising a second route for capabilities already supplied as tools.
pub fn agent_instructions(switches: super::session::ToolSwitches) -> String {
  use super::session::ToolDelivery;
  let mut text = AGENT_INSTRUCTIONS.to_owned();
  if switches.shell {
    let mut commands = vec!["`wish mcp` — MCP services", "`wish skill` — discover/read skills"];
    use crate::capability::Capability;
    for capability in [
      Capability::Sessions,
      Capability::Task,
      Capability::WebFetch,
      Capability::ImageGenerate,
      Capability::VideoGenerate,
      Capability::AudioGenerate,
      Capability::AudioTranscribe,
      Capability::WebSearch,
      Capability::History,
      Capability::AskUser,
    ] {
      if switches.enabled(capability)
        && switches.delivery(capability) == ToolDelivery::Command
        && let Some(instruction) = capability.command_instruction()
      {
        commands.push(instruction);
      }
    }
    text.push_str("\n\n# Wish commands\n");
    text.push_str(&commands.join("\n"));
    text.push_str(
      "\nRun `wish <command> --help` for full usage. Availability follows session settings.",
    );
  }
  text.push_str(&extension_instructions(switches));
  text
}

/// Discovery follows each capability's effective route, including mixed delivery and
/// the direct-tool fallback when the session shell is disabled.
fn extension_instructions(switches: super::session::ToolSwitches) -> String {
  use crate::capability::Capability;
  let mut direct = Vec::new();
  let mut commands = Vec::new();
  for (capability, name) in [
    (Capability::ImageGenerate, "image generation"),
    (Capability::VideoGenerate, "video generation"),
    (Capability::AudioGenerate, "audio generation"),
    (Capability::AudioTranscribe, "audio transcription"),
    (Capability::WebFetch, "webpage reading"),
    (Capability::WebSearch, "web search"),
  ] {
    if switches.enabled(capability) {
      if switches.advertised(capability) {
        direct.push(name);
      } else {
        commands.push(name);
      }
    }
  }
  if direct.is_empty() && commands.is_empty() {
    return String::new();
  }
  let mut text = String::from(
    "\n\n# Extension tools\nDiscover available tools before using an extension. Search returns tool names, tags and configured services; inspect returns the selected tool's schema and examples. Search without tags to see all available tools. Do not guess tool names or arguments. Availability follows this session's permissions and configured services.",
  );
  if !direct.is_empty() {
    text.push_str(&format!(
      "\nDirect tools ({}): call `extension_tools` with {{\"action\":\"search\"}}, then {{\"action\":\"inspect\",\"tool\":\"TOOL\"}}. Invoke the discovered tool by its name using the inspected schema.",
      direct.join(", ")
    ));
  }
  if !commands.is_empty() {
    text.push_str(&format!(
      "\nWish commands in the session shell ({}): run `wish extension search [TAG ...]`, then `wish extension inspect TOOL`, then `wish extension call TOOL --json '<arguments>'` (or `--stdin`). Run `wish extension --help` for usage. These tools are available through shell commands even when they are absent from the direct tool list.",
      commands.join(", ")
    ));
  }
  text
}
pub fn apply_agent_instructions(
  messages: &mut Vec<Message>,
  switches: super::session::ToolSwitches,
) {
  replace_agent_instructions(messages, agent_instructions(switches));
}
fn replace_agent_instructions(messages: &mut Vec<Message>, text: String) {
  let mut found = false;
  for message in messages.iter_mut() {
    if let Message::System { metadata, content } = message
      && metadata["source"] == "wish_agent_instructions"
    {
      *content = vec![ContentBlock::Text { text: text.clone() }];
      found = true;
    }
  }
  if !found {
    messages.insert(
      0,
      Message::System {
        metadata: serde_json::json!({"source":"wish_agent_instructions"}),
        content: vec![ContentBlock::Text { text }],
      },
    );
  }
}
pub fn persist_agent_instructions(
  session: &mut crate::session::Session,
  config: crate::session::SessionConfig,
  switches: super::session::ToolSwitches,
) -> Result<(), crate::session::SessionError> {
  if session.has_deferred_instruction("wish_agent_instructions") {
    if serde_json::to_value(session.get_config()).ok() == serde_json::to_value(&config).ok() {
      return Ok(());
    }
    return session.set_config(config);
  }
  session.set_system_instruction_config(
    "wish_agent_instructions".into(),
    agent_instructions(switches),
    config,
  )
}

/// A separate managed prefix keeps environment updates independent of capability instructions.
pub fn environment_instruction(shell: &crate::task::ShellCommand, cwd: &std::path::Path) -> String {
  let kind = shell.program.file_name().unwrap_or_default().to_string_lossy();
  format!(
    "# Execution environment\nShell type: {kind}\nShell program: {}\nShell arguments: {}\nWorking directory: {}\nEach command starts in this working directory. Existing processes retain their original environment.",
    serde_json::json!(shell.program),
    serde_json::json!(shell.args),
    serde_json::json!(cwd)
  )
}

/// A tail notice explains the current routes even while the generation keeps its old prefix.
pub fn delivery_notice(
  previous: super::session::ToolSwitches,
  next: super::session::ToolSwitches,
) -> String {
  use super::session::ToolDelivery;
  let route = |enabled: bool, shell: bool, delivery: ToolDelivery| {
    if !enabled {
      "disabled"
    } else if shell && delivery == ToolDelivery::Command {
      "Wish command in shell"
    } else {
      "direct tool"
    }
  };
  let mut changes = Vec::new();
  for (name, was, enabled, old_delivery, delivery) in [
    (
      "history / wish history",
      previous.history_search,
      next.history_search,
      previous.history_search_delivery,
      next.history_search_delivery,
    ),
    (
      "session / wish session",
      previous.sessions,
      next.sessions,
      previous.sessions_delivery,
      next.sessions_delivery,
    ),
    ("task", previous.shell, next.shell, previous.task_delivery, next.task_delivery),
    (
      "video extensions / wish extension",
      previous.video_generate,
      next.video_generate,
      previous.video_generate_delivery,
      next.video_generate_delivery,
    ),
    (
      "transcription extensions / wish extension",
      previous.audio_transcribe,
      next.audio_transcribe,
      previous.audio_transcribe_delivery,
      next.audio_transcribe_delivery,
    ),
    (
      "audio extensions / wish extension",
      previous.audio_generate,
      next.audio_generate,
      previous.audio_generate_delivery,
      next.audio_generate_delivery,
    ),
    (
      "image extensions / wish extension",
      previous.image_generate,
      next.image_generate,
      previous.image_generate_delivery,
      next.image_generate_delivery,
    ),
    (
      "web fetch extensions / wish extension",
      previous.web_fetch,
      next.web_fetch,
      previous.web_fetch_delivery,
      next.web_fetch_delivery,
    ),
    (
      "web search extensions / wish extension",
      previous.web_search,
      next.web_search,
      previous.web_search_delivery,
      next.web_search_delivery,
    ),
    (
      "ask_user / wish askuser",
      previous.ask_user,
      next.ask_user,
      previous.ask_user_delivery,
      next.ask_user_delivery,
    ),
  ] {
    let old = route(was, previous.shell, old_delivery);
    let new = route(enabled, next.shell, delivery);
    if old != new {
      changes.push(format!("{name}: {old} -> {new}"));
    }
  }
  for (name, was, enabled) in [
    ("shell", previous.shell, next.shell),
    ("wish mcp", previous.mcp, next.mcp),
    ("wish skill", previous.skills, next.skills),
  ] {
    if was != enabled {
      changes.push(format!("{name}: {}", if enabled { "enabled" } else { "disabled" }));
    }
  }
  format!(
    "Tool availability changed. Follow these settings for subsequent calls, even if the initial instructions or retained tool definitions describe an earlier route:\n{}\nFor command usage, run `wish <command> --help`. The next generation will use updated initial instructions and tool definitions.",
    changes.join("\n")
  ) + &extension_instructions(next)
}

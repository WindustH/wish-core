//! 12 to 13: the Anthropic preset stops sending Claude Code's user agent. The API bills a call that
//! names it as Claude Code traffic, which an API key's credits do not pay for, so every such call
//! failed for want of credit. A provider made from the preset kept the header in its own `headers`,
//! where it would outlive the preset, so it goes from there too; the preset sends Wish's own now.

use super::Data;

pub const SUMMARY: &str = "stop sending Claude Code's user agent with an Anthropic API key";

pub fn apply(data: &mut Data) -> Result<(), String> {
  let Some(providers) = data.config.get_mut("providers").and_then(|p| p.as_object_mut()) else {
    return Ok(());
  };
  for provider in providers.values_mut() {
    if provider.get("preset").and_then(|p| p.as_str()) != Some("anthropic") {
      continue;
    }
    if let Some(headers) = provider.get_mut("headers").and_then(|h| h.as_object_mut()) {
      headers.retain(|name, value| {
        !(name.eq_ignore_ascii_case("user-agent")
          && value.as_str().is_some_and(|agent| agent.starts_with("claude-cli/")))
      });
    }
  }
  Ok(())
}

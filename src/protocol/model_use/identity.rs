//! Client identity headers required by upstream deployments.
use crate::protocol::endpoint::{CredentialField, Endpoint, SourceHeader, copilot_oauth};
pub const CODEX_USER_AGENT: &str = "codex_cli_rs/0.156.1";
pub fn apply_headers(
  mut endpoint: Endpoint,
  preset: Option<&str>,
  protocol: &str,
  user_agent: &str,
) -> Endpoint {
  match preset {
    Some("opencode_go") => {
      endpoint = endpoint
        .with_header("x-opencode-session", "{session}")
        .with_header("user-agent", user_agent);
    }
    Some("openai_codex") => {
      endpoint = endpoint
        .with_header("user-agent", CODEX_USER_AGENT)
        .with_header("session-id", "{session}")
        .with_header("thread-id", "{session}")
        .with_header("x-client-request-id", "{session}")
        .with_credential_header("chatgpt-account-id", CredentialField::AccountId);
    }
    Some("openai") => {
      endpoint = endpoint
        .with_header("user-agent", CODEX_USER_AGENT)
        .with_header("originator", "codex_cli_rs");
      if protocol == "openai_responses" {
        endpoint = endpoint
          .with_header("session-id", "{session}")
          .with_header("thread-id", "{session}")
          .with_header("x-client-request-id", "{session}");
      }
    }
    // Not Claude Code's own user agent: the API bills a call that names it as Claude Code
    // traffic, which an API key's credits do not pay for.
    Some("anthropic") => {
      endpoint = endpoint
        .with_header("user-agent", user_agent)
        .with_header("x-app", "cli")
        .with_header("x-claude-code-session-id", "{session}");
    }
    Some("google_antigravity") => {
      for header in crate::protocol::endpoint::antigravity::API_HEADERS {
        if let SourceHeader::Literal(name, value) = header {
          endpoint = endpoint.with_header(name, value);
        }
      }
      endpoint = endpoint.with_header("x-machine-session-id", "{session}");
    }
    Some("github_copilot") => {
      for header in copilot_oauth::API_HEADERS {
        if let SourceHeader::Literal(name, value) = header {
          endpoint = endpoint.with_header(name, value);
        }
      }
      endpoint = endpoint.with_header("openai-intent", "conversation-panel");
    }
    _ => {}
  }

  endpoint
}

//! Where each account protocol is read from: this tree's slice of the read-side table.
//!
//! Some protocols pin one host - every usage read here belongs to the service that reported it -
//! and some are implemented by many providers, so their host and path come from the caller's own
//! configuration and the entry only states what the wire requires. A passive protocol has no entry
//! at all: it is only ever read off a model call.
//!
//! The call an entry describes is built through [`Source`], with the caller's credentials.

use super::AccountStateProtocol;
use crate::protocol::endpoint::{
  AuthScheme, CredentialField, Source, SourceHeader, copilot_oauth::EDITOR_HEADERS,
};

/// The source that answers for an account protocol, or `None` for one that rides a served reply.
pub(crate) fn find_source(protocol: AccountStateProtocol) -> Option<Source> {
  let id = protocol.get_id();
  let at = |base_url, path| Source {
    protocol: id,
    base_url: Some(base_url),
    path: Some(path),
    auth: AuthScheme::Bearer(None),
    headers: &[],
  };
  Some(match protocol {
    AccountStateProtocol::OpenAiCodexQuotaHeaders
    | AccountStateProtocol::AnthropicRatelimitHeaders
    | AccountStateProtocol::OpenAiRatelimitHeaders
    | AccountStateProtocol::GroqRatelimitHeaders
    | AccountStateProtocol::CerebrasRatelimitHeaders
    | AccountStateProtocol::MistralRatelimitHeaders => return None,
    AccountStateProtocol::DeepseekUserBalance => at("https://api.deepseek.com", "/user/balance"),
    // Moonshot serves one API from two hosts: `api.moonshot.cn` for an account registered in the
    // mainland console, `api.moonshot.ai` for a global one. Which host an account lives on is the
    // caller's fact, so it arrives as a base URL override.
    AccountStateProtocol::KimiOpenBalance => at("https://api.moonshot.cn", "/v1/users/me/balance"),
    // The code service serves the plan of a coding subscription from a host of its own, and wants
    // the user agent its own CLI sends. `api.kimi.ai` answers the same path for a global account.
    AccountStateProtocol::KimiCodeCompanionUsage => Source {
      headers: &[
        SourceHeader::Literal("user-agent", "Kimi-Code/2.0.0"),
        SourceHeader::Literal("accept", "application/json"),
      ],
      ..at("https://api.kimi.com", "/coding/v1/usages")
    },
    // Two key families, two endpoints: a platform key (`sk-api-*`) reads the account balance, a
    // token-plan key (`sk-cp-*`) reads the plan's own windows.
    AccountStateProtocol::MinimaxAccountBalance => {
      at("https://api.minimaxi.com", "/account/query_balance")
    }
    AccountStateProtocol::MinimaxTokenPlanRemains => {
      at("https://api.minimaxi.com", "/v1/token_plan/remains")
    }
    // `open.bigmodel.cn` serves the same path to the mainland console: one API, two hosts.
    AccountStateProtocol::ZaiCodingPlanMonitor => {
      at("https://api.z.ai", "/api/monitor/usage/quota/limit")
    }
    // A mainland account reads the same path from `api.siliconflow.cn`'s global twin.
    AccountStateProtocol::SiliconflowBalance => at("https://api.siliconflow.cn", "/v1/user/info"),
    // A key reads its own quota here; the account-wide view of the same service is the credits
    // endpoint, which only a management key is allowed to read.
    AccountStateProtocol::OpenrouterKeyQuota => at("https://openrouter.ai", "/api/v1/auth/key"),
    AccountStateProtocol::OpenrouterCredits => at("https://openrouter.ai", "/api/v1/credits"),
    // The inference router is `router.huggingface.co`; the account this reads is the hub's own.
    AccountStateProtocol::HuggingfaceWhoamiBilling => {
      at("https://huggingface.co", "/api/whoami-v2")
    }
    // An enterprise workspace reads its own quota, and the workspace travels twice: the path
    // addresses it and a header names it.
    AccountStateProtocol::QwenWorkspaceQuota => Source {
      headers: &[SourceHeader::Credential("x-dashscope-workspace", CredentialField::WorkspaceId)],
      ..at("https://dashscope.aliyuncs.com", "/api/v1/workspaces/{workspace_id}/quota")
    },
    // Codex asks its account endpoint with the same bearer and the same account header as its
    // model calls.
    AccountStateProtocol::OpenAiCodexUsage => Source {
      headers: &[SourceHeader::Credential("chatgpt-account-id", CredentialField::AccountId)],
      ..at("https://chatgpt.com", "/backend-api/wham/usage")
    },
    // Copilot's allowances are GitHub's to tell: asked of its API with the GitHub token the
    // session is exchanged from, as the editor asks them.
    AccountStateProtocol::GitHubCopilotUsage => Source {
      auth: AuthScheme::GitHubToken,
      headers: EDITOR_HEADERS,
      ..at("https://api.github.com", "/copilot_internal/user")
    },
    // A gateway answers on the machine it runs on, and to the key of a gateway shared on its
    // network; one elsewhere arrives as a base URL override.
    AccountStateProtocol::MagpieQuotas => at("http://127.0.0.1:3425", "/v1/magpie/quotas"),
  })
}

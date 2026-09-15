//! Where each account protocol is read from: this tree's slice of the read-side table.
//!
//! Some protocols pin one host - every usage read here belongs to the service that reported it -
//! and some are implemented by many providers, so their host and path come from the caller's own
//! configuration and the entry only states what the wire requires. A passive protocol has no entry
//! at all - it is only ever read off a model call - and neither does one whose endpoint is not
//! pinned down yet.
//!
//! The call an entry describes is built by [`Source`](crate::protocol::outbound::Source), with the
//! caller's credentials.

use super::AccountStateProtocol;
use crate::protocol::outbound::{AuthProtocol, CredentialField};
use crate::protocol::outbound::{Source, SourceHeader};

/// The source that answers for an account protocol, when there is one.
pub(crate) fn source(protocol: AccountStateProtocol) -> Option<&'static Source> {
  SOURCES.iter().find(|source| source.protocol == protocol.id())
}

/// Every account read this crate knows: one entry per readable protocol id.
const SOURCES: &[Source] = &[
  Source {
    protocol: AccountStateProtocol::DeepseekUserBalance.id(),
    base_url: Some("https://api.deepseek.com"),
    path: Some("/user/balance"),
    auth: AuthProtocol::Bearer(None),
    headers: &[],
  },
  // Moonshot serves one API from two hosts: `api.moonshot.cn` for an account registered in the
  // mainland console, `api.moonshot.ai` for a global one. Which host an account lives on is the
  // caller's fact, so it arrives as a base URL override.
  Source {
    protocol: AccountStateProtocol::KimiOpenBalance.id(),
    base_url: Some("https://api.moonshot.cn"),
    path: Some("/v1/users/me/balance"),
    auth: AuthProtocol::Bearer(None),
    headers: &[],
  },
  // The code service serves the plan of a coding subscription from a host of its own, and wants the
  // user agent its own CLI sends. `api.kimi.ai` answers the same path for a global account.
  Source {
    protocol: AccountStateProtocol::KimiCodeCompanionUsage.id(),
    base_url: Some("https://api.kimi.com"),
    path: Some("/coding/v1/usages"),
    auth: AuthProtocol::Bearer(None),
    headers: &[
      SourceHeader::Literal("user-agent", "Kimi-Code/2.0.0"),
      SourceHeader::Literal("accept", "application/json"),
    ],
  },
  // Two key families, two endpoints: a platform key (`sk-api-*`) reads the account balance, a
  // token-plan key (`sk-cp-*`) reads the plan's own windows.
  Source {
    protocol: AccountStateProtocol::MinimaxAccountBalance.id(),
    base_url: Some("https://api.minimaxi.com"),
    path: Some("/account/query_balance"),
    auth: AuthProtocol::Bearer(None),
    headers: &[],
  },
  Source {
    protocol: AccountStateProtocol::MinimaxTokenPlanRemains.id(),
    base_url: Some("https://api.minimaxi.com"),
    path: Some("/v1/token_plan/remains"),
    auth: AuthProtocol::Bearer(None),
    headers: &[],
  },
  // `open.bigmodel.cn` serves the same path to the mainland console: one API, two hosts.
  Source {
    protocol: AccountStateProtocol::ZaiCodingPlanMonitor.id(),
    base_url: Some("https://api.z.ai"),
    path: Some("/api/monitor/usage/quota/limit"),
    auth: AuthProtocol::Bearer(None),
    headers: &[],
  },
  // A mainland account reads the same path from `api.siliconflow.cn`'s global twin.
  Source {
    protocol: AccountStateProtocol::SiliconflowBalance.id(),
    base_url: Some("https://api.siliconflow.cn"),
    path: Some("/v1/user/info"),
    auth: AuthProtocol::Bearer(None),
    headers: &[],
  },
  // A key reads its own quota here; the account-wide view of the same service is the credits
  // endpoint, which only a management key is allowed to read.
  Source {
    protocol: AccountStateProtocol::OpenrouterKeyQuota.id(),
    base_url: Some("https://openrouter.ai"),
    path: Some("/api/v1/auth/key"),
    auth: AuthProtocol::Bearer(None),
    headers: &[],
  },
  Source {
    protocol: AccountStateProtocol::OpenrouterCredits.id(),
    base_url: Some("https://openrouter.ai"),
    path: Some("/api/v1/credits"),
    auth: AuthProtocol::Bearer(None),
    headers: &[],
  },
  // The inference router is `router.huggingface.co`; the account this reads is the hub's own.
  Source {
    protocol: AccountStateProtocol::HuggingfaceWhoamiBilling.id(),
    base_url: Some("https://huggingface.co"),
    path: Some("/api/whoami-v2"),
    auth: AuthProtocol::Bearer(None),
    headers: &[],
  },
  // An enterprise workspace reads its own quota, and the workspace travels twice: the path
  // addresses it and a header names it.
  Source {
    protocol: AccountStateProtocol::QwenWorkspaceQuota.id(),
    base_url: Some("https://dashscope.aliyuncs.com"),
    path: Some("/api/v1/workspaces/{workspace_id}/quota"),
    auth: AuthProtocol::Bearer(None),
    headers: &[SourceHeader::Credential("x-dashscope-workspace", CredentialField::WorkspaceId)],
  },
  // Codex asks its account endpoint with the same bearer and the same account header as its model
  // calls.
  Source {
    protocol: AccountStateProtocol::OpenAiCodexUsage.id(),
    base_url: Some("https://chatgpt.com"),
    path: Some("/backend-api/wham/usage"),
    auth: AuthProtocol::Bearer(None),
    headers: &[SourceHeader::Credential("chatgpt-account-id", CredentialField::AccountId)],
  },
];

//! What an account has left, in one shape across providers.
//!
//! A service reports its account state in its own vocabulary: a balance in a currency, a token plan
//! with a weekly window, a request rate limit, an unlimited flag. This module is the shape those
//! bodies are read into, so a caller can show or act on usage without knowing which service
//! produced it.
//!
//! Two rules run through every field:
//!
//! - Numbers keep the exact form the service reported, as decimal strings. A balance re-encoded
//!   through a float can lose the cent that matters, and a zero that was invented is worse than a
//!   missing value.
//! - What cannot be represented is kept as text in [`AccountState::warnings`] rather than
//!   guessed at, and an unlimited allowance is never flattened into a number.
//! - A failure the service reports inside a successful body - a rejected key, an account in
//!   arrears, a plan out of quota - is read into [`AccountState::failure`], which keeps the
//!   service's own code and message and adds the one part a caller can act on without knowing the
//!   service.
//!
//! Each service's own reading of its body lives in a module below this one, and
//! [`parse_account_body`] picks the one a protocol id names. Where each protocol is read from is
//! the same knowledge: `source.rs` is this tree's slice of the read-side table. A rate limit a
//! service reports only on a call it really served has no body of its own: [`parse_headers`]
//! reads those off the reply's headers instead.

use serde_json::Value;

use crate::Error;

pub mod codex;
pub mod deepseek;
pub mod fetch;
pub mod headers;
pub mod huggingface;
pub mod kimi;
pub mod minimax;
pub mod openrouter;
pub mod qwen;
pub mod siliconflow;
mod source;
pub mod zai;

pub use fetch::fetch;

/// One allowance window: a limit, and how much of it is spent.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct QuotaWindow {
  /// Stable window id, e.g. `primary`, `usage`, `weekly`, `model_1`.
  pub id: String,
  /// Display name, when the service reports one.
  pub name: Option<String>,
  /// What the amounts count: `tokens`, `requests`, `credits`, `time`, `currency_minor` (minor
  /// units of the service's currency) or `unknown`; a service that counts something of its own
  /// names it (`usd`), and a window that reports no amounts at all stays `unknown`.
  pub unit: String,
  /// Amount spent, in the service's own form.
  pub used: Option<String>,
  /// The allowance, in the service's own form.
  pub limit: Option<String>,
  /// What is left, when the service reports it directly.
  pub remaining: Option<String>,
  /// Percentage spent, when the service reports it directly or as the share that is left; never
  /// back-computed from the amounts.
  pub used_percent: Option<String>,
  /// The window's own shape, kept as it came (a duration, or nested sub-windows).
  pub window: Option<Value>,
  /// When the window resets, as the service reported it.
  pub resets_at: Option<String>,
  /// Whether the allowance is spent, when the service says so.
  pub reached: Option<bool>,
  /// Whether the allowance is unlimited, which is not the same as a zero or missing limit.
  pub unlimited: Option<bool>,
}

/// One account balance.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Balance {
  /// Currency or credit unit as reported.
  pub currency: String,
  /// What is spendable; negative when the account owes.
  pub available: Option<String>,
  /// The total amount.
  pub total: Option<String>,
  /// The cash-paid portion.
  pub cash: Option<String>,
  /// The granted or discounted portion.
  pub granted: Option<String>,
  /// The topped-up portion.
  pub topped_up: Option<String>,
  /// The voucher portion.
  pub voucher: Option<String>,
  /// Credit line the service grants, when it reports one.
  pub credit: Option<String>,
  /// What the account owes the service, when it is in arrears.
  pub owed: Option<String>,
  /// Exponent of the minor unit, when amounts are reported in minor units.
  pub minor_unit: Option<u32>,
}

/// Why the service would not answer for this account, or would not serve a call for it.
///
/// The service's own code and message are kept verbatim; the kind is what was added, so a caller
/// can tell "the key is wrong" from "the account is out of money" without a table of every
/// service's numbering.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Failure {
  /// The normalized kind.
  pub kind: FailureKind,
  /// The service's own code, exactly as it wrote it.
  pub code: Option<String>,
  /// The service's own message.
  pub message: String,
}

/// The kinds of failure worth telling apart.
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureKind {
  /// The credential is missing, wrong, or not accepted by this endpoint.
  Unauthorized,
  /// The account has nothing left to spend.
  Unpaid,
  /// The service is refusing requests for now: a rate limit, or a plan between windows.
  Throttled,
  /// A failure this shape does not know how to name.
  Unknown,
}

/// One service's account snapshot, as far as it reports any.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct AccountState {
  /// The protocol that read this body.
  pub protocol: AccountStateProtocol,
  /// Plan allowance windows.
  pub quotas: Vec<QuotaWindow>,
  /// Account balances.
  pub balances: Vec<Balance>,
  /// The failure the service reported inside this body, when it reported one.
  pub failure: Option<Failure>,
  /// Fields that were dropped, unknown or contradictory, kept as text.
  pub warnings: Vec<String>,
  /// Coarse availability, when the body says so.
  pub availability: Option<String>,
  /// Plan name, when the service reports one.
  pub plan_type: Option<String>,
}

/// Which account reading a body or a reply is read by.
///
/// One variant per service and shape: the vocabulary this crate knows. A caller names one to read an
/// account, and a reading names the one that produced it. [`AccountStateProtocol::get_id`] is the same name in
/// text, so the source tables and any configuration boundary can carry it and
/// [`FromStr`](std::str::FromStr) brings it back.
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccountStateProtocol {
  /// The numbers a served reply carries, where a caller asks for nothing else.
  ResponseUsage,
  /// What a Codex reply says about the account: its `x-codex-*` headers, or the payload of the
  /// rate-limit frame it sends.
  OpenAiCodexQuotaHeaders,
  /// The meters a Codex subscription's own account endpoint reports.
  OpenAiCodexUsage,
  /// Anthropic's rate-limit headers.
  AnthropicRatelimitHeaders,
  /// OpenAI's rate-limit headers.
  OpenAiRatelimitHeaders,
  /// Groq's rate-limit headers.
  GroqRatelimitHeaders,
  /// Cerebras' rate-limit headers.
  CerebrasRatelimitHeaders,
  /// Mistral's rate-limit headers.
  MistralRatelimitHeaders,
  /// The balance of a DeepSeek platform account.
  DeepseekUserBalance,
  /// The balance of a Moonshot open-platform account.
  KimiOpenBalance,
  /// The plan windows of a Kimi coding subscription.
  KimiCodeCompanionUsage,
  /// The plan a Z.ai coding subscription runs on.
  ZaiCodingPlanMonitor,
  /// The windows of a MiniMax token plan.
  MinimaxTokenPlanRemains,
  /// The balance of a MiniMax platform account.
  MinimaxAccountBalance,
  /// The balance of a SiliconFlow account.
  SiliconflowBalance,
  /// What one OpenRouter key has left.
  OpenrouterKeyQuota,
  /// What the OpenRouter account has left.
  OpenrouterCredits,
  /// The plan a Hugging Face account is on.
  HuggingfaceWhoamiBilling,
  /// The quota of one Qwen workspace.
  QwenWorkspaceQuota,
  /// The ledger this crate keeps itself, for a caller with no endpoint to read.
  LocalUsageLedger,
}

impl AccountStateProtocol {
  /// Every protocol this crate knows.
  pub const ALL: &[AccountStateProtocol] = &[
    AccountStateProtocol::ResponseUsage,
    AccountStateProtocol::OpenAiCodexQuotaHeaders,
    AccountStateProtocol::OpenAiCodexUsage,
    AccountStateProtocol::AnthropicRatelimitHeaders,
    AccountStateProtocol::OpenAiRatelimitHeaders,
    AccountStateProtocol::GroqRatelimitHeaders,
    AccountStateProtocol::CerebrasRatelimitHeaders,
    AccountStateProtocol::MistralRatelimitHeaders,
    AccountStateProtocol::DeepseekUserBalance,
    AccountStateProtocol::KimiOpenBalance,
    AccountStateProtocol::KimiCodeCompanionUsage,
    AccountStateProtocol::ZaiCodingPlanMonitor,
    AccountStateProtocol::MinimaxTokenPlanRemains,
    AccountStateProtocol::MinimaxAccountBalance,
    AccountStateProtocol::SiliconflowBalance,
    AccountStateProtocol::OpenrouterKeyQuota,
    AccountStateProtocol::OpenrouterCredits,
    AccountStateProtocol::HuggingfaceWhoamiBilling,
    AccountStateProtocol::QwenWorkspaceQuota,
    AccountStateProtocol::LocalUsageLedger,
  ];

  /// The name this protocol is known by in text: what the source tables, the documentation and a
  /// reading's own [`AccountState::protocol`] say.
  pub const fn get_id(self) -> &'static str {
    match self {
      AccountStateProtocol::ResponseUsage => "response_usage",
      AccountStateProtocol::OpenAiCodexQuotaHeaders => "openai_codex_quota_headers",
      AccountStateProtocol::OpenAiCodexUsage => "openai_codex_usage",
      AccountStateProtocol::AnthropicRatelimitHeaders => "anthropic_ratelimit_headers",
      AccountStateProtocol::OpenAiRatelimitHeaders => "openai_ratelimit_headers",
      AccountStateProtocol::GroqRatelimitHeaders => "groq_ratelimit_headers",
      AccountStateProtocol::CerebrasRatelimitHeaders => "cerebras_ratelimit_headers",
      AccountStateProtocol::MistralRatelimitHeaders => "mistral_ratelimit_headers",
      AccountStateProtocol::DeepseekUserBalance => "deepseek_user_balance",
      AccountStateProtocol::KimiOpenBalance => "kimi_open_balance",
      AccountStateProtocol::KimiCodeCompanionUsage => "kimi_code_companion_usage",
      AccountStateProtocol::ZaiCodingPlanMonitor => "zai_coding_plan_monitor",
      AccountStateProtocol::MinimaxTokenPlanRemains => "minimax_token_plan_remains",
      AccountStateProtocol::MinimaxAccountBalance => "minimax_account_balance",
      AccountStateProtocol::SiliconflowBalance => "siliconflow_balance",
      AccountStateProtocol::OpenrouterKeyQuota => "openrouter_key_quota",
      AccountStateProtocol::OpenrouterCredits => "openrouter_credits",
      AccountStateProtocol::HuggingfaceWhoamiBilling => "hf_whoami_billing",
      AccountStateProtocol::QwenWorkspaceQuota => "qwen_workspace_quota",
      AccountStateProtocol::LocalUsageLedger => "local_usage_ledger",
    }
  }

  /// Whether this protocol is only ever observed on a real model call, so asking for it on its own
  /// is a configuration mistake rather than a request.
  pub fn is_passive(self) -> bool {
    matches!(
      self,
      AccountStateProtocol::ResponseUsage
        | AccountStateProtocol::OpenAiCodexQuotaHeaders
        | AccountStateProtocol::LocalUsageLedger
    ) || headers::is_header_dialect(self)
  }
}

impl std::fmt::Display for AccountStateProtocol {
  fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    formatter.write_str(self.get_id())
  }
}

impl std::str::FromStr for AccountStateProtocol {
  type Err = Error;

  /// Reads back what [`AccountStateProtocol::get_id`] wrote, for a boundary that carries text.
  ///
  /// # Errors
  ///
  /// [`Error::Build`] for a name this crate does not know.
  fn from_str(id: &str) -> Result<Self, Self::Err> {
    Self::ALL
      .iter()
      .copied()
      .find(|protocol| protocol.get_id() == id)
      .ok_or_else(|| Error::Build(format!("unknown account protocol `{id}`")))
  }
}

/// Why an account state cannot be read where it was asked for.
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unsupported {
  /// The reading rides the headers of a served reply: there is no body of its own to parse, and
  /// no endpoint to ask.
  RidesTheHeaders,
  /// The reading rides a served reply's payload, or is kept by this crate's own ledger: it has
  /// no ask of its own anywhere.
  RidesAReply,
  /// The reading is served by a request of its own: a reply will not carry it.
  HasItsOwnRequest,
}

impl Unsupported {
  /// The reason in the words a caller reads back.
  pub const fn get_text(self) -> &'static str {
    match self {
      Unsupported::RidesTheHeaders => "is read from a reply's headers, not from a body of its own",
      Unsupported::RidesAReply => {
        "rides a served reply's payload or this crate's own ledger, and has no ask of its own"
      }
      Unsupported::HasItsOwnRequest => {
        "is read by a request of its own, which a reply will not carry"
      }
    }
  }
}

/// Reads an account body for a known protocol.
///
/// # Errors
///
/// Returns [`Error::Unsupported`] for a protocol that is read from a reply's headers instead of a
/// body of its own, and for the two whose numbers belong to a served reply or this crate's own
/// ledger; [`Error::Malformed`] when the body
/// is missing a container the protocol cannot work without, while a merely absent field stays absent,
/// with a warning.
pub fn parse_account_body(
  protocol: AccountStateProtocol,
  body: &Value,
) -> Result<AccountState, Error> {
  match protocol {
    AccountStateProtocol::DeepseekUserBalance => deepseek::parse(body),
    AccountStateProtocol::KimiOpenBalance => kimi::parse_balance(body),
    AccountStateProtocol::KimiCodeCompanionUsage => kimi::parse_companion(body),
    AccountStateProtocol::MinimaxTokenPlanRemains => minimax::parse_quota(body),
    AccountStateProtocol::MinimaxAccountBalance => Ok(minimax::parse_balance(body)),
    AccountStateProtocol::ZaiCodingPlanMonitor => Ok(zai::parse(body)),
    AccountStateProtocol::SiliconflowBalance => siliconflow::parse(body),
    AccountStateProtocol::OpenrouterKeyQuota => openrouter::parse_key(body),
    AccountStateProtocol::OpenrouterCredits => openrouter::parse_credits(body),
    AccountStateProtocol::HuggingfaceWhoamiBilling => huggingface::parse(body),
    AccountStateProtocol::QwenWorkspaceQuota => qwen::parse_workspace_quota(body),
    AccountStateProtocol::OpenAiCodexQuotaHeaders => codex::parse(body),
    AccountStateProtocol::OpenAiCodexUsage => Ok(codex::parse_usage(body)),
    AccountStateProtocol::AnthropicRatelimitHeaders
    | AccountStateProtocol::OpenAiRatelimitHeaders
    | AccountStateProtocol::GroqRatelimitHeaders
    | AccountStateProtocol::CerebrasRatelimitHeaders
    | AccountStateProtocol::MistralRatelimitHeaders => Err(Error::build_unsupported(
      "account state",
      protocol,
      Unsupported::RidesTheHeaders.get_text(),
    )),
    // Named, but nothing reads them on their own: the numbers they stand for belong to a served
    // reply, and a caller holding one reads it through the call it came from.
    AccountStateProtocol::ResponseUsage | AccountStateProtocol::LocalUsageLedger => {
      Err(Error::build_unsupported("account state", protocol, Unsupported::RidesAReply.get_text()))
    }
  }
}

/// Reads a reply's headers for a known protocol.
///
/// The passive dialects - the rate limits a service only reports on a call it really served - are
/// read from the reply's headers rather than from a body, and none of them has an endpoint to ask
/// on its own.
///
/// # Errors
///
/// Returns [`Error::Malformed`] for a protocol that is not read from headers.
pub fn parse_headers(
  protocol: AccountStateProtocol,
  headers: &[(String, String)],
) -> Result<AccountState, Error> {
  headers::parse(protocol, headers)
}

/// Reads the account reading a real call carries, when the protocol is one of those.
///
/// These readings have no request of their own: a service reports what a call left in the account
/// only on a call it really served, either in the reply's headers or, for
/// `openai_codex_quota_headers`, in the payload of a rate-limit frame the reply carried.
///
/// # Errors
///
/// [`Error::Unsupported`] when the protocol is read over a request of its own - this call site has
/// nothing to read it from - or when the reply carried none of what the protocol is read from.
pub fn parse_reply(
  protocol: AccountStateProtocol,
  headers: &[(String, String)],
  body: Option<&Value>,
) -> Result<AccountState, Error> {
  if headers::is_header_dialect(protocol) {
    return parse_headers(protocol, headers);
  }
  match protocol {
    // A codex reply carries its reading twice over: the `x-codex-*` headers ride every call, and
    // the payload a `response.rate_limits` frame carries says the same thing at more length. The
    // payload is preferred where there is one, and the head is what a stream has instead.
    AccountStateProtocol::OpenAiCodexQuotaHeaders => {
      match body.filter(|body| body.get("rate_limits").is_some()) {
        Some(body) => codex::parse(body),
        None => codex::from_headers(headers).ok_or_else(|| {
          Error::Build(
            "`openai_codex_quota_headers` is read from the `x-codex-*` headers of a reply or from the payload of its own rate-limit frame, and this reply carried neither"
              .to_owned(),
          )
        }),
      }
    }
    _ => Err(Error::build_unsupported(
      "account state",
      protocol,
      Unsupported::HasItsOwnRequest.get_text(),
    )),
  }
}

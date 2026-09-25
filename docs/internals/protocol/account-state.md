# Account state

`src/protocol/account_state/` is the intermediate layer for what a service says about the account
behind a key. It converts a body or a reply head into `AccountState`, so the UI can show quota and
balance without knowing which service produced it.

```
caller                      account                        service
──────────────────────────────────────────────────────────────────────────────
AccountState ◀── parse_reply() ◀──┬── headers.rs          ◀── reply head      (every model call)
                                  └── codex.rs            ◀── body rate_limits / x-codex-* head
AccountState ◀── fetch() ◀── parse_account_body() ◀── <service>.rs ◀── body ◀── the read's own GET
                    └──▶ source.rs entry × Credentials ──▶ dispatch ──▶ transport
```

The client calls `parse_reply` on every model call when a reply-borne protocol is configured, and
`fetch` from `Client::get_account_state` ([client](../client.md#calls)).

## Protocols

`AccountStateProtocol::get_id()` is the text form (`FromStr` reads it back; `ALL` lists them).

| Read from | Variants |
| --- | --- |
| reply headers (`headers.rs`) | `AnthropicRatelimitHeaders`, `OpenAiRatelimitHeaders`, `GroqRatelimitHeaders`, `CerebrasRatelimitHeaders`, `MistralRatelimitHeaders` |
| a reply's body or head | `OpenAiCodexQuotaHeaders`: a buffered body's `rate_limits`, else the `x-codex-*` headers. On a stream, the `response.rate_limits`/`codex.rate_limits` frame also arrives as `StreamEvent::Account` and replaces the reading |
| a request of its own (`source.rs`) | `OpenAiCodexUsage`, `DeepseekUserBalance`, `KimiOpenBalance`, `KimiCodeCompanionUsage`, `ZaiCodingPlanMonitor`, `MinimaxTokenPlanRemains`, `MinimaxAccountBalance`, `SiliconflowBalance`, `OpenrouterKeyQuota`, `OpenrouterCredits`, `HuggingfaceWhoamiBilling` (`hf_whoami_billing`), `QwenWorkspaceQuota` |
| nothing yet | `ResponseUsage`, `LocalUsageLedger`: named, with no reader |

Asking for a reading on the wrong side is `Error::Unsupported` with reason
`RidesTheHeaders`, `RidesAReply` or `HasItsOwnRequest`. On the reply side the error fails the model
call it rides. A Codex reply that carries neither a `rate_limits` body nor the headers fails
with `Error::Build`. A header dialect whose headers are all absent reads as success plus a warning.

## Sources

Each `source.rs` entry states a default host, a path, bearer auth and any extra headers: Qwen's
`{workspace_id}` path plus `x-dashscope-workspace`, Codex's `chatgpt-account-id`, and a Kimi Code
user agent. The `base_url` argument overrides the host, and the server always passes the provider's
own `base_url` (`src/server/http/providers.rs`). `fetch` makes one GET with no retry. A non-`2xx`
body is decoded by `http_error::decode_provider_envelope`.

## Shape

```text
AccountState { protocol, quotas: [QuotaWindow], balances: [Balance], failure, warnings,
               availability, plan_type }
QuotaWindow  { id, name, unit, used, limit, remaining, used_percent, window, resets_at,
               reached, unlimited }
Balance      { currency, available, total, cash, granted, topped_up, voucher, credit, owed,
               minor_unit }
Failure      { kind: Unauthorized | Unpaid | Throttled | Unknown, code, message }
```

Numbers keep the service's exact decimal text. An unlimited allowance is never flattened into a
number. What cannot be represented goes to `warnings`. A refusal the service reports inside a `2xx`
body (a rejected key, an account in arrears) is data in `failure`, not an error.

# Client

`src/client.rs` is one upstream, ready to call: a `ModelUseProtocol` (one wire's
renderer, reader, stream decoder and error envelope), an [`Endpoint`](protocol/endpoint.md) target
(where calls go, how they are proven), `Credentials` (what the account is reached with, placed when
each call is built) and a `Transport` (one attempt each). It does nothing else, except run the retry loop: the
one policy that has to sit where the attempt is made. This page is the only description of retry.

```rust
let transport = ReqwestTransport::new(Limits::default(), Proxy::Environment)?;
let endpoint =
  Endpoint::new("https://api.anthropic.com", "/v1/messages", AuthScheme::Header("x-api-key"))?;
let client = Client::new(
  ModelUseProtocol::AnthropicMessages(MessagesApiCompatMode::default()),
  endpoint,
  transport,
)
.with_credentials(Credentials::from_api_key("sk-ant-..."))
.with_model_list(ModelListProtocol::AnthropicModels)
.with_account_state(AccountStateProtocol::AnthropicRatelimitHeaders);
```

The server builds its clients in [`src/server/provider.rs`](../../src/server/provider.rs).

## Build-time axes

| Axis | Set by | Notes |
| --- | --- | --- |
| `ModelUseProtocol` | `Client::new` | the conversation wire ([model-use](protocol/model-use.md)) |
| `AuthScheme` | the `Endpoint` | placement and renewal ([endpoint](protocol/endpoint.md)) |
| `ModelListProtocol` | `with_model_list` | optional |
| `AccountStateProtocol` | `with_account_state` | optional |
| `UpstreamCompactionProtocol` | `with_upstream_compaction` | optional; pairing checked, returns `Result` |
| `TokenCountProtocol` | `with_token_count` | optional; pairing checked, returns `Result` |
| `CodeAgentIdentity` | `with_code_agent_identity` | optional: `Codex` or `Claude` body identifiers, `Copilot` call headers |

Asking a client for a feature it was not given, or one its protocol cannot serve, is
`Error::Unsupported`, which is never retried. Some refusals come from a renderer instead and are
`Error::Build`; see [error](protocol/error.md).

`with_session_id` binds the ID that fills `{session}` header templates and the code-agent body
fields. Unbound, each model call gets a fresh UUID. With `CodeAgentIdentity::Codex` on Responses,
the body gets `prompt_cache_key` (unless set) and `client_metadata{session_id,thread_id}`. With
`Claude` on Messages it gets `metadata.user_id`. With `Copilot`, every model call carries
`x-initiator`: `user` when the conversation ends on a message a person sent, `agent` when it ends
on a tool's result or what a tool handed the model (a user message whose metadata names the
tool's call); a conversation with an image adds `copilot-vision-request: true`.
`with_attempt_observer` attaches an `AttemptObserver`, made for every physical streaming attempt,
retries and streamed compaction included. The server uses it for throughput sampling ([statistics](statistics.md)).

## Credentials

For a subscription, the credentials carry tokens instead of a key (`api_key` = access token,
`refresh_token`, `expires_at`, `account_id`). The auth scheme carries the renewal beside the
placement: `AuthScheme::Bearer(Some(CredentialRenewal::CodexOAuth))`, or `CopilotToken` for a
Copilot session renewed from the GitHub token in `refresh_token`.
`Client::refresh_credentials` returns renewed credentials and installs them nowhere; the server
saves them in the provider's configuration, which builds the client again. A `build_call` over spent credentials is refused
as `Error::Renewal` before anything is sent. Storing what the exchange rotated stays with the
caller.

## Calls

`call(&request)` is the single model-use entry point. `Request.stream` selects its result:

- `false` returns `CallResponse::Complete(Box<Response>)`.
- `true` returns `CallResponse::Stream(EventStream)` (`src/client/stream.rs`).

`CallResponse` belongs to the executor's `ModelCaller` contract (`executor::model`), which
`Client` implements: the executor only knows that trait ([executor](executor.md)).

The same field drives request construction: the body `stream`/usage options where the wire has
them, or the streaming URL for Google GenerateContent and Bedrock. The Codex deployment refuses a
buffered request locally with `Error::Build`, so `stream: false` never reaches it.

```rust
request.stream = true; // false uses the same call and returns a complete response
let response = match client.call(&request).await? {
  CallResponse::Complete(response) => *response,
  CallResponse::Stream(mut stream) => {
    let mut accumulator = stream.create_accumulator();
    while let Some(event) = stream.next().await? {
      accumulator.feed(event)?;
    }
    accumulator.finish()?
  }
};
```

For explicit interruption, keep the accumulator outside pending reads, abort the stream, then call
`accumulator.interrupt(ToolExecutionState::NotStarted)` if none of its tools ran. The protocol
returns eligible content with tool results already paired; see
[interrupted streams](protocol/model-use.md#interrupted-streams). Output-limit continuation is not
done here: `call` returns the provider's response and stop reason as-is
([executor](executor.md#automatic-output-continuation)).

`compact_upstream` takes an `UpstreamCompactionRequest` over the configured compaction protocol
and returns the compacted conversation
([upstream-compaction](protocol/upstream-compaction.md)).

`count_tokens(&request)` needs `with_token_count(TokenCountProtocol)`. It reuses the client's
endpoint, credentials, transport and retry policy, and always sends a buffered count request
([token-count](protocol/token-count.md)).

Two reads, each over its own protocol and each a single attempt without retry:

- `get_model_list(&ModelListQuery)`: one page of the upstream's models. The query names the host
  and path; auth comes from the protocol's source table
  ([model-list](protocol/model-list.md)).
- `get_account_state(base_url)`: what the upstream says about the account behind the key
  ([account-state](protocol/account-state.md)). `base_url` overrides the source's host.

`with_account_state` serves two readings. A reply-borne protocol (rate-limit headers, the Codex
quota frame) fills `Response::account_state`, and the accumulator of a stream, on every call. If
that reading cannot be parsed, the model call itself fails. A protocol with a request of its own
is read by `get_account_state`.

## Retry

`utils::retry` owns `RetryPolicy` and a generic `retry(policy, attempt, decide)` loop. The client
supplies the decision: `Error::is_retryable` ([error](protocol/error.md)), mapped by
`classify_retry`. Nothing in the executor retries, and model retries never repeat tool side
effects.

| Lane | Loop | What is replaced |
| --- | --- | --- |
| buffered `call`, `count_tokens`, `compact_upstream` | `utils::retry::retry` | the whole attempt |
| streamed `call` | `utils::retry::retry` | the attempt: the opening and its first event |
| `get_model_list`, `get_account_state`, `search`, Codex and Copilot token exchanges | none | nothing: one attempt |

A streamed attempt reads its first event before the stream is handed over. Until then a retryable
failure replaces the attempt. After that, failures are terminal, because a replay would splice a
second copy of the answer into what the caller has seen. Streamed Codex compaction is the
exception: it replaces the whole attempt, since nothing reaches the caller before the compaction
item does.

Every client retries by `RetryPolicy::default()`: 3 attempts, 500 ms initial delay, ×2 per
attempt, 30 s cap, full jitter. An upstream `Retry-After` (delta-seconds or HTTP date, capped at
1 h) is a floor on the delay.

## Limits and the stream-head retry

`Limits` and `Proxy` are set on `ReqwestTransport` and bound each attempt.

| `Limits` field | Default |
| --- | --- |
| `connect` | 10 s |
| `first_byte` (response head) | 60 s |
| `idle` (gap between body chunks) | 60 s |
| `total` | 300 s |
| `max_response_bytes` / `max_error_body_bytes` | 32 MiB / 64 KiB |
| `max_event_bytes` / `max_stream_events` | 8 MiB / 100 000 |

`ReqwestTransport::with_stream_total` replaces `total` for streamed attempts. The server sets it
to 30 minutes. A stream's total deadline restarts when its head arrives. `Proxy` is `Environment`,
`Disabled` or `Manual{url, basic_auth}`.

Which transport failures retry:

| Phase | Retryable | Why |
| --- | --- | --- |
| `Connect` | yes | nothing reached the service |
| `AwaitStreamHeaders` | yes | a service sends a stream's head when it admits the call; no head within `min(first_byte, total)` means the call is stuck before generation |
| `AwaitHeaders` (buffered) | no | a buffered head comes with the whole answer, so the service may be generating and billing |
| `ReadBody` | no | the service started answering |

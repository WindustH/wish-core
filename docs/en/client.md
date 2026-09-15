# Client

`src/client.rs` is one upstream, ready to call: a `ModelUseProtocol` (one wire's renderer, reader, stream
decoder and error envelope), an [`Outbound`](protocol/outbound.md) target (where calls go, how
they are proven), `Credentials` (the account's material, placed at dispatch time) and a
`Transport` (one attempt each). It deliberately does nothing else - and owns the retry loop, the
one policy that has to sit where the attempt is made.

```rust
let transport = ReqwestTransport::new(Limits::default(), Proxy::Environment)?;
let outbound =
  Outbound::new("https://api.anthropic.com", "/v1/messages", AuthProtocol::Header("x-api-key"))?;
let client = Client::new(
  ModelUseProtocol::AnthropicMessages(MessagesApiCompatMode::default()),
  outbound,
  transport,
)
.with_credentials(Credentials::key("sk-ant-..."))
.with_model_list(ModelListProtocol::AnthropicModels)
.with_account_state(AccountStateProtocol::AnthropicRatelimitHeaders)
.with_retry(RetryPolicy::default());
```

Five protocol axes, all named when the client is built: `ModelUseProtocol` (what the conversation
wire speaks), `AuthProtocol` (how calls are proven, on the
[`Outbound`](protocol/outbound.md)), and the three reads - `ModelListProtocol`,
`AccountStateProtocol`, `UpstreamCompactionProtocol` - each optional, each the protocol kind of
its own feature rather than a property of the conversation wire. `with_upstream_compaction`
validates the pairing with the model-use protocol at build time; asking a client for a feature it
was not given, or one the chosen protocol cannot serve, is `Error::Unsupported` - typed,
readable, and never worth a retry.

The same protocol through a gateway is the same lines with another base URL. For a subscription,
the material carries tokens instead of a key (`api_key` = access token, `refresh_token`,
`expires_at`, `account_id`), and the auth protocol carries the renewal beside the placement
(`AuthProtocol::Bearer(Some(CredentialsRefreshProtocol::OAuth))`, say):
`Client::credentials_expired` asks whether the material has run out, `Client::refresh_credentials`
returns the renewed material, `Client::set_credentials` installs it - and a `dispatch` over spent
material is refused as `Error::Renewal` before anything is sent. Storing what the exchange rotated
stays with the caller.

Three lanes take a `Request` or an `UpstreamCompactionRequest` in this crate's shape:

- `call` - the buffered lane: decode the `2xx` body into a `Response`, classify the non-`2xx` one
  into an `Error`. A transient failure replaces the whole attempt while the policy allows.
- `call_stream` - the streamed lane: an `EventStream` of the protocol's `StreamEvent`s, nothing
  assembled on the way. Retries stop at the first event handed over - a replay would splice a
  second copy of the answer into what the caller has already seen.
- `upstream_compact` - the call that asks a service to stand in for a conversation
  ([upstream-compaction](protocol/upstream-compaction.md)), over the compaction protocol named at
  build time.

Two more read what the upstream serves, each over its own protocol:

- `get_model_list` - one page of the upstream's models
  ([model-list](protocol/model-list.md)).
- `get_account_state` - what the upstream says about the account behind the key
  ([account-state](protocol/account-state.md)); `base_url` overrides where the ask goes, for a
  reading kept behind another door than the conversation target.

```rust
let mut stream = client.call_stream(&request).await?;
let mut accumulator = StreamAccumulator::new().with_account_state(stream.account_state().cloned());
while let Some(event) = stream.next().await? {
  accumulator.feed(event)?;
}
let response = accumulator.finish()?;
```

`with_account_state` serves two readings at once: a protocol a reply carries fills
`Response::account_state` and `stream.account_state()` on every call, and a protocol with a
request of its own is read by `Client::get_account_state`. A protocol from the wrong side of that
divide fails the ask it cannot serve, with the reason its feature states. `Limits` and `Proxy`,
set on the transport, bound the attempt - the caller's facts, because only the caller knows its
own network.

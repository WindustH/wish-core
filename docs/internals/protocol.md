# Protocol

`src/protocol/` is the intermediate layer as a whole. It receives a request in our intermediate
representation and the outbound configuration it is aimed at, and converts them into one HTTP call:
method, url, headers and body. It also receives what an upstream answered, an HTTP body or the
records of a stream, and converts that into the corresponding intermediate representation. Each
part has its own page:

- [model-use](protocol/model-use.md): what is said to and by the model.
- [token-count](protocol/token-count.md): a provider's count of a request's input.
- [upstream-compaction](protocol/upstream-compaction.md): asking a service to stand in for a
  conversation.
- [account-state](protocol/account-state.md) and [model-list](protocol/model-list.md): what a
  service says about the account and its models.
- [outbound](protocol/outbound.md): the target, the auth plan and the material every call is
  reached with.
- [error](protocol/error.md): the error vocabulary, including `http_error.rs`'s decoding of
  non-`2xx` envelopes.

Each feature names its protocols with its own enum: `ModelUseProtocol`, `TokenCountProtocol`,
`UpstreamCompactionProtocol`, `AccountStateProtocol`, `ModelListProtocol`, and on the outbound
`AuthProtocol` with its `CredentialsRefreshProtocol` option. A feature a protocol does not carry is
reported as a typed error. The client reports a missing or unpaired feature as
`Error::Unsupported`. A renderer refusing a request it cannot express, such as a buffered Codex
call or an `UpstreamCompaction` message on another wire, reports `Error::Build`.

Every module is a converter: one thing in, one thing out. Each capability of the binary is a
composition of a few of them.

```
 what each module converts
 ─────────────────────────

 model_use/request/<dialect>     Request ──────────────────▶ a JSON body (the client wraps it in a Draft)
 model_use/response/<dialect>    a reply body ─────────────▶ Response
 model_use/stream/<dialect>      wire records ─▶ StreamEvent ─▶ StreamAccumulator ─▶ Response
 token_count                      Request ─▶ a count body;   a reply ─▶ TokenCount
 upstream_compaction              UpstreamCompactionRequest ─▶ a body;  a reply ─▶ UpstreamCompaction
 account_state/<service>          a body, or a reply head ──▶ AccountState
 model_list/<dialect>             one page ─────────────────▶ ModelCatalog
 http_error                       a non-2xx body ───────────▶ Error::Upstream
 outbound                         a Draft × an Outbound × Credentials ─▶ Call
 outbound/oauth                   a refresh token ──────────▶ Tokens
 outbound/adc                     an ADC file ──────────────▶ an access token
 outbound/sigv4                   AWS material ─────────────▶ signed headers
 wire                             the container: Call, Reply, ReplyStream
```

```
 how they compose
 ────────────────

 a conversation call                        bound by executor/model/client.rs
   Request  ─▶ request/<dialect> ─▶ a Draft ──┐
   Outbound × Credentials ─▶ dispatch ────────┼─▶ Call ─▶ transport ─▶ HTTP
                                              │
   Response ◀─ response/<dialect> ◀─ body  ───┤◀─ Reply          the buffered lane
   Response ◀─ stream/<dialect> ◀─ records ───┘◀─ ReplyStream  the streamed lane

 an account read                            bound by account_state/fetch.rs
   the entry's Outbound × Credentials ─▶ dispatch ─▶ Call ─▶ transport ─▶ HTTP
   AccountState ◀─ account_state/<service> ◀─ the body

 a model list                               bound by model_list/fetch.rs
   the entry's Outbound × Credentials ─▶ dispatch ─▶ Call ─▶ transport ─▶ HTTP
   ModelCatalog ◀─ model_list/<dialect> ◀─ one page

 a token refresh                            when to refresh is the caller's call
   a credential ─▶ outbound/oauth or outbound/adc ─▶ transport ─▶ new material
```

The two lanes of a conversation call share the request and the reply head, and differ only in what
reads the answer. Token counts and compactions ride the conversation client too: their bodies are
rendered by their own renderers and sent through the same dispatch, and what comes back is decoded
as one more reply. An account reading often needs no read of its own, because a served call
reports one in its reply head (`account_state::parse_reply`).

Every crossing to and from the network goes through the same container: `Call`, `Reply` and
`ReplyStream` (`wire.rs`) are the only values that pass between a converter and a transport, so no
dialect knows HTTP and no transport knows a body. The binders hold both ends:

- `executor/model/client.rs` binds the conversation wire and runs the retry loops. Retry is
  described in [client](client.md#retry).
- Each read's `fetch.rs`, and the OAuth and ADC exchanges, bind their own round trip. Each is one
  attempt with no retry.
